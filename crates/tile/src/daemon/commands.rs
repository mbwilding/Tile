//! Running dispatchers and launcher actions.

use std::path::Path;
use tile_core::{Command, Effect, MonitorInfo, Rect, WindowId};
use tile_launcher::system::{Action, Power};
use tile_platform::{close_foreground, cursor_pos, desktops as vd, focus_exe, set_cursor_pos};

use super::Daemon;
use crate::launch;

impl Daemon {
    /// Does what a system row of the launcher asks for.
    pub(super) fn run_system(&mut self, action: Action) {
        if action == Action::Power(Power::Lock) {
            self.run_command(Command::Lock);
        } else if let Some(line) = tile_launcher::system_command_line(&action) {
            launch::exec(&line);
        } else {
            let what = format!("{action:?}");
            let failed = what.clone();
            let spawned = std::thread::Builder::new()
                .name("system action".into())
                .spawn(move || {
                    if !tile_launcher::apply_system_action(&action) {
                        tracing::warn!("couldn't do {failed}");
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!("couldn't do {what}: {e}");
            }
        }
    }

    /// In shell mode the launcher is how everything starts, so it is always bound: a bind of
    /// yours if there is one, else Win+R or, if that is taken, Win+D.
    pub(super) fn shell_launcher_bind(&self) -> Option<(tile_config::Keybind, Command)> {
        let binds = &self.loaded.binds;
        if !self.shell_wanted() || binds.iter().any(|(_, c)| *c == Command::Launcher) {
            return None;
        }
        ["super+r", "super+d"].into_iter().find_map(|keys| {
            let bind: tile_config::Keybind = keys.parse().ok()?;
            let free = !binds.iter().any(|(b, _)| *b == bind);
            if free {
                tracing::info!("shell mode: the launcher is bound to {keys}");
            }
            free.then_some((bind, Command::Launcher))
        })
    }

    /// Installs the binds. While tiling is off or paused only those that
    /// [work while paused](Command::works_while_paused) fire.
    pub(super) fn install_binds(&mut self) {
        self.hotkeys.set_paused(self.paused || !self.tiling);
        // Without the takeover, Windows would lock *and* run the bind.
        let win_l = self.win_l_available();
        if !win_l && self.loaded.binds.iter().any(|(bind, _)| bind.is_win_l()) {
            tracing::info!(
                "Win+L binds are skipped: Windows keeps Win+L (see `general.override_win_l`)"
            );
        }
        let fallback = self.shell_launcher_bind();
        let mut usable: Vec<_> = self
            .loaded
            .binds
            .iter()
            .filter(|(bind, _)| win_l || !bind.is_win_l())
            .collect();
        usable.extend(fallback.as_ref());
        let binds: Vec<_> = usable
            .iter()
            .map(|(bind, command)| (bind.clone(), command.works_while_paused()))
            .collect();
        self.commands = usable.iter().map(|(_, c)| c.clone()).collect();
        self.hotkeys.set_binds(&binds);
        self.hotkeys
            .set_disable_start_menu(self.loaded.config.general.disable_start_menu);
    }

    /// `focusorexec`: focuses a window of the command's program if there is one, otherwise
    /// runs the command.
    /// Focuses a window Tile manages, switching to its workspace first.
    pub(super) fn focus_managed(&mut self, id: WindowId) {
        let effects = self.desktops.with_owner(id, |s| s.window_focused(id));
        self.apply(effects);
        self.windows.focus(id);
        if let Some(frame) = self.windows.frame(id) {
            self.warp_cursor(frame);
        }
    }

    pub(super) fn focus_or_exec(&mut self, command: &str) {
        let exe = Path::new(program(command))
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        if let Some(exe) = exe {
            let current = self.desktops.current_id();
            let focused = self.focused();
            let mut matches: Vec<WindowId> = self
                .desktops
                .managed()
                .into_iter()
                .filter(|id| {
                    self.desktops.owner(*id) == Some(current)
                        && self
                            .windows
                            .info(*id)
                            .is_some_and(|i| i.exe.eq_ignore_ascii_case(&exe))
                })
                .collect();
            matches.sort_by_key(|id| id.0);
            // Pressing again moves on to the next window of the program.
            let next = focused
                .and_then(|f| matches.iter().find(|id| id.0 > f.0))
                .or_else(|| matches.first())
                .copied();
            if let Some(id) = next {
                self.focus_managed(id);
                return;
            }
            if focus_exe(&exe) {
                return;
            }
        }
        launch::exec(command);
    }

    pub(super) fn run_command(&mut self, command: Command) {
        match command {
            Command::Reload => self.reload(),
            Command::TogglePause => self.toggle_pause(),
            Command::ToggleTile => self.set_paused(!self.paused),
            Command::ToggleRemap => self.set_remap_paused(!self.remap_paused),
            Command::ToggleShell => self.toggle_shell(),
            Command::ToggleLog => self.toggle_logging(),
            Command::Exit => self.quit = true,
            Command::Exec(line) => launch::exec(&line),
            Command::FocusOrExec(line) => self.focus_or_exec(&line),
            Command::KillActive => close_foreground(),
            Command::Desktop(target) => vd::switch_desktop(target),
            Command::Lock => self.lock(),
            Command::Launcher => {
                let area = (self.tiling && !self.paused)
                    .then(|| self.focused_monitor().map(|m| m.work_area))
                    .flatten();
                self.launcher.toggle(area);
            }
            _ if self.paused || !self.tiling => {}
            Command::MoveToDesktop(target) => {
                if let Some(id) = self.focused() {
                    vd::move_to_desktop(id.0, target);
                    self.check_desktop(id);
                }
            }
            Command::Pin => {
                if let Some(id) = self.focused() {
                    vd::toggle_pin(id.0);
                    self.check_desktop(id);
                }
            }
            command => {
                let switches =
                    matches!(command, Command::Workspace(_) | Command::MoveToWorkspace(_));
                let before = self.focused_monitor().map(|m| m.id);
                let effects = self.desktops.current().apply(&command);
                let focus = effects.iter().rev().find_map(|e| match e {
                    Effect::Focus(id) => Some(*id),
                    _ => None,
                });
                let window = focus.and_then(|id| {
                    effects.iter().rev().find_map(|e| match e {
                        Effect::Place { id: i, rect } if *i == id => Some(*rect),
                        _ => None,
                    })
                });
                self.apply(effects);
                let moved_monitor = self.focused_monitor().map(|m| m.id) != before;
                let target = window.or_else(|| {
                    self.focused_monitor()
                        .filter(|_| moved_monitor)
                        .map(|m| m.rect)
                });
                if let Some(target) = target {
                    self.warp_cursor(target);
                }
                if switches {
                    self.show_workspace();
                }
            }
        }
    }

    /// After a key moved focus: puts the pointer on the focused window (it stays put if already
    /// over it), then hides it as typing would, until the mouse moves.
    pub(super) fn warp_cursor(&mut self, target: Rect) {
        if !self.loaded.config.cursor.warp {
            return;
        }
        let (x, y) = cursor_pos();
        if !target.contains_point(x, y) {
            set_cursor_pos(target.center());
        }
        if self.cursor_hidden.is_some() {
            // Already hidden by typing: it must not reappear because we moved it.
            self.cursor_hidden = Some(cursor_pos());
        } else {
            self.hide_cursor();
        }
    }

    /// Flashes the focused workspace's number on its monitor.
    pub(super) fn show_workspace(&self) {
        let state = self.desktops.current_ref();
        if let (Some(workspace), Some(monitor)) =
            (state.focused_workspace(), state.focused_monitor())
        {
            self.indicator.show(workspace, monitor.work_area);
        }
    }

    pub(super) fn focused(&self) -> Option<WindowId> {
        self.desktops.current_ref().focused()
    }

    pub(super) fn focused_monitor(&self) -> Option<&MonitorInfo> {
        self.desktops.current_ref().focused_monitor()
    }
}

/// The program a command line starts: its first word, or the quoted path it starts with.
fn program(command: &str) -> &str {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(quoted),
        None => command.split_whitespace().next().unwrap_or(command),
    }
}

#[cfg(test)]
mod tests {
    use super::program;

    #[test]
    fn program_is_the_first_word_or_quoted_path() {
        assert_eq!(program("  notepad.exe file.txt"), "notepad.exe");
        assert_eq!(
            program(r#""C:\Program Files\App\app.exe" --flag"#),
            r"C:\Program Files\App\app.exe"
        );
        assert_eq!(program(r#""unterminated path"#), "unterminated path");
        assert_eq!(program(""), "");
    }
}
