use std::{collections::HashMap, path::Path};

use tile_config::Color;
use tile_core::{Rect, WindowId, WindowInfo};
use uiautomation::{UIAutomation, patterns::UIWindowPattern, types::Handle};
use winsafe::{
    self as w, co,
    prelude::{
        Handle as _, shell_IEnumShellItems as _, shell_IShellItem as _, shell_IShellItem2 as _,
    },
};

use crate::{
    frame::{Insets, Placement, set_frame},
    gui::init_com,
    monitors::rect,
};

/// A window that passed the manageability checks.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub id: WindowId,
    pub info: WindowInfo,
    /// Owned dialogs and fixed-size windows should float by default.
    pub floating_hint: bool,
    /// The visible frame, without the invisible resize borders.
    pub rect: Rect,
}

#[derive(Debug)]
pub enum PlaceError {
    /// The window no longer exists.
    Gone,
    /// The window belongs to an elevated process; we can't move it.
    AccessDenied,
    Other(w::co::ERROR),
}

/// Owns the handles of all managed windows and performs operations on them.
pub struct Windows {
    handles: HashMap<WindowId, w::HWND>,
    insets: HashMap<WindowId, Insets>,
    own_pid: u32,
    automation: Option<UIAutomation>,
}

fn id_of(hwnd: &w::HWND) -> WindowId {
    WindowId(hwnd.ptr() as isize)
}

fn colorref(color: Option<Color>) -> w::COLORREF {
    match color {
        Some(c) => w::COLORREF::from_rgb(c.r, c.g, c.b),
        // DWMWA_COLOR_DEFAULT (0xFFFFFFFF).
        None => w::COLORREF::from(co::CLR::NONE),
    }
}

fn has(bits: isize, flag: u32) -> bool {
    bits & flag as isize != 0
}

fn frame_of(hwnd: &w::HWND) -> Option<Rect> {
    match hwnd.DwmGetWindowAttribute(co::DWMWA::EXTENDED_FRAME_BOUNDS) {
        Ok(w::DwmAttr::ExtendedFrameBounds(r)) => Some(rect(r)),
        _ => hwnd.GetWindowRect().ok().map(rect),
    }
}

fn is_cloaked(hwnd: &w::HWND) -> bool {
    matches!(
        hwnd.DwmGetWindowAttribute(co::DWMWA::CLOAKED),
        Ok(w::DwmAttr::Cloaked(c)) if c.raw() != 0
    )
}

fn process_name(pid: u32) -> String {
    w::HPROCESS::OpenProcess(co::PROCESS::QUERY_LIMITED_INFORMATION, false, pid)
        .and_then(|p| p.QueryFullProcessImageName(co::PROCESS_NAME::WIN32))
        .ok()
        .and_then(|path| {
            Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

fn info_of(hwnd: &w::HWND) -> WindowInfo {
    let (_, pid) = hwnd.GetWindowThreadProcessId();
    WindowInfo {
        title: hwnd.GetWindowText().unwrap_or_default(),
        class: hwnd.GetClassName().unwrap_or_default(),
        exe: process_name(pid),
    }
}

/// Applies the manageability rules (visible, uncloaked, top-level, activatable, titled).
fn examine(hwnd: &w::HWND, own_pid: u32) -> Option<Discovered> {
    if !hwnd.IsWindowVisible() || hwnd.IsIconic() || is_cloaked(hwnd) {
        return None;
    }
    let style = hwnd.GetWindowLongPtr(co::GWLP::STYLE);
    let ex_style = hwnd.GetWindowLongPtr(co::GWLP::EXSTYLE);
    if has(style, co::WS::CHILD.raw())
        || has(ex_style, co::WS_EX::TOOLWINDOW.raw())
        || has(ex_style, co::WS_EX::NOACTIVATE.raw())
    {
        return None;
    }
    let owned = hwnd.GetWindow(co::GW::OWNER).is_ok();
    let caption = style & co::WS::CAPTION.raw() as isize == co::WS::CAPTION.raw() as isize;
    if owned && !caption {
        return None;
    }
    let (_, pid) = hwnd.GetWindowThreadProcessId();
    if pid == own_pid {
        return None;
    }
    let info = info_of(hwnd);
    if info.title.is_empty() {
        return None;
    }
    Some(Discovered {
        id: id_of(hwnd),
        info,
        floating_hint: owned || !has(style, co::WS::THICKFRAME.raw()),
        rect: frame_of(hwnd)?,
    })
}

/// Last resort: share the foreground thread's input state so the focus change is allowed.
fn focus_attached(hwnd: &w::HWND) -> bool {
    let Some(current) = w::HWND::GetForegroundWindow() else {
        return false;
    };
    let (foreground_thread, _) = current.GetWindowThreadProcessId();
    let own_thread = w::GetCurrentThreadId();
    if foreground_thread == own_thread
        || w::AttachThreadInput(own_thread, foreground_thread, true).is_err()
    {
        return false;
    }
    let focused = hwnd.SetForegroundWindow();
    let _ = w::AttachThreadInput(own_thread, foreground_thread, false);
    focused
}

/// Brings `hwnd` to the foreground, restoring it if minimised. `false` if Windows refused.
pub fn focus_hwnd(hwnd: &w::HWND) -> bool {
    if hwnd.IsIconic() {
        let _ = hwnd.ShowWindowAsync(co::SW::RESTORE);
    }
    let mut focused = hwnd.SetForegroundWindow();
    if !focused {
        // Windows only lets the process that received the last input steal focus.
        // A zero-distance mouse move makes that us, without any visible side effect.
        let nudge = w::MOUSEINPUT {
            dwFlags: co::MOUSEEVENTF::MOVE,
            ..Default::default()
        };
        let _ = w::SendInput(&[w::HwKbMouse::Mouse(nudge)]);
        focused = hwnd.SetForegroundWindow() || focus_attached(hwnd);
    }
    let _ = hwnd.BringWindowToTop();
    focused
}

/// Puts focus on the desktop. With nothing left to focus on a monitor, Windows would otherwise
/// pick a window on another one.
pub fn focus_desktop() {
    if let Some(desktop) = enumerate(|h| h.GetClassName().is_ok_and(|c| c == "Progman")).first() {
        focus_hwnd(desktop);
    }
}

/// Brings a window of the process `exe` (a file name like `chrome.exe`) on the current virtual
/// desktop to the foreground, even a minimized one. `false` if there is none.
pub fn focus_exe(exe: &str) -> bool {
    let own_pid = w::GetCurrentProcessId();
    let candidates = enumerate(|h| {
        let ex_style = h.GetWindowLongPtr(co::GWLP::EXSTYLE);
        h.IsWindowVisible()
            && !is_cloaked(h)
            && !has(ex_style, co::WS_EX::TOOLWINDOW.raw())
            && h.GetWindow(co::GW::OWNER).is_err()
            && h.GetWindowThreadProcessId().1 != own_pid
            && h.GetWindowText().is_ok_and(|t| !t.is_empty())
            && info_of(h).exe.eq_ignore_ascii_case(exe)
    });
    candidates.first().is_some_and(focus_hwnd)
}

/// The file a `shell:AppsFolder` item starts, like the target of its Start menu shortcut.
/// `ShellExecute` ignores the `runas` verb on the virtual item itself, so this is what gets
/// elevated.
fn apps_folder_target(parsing_name: &str) -> Option<String> {
    let item: w::IShellItem2 =
        w::SHCreateItemFromParsingName(parsing_name, None::<&w::IBindCtx>).ok()?;
    let path = item.GetString(&co::PKEY::Link_TargetParsingPath).ok()?;
    std::path::Path::new(&path).is_file().then_some(path)
}

/// An app Start lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// What Start shows, like `Windows Terminal`.
    pub name: String,
    /// The `shell:AppsFolder` id: an AppUserModelID or a path.
    pub id: String,
}

/// The items of `shell:AppsFolder` with a name and an id. Needs COM on the calling thread.
fn apps_folder() -> w::HrResult<Vec<App>> {
    let folder: w::IShellItem =
        w::SHCreateItemFromParsingName("shell:AppsFolder", None::<&w::IBindCtx>)?;
    let items: w::IEnumShellItems =
        folder.BindToHandler(None::<&w::IBindCtx>, &co::BHID::EnumItems)?;
    let mut apps = Vec::new();
    for item in items.iter() {
        let item = item?;
        let (Ok(name), Ok(id)) = (
            item.GetDisplayName(co::SIGDN::NORMALDISPLAY),
            item.GetDisplayName(co::SIGDN::PARENTRELATIVEPARSING),
        ) else {
            continue;
        };
        if !name.trim().is_empty() && !id.is_empty() {
            apps.push(App { name, id });
        }
    }
    Ok(apps)
}

/// Everything Start lists, by name, without duplicates. Runs COM on the calling thread.
pub fn list_apps() -> Vec<App> {
    let _com = init_com();
    let mut apps = apps_folder().unwrap_or_else(|e| {
        tracing::warn!("couldn't list apps: {e}");
        Vec::new()
    });
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.id.clone()));
    apps.dedup();
    apps
}

/// Starts a file, folder or `shell:` path as administrator, asking for consent unless Tile
/// already runs elevated. Returns at once; a failure is logged.
pub fn run_as_admin(target: &str) {
    let target = target.to_owned();
    // ShellExecute wants a single-threaded COM apartment of its own.
    std::thread::spawn(move || {
        let _com = init_com();
        let target = if target.starts_with("shell:AppsFolder") {
            apps_folder_target(&target).unwrap_or(target)
        } else {
            target
        };
        if let Err(e) = w::HWND::NULL.ShellExecute("runas", &target, None, None, co::SW::SHOWNORMAL)
        {
            tracing::warn!("couldn't run `{target}` as administrator: {e}");
        }
    });
}

/// Closes the foreground window like Alt+F4, unless it belongs to the shell.
pub fn close_foreground() {
    if let Some(raw) = foreground()
        && !is_shell_window(raw)
        && !tile_sys::post_close(raw)
    {
        tracing::debug!("couldn't ask {raw:#x} to close");
    }
}

/// Enumerates top-level windows, keeping the handles `keep` selects.
/// The top-level windows for which `keep` returns true.
pub fn enumerate(mut keep: impl FnMut(&w::HWND) -> bool) -> Vec<w::HWND> {
    let mut out = Vec::new();
    let result = w::EnumWindows(|hwnd: w::HWND| {
        if keep(&hwnd) {
            out.push(hwnd);
        }
        true
    });
    if let Err(e) = result {
        tracing::warn!("EnumWindows failed: {e}");
    }
    out
}

impl Default for Windows {
    fn default() -> Self {
        Self::new()
    }
}

impl Windows {
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
            insets: HashMap::new(),
            own_pid: w::GetCurrentProcessId(),
            automation: None,
        }
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.handles.contains_key(&id)
    }

    /// Finds every manageable window not yet known, and takes ownership of its handle.
    pub fn discover_all(&mut self) -> Vec<Discovered> {
        let own_pid = self.own_pid;
        let mut found = Vec::new();
        for hwnd in enumerate(|h| !self.handles.contains_key(&id_of(h))) {
            if let Some(d) = examine(&hwnd, own_pid) {
                found.push(d);
                self.handles.insert(id_of(&hwnd), hwnd);
            }
        }
        found
    }

    /// Checks a single window reported by an event.
    pub fn discover(&mut self, raw: isize) -> Option<Discovered> {
        let id = WindowId(raw);
        if self.handles.contains_key(&id) {
            return None;
        }
        let hwnd = tile_sys::hwnd(raw)?;
        let found = examine(&hwnd, self.own_pid)?;
        self.handles.insert(id, hwnd);
        Some(found)
    }

    pub fn forget(&mut self, id: WindowId) {
        self.handles.remove(&id);
        self.insets.remove(&id);
    }

    pub fn info(&self, id: WindowId) -> Option<WindowInfo> {
        self.handles.get(&id).map(info_of)
    }

    pub fn frame(&self, id: WindowId) -> Option<Rect> {
        self.handles.get(&id).and_then(frame_of)
    }

    /// The window's invisible borders, for animating it with a [`Mover`](crate::Mover).
    pub fn insets_of(&mut self, id: WindowId) -> Option<Insets> {
        let Self {
            handles, insets, ..
        } = self;
        Some(measure_insets(insets, id, handles.get(&id)?))
    }

    pub fn is_alive(&self, id: WindowId) -> bool {
        self.handles.get(&id).is_some_and(w::HWND::IsWindow)
    }

    /// Whether the window is still visible to the user (not hidden or cloaked by its app).
    pub fn is_visible(&self, id: WindowId) -> bool {
        self.handles
            .get(&id)
            .is_some_and(|h| h.IsWindowVisible() && !is_cloaked(h))
    }

    /// Moves a window so its visible frame covers `target`.
    ///
    /// With `force`, the window isn't sent `WM_WINDOWPOSCHANGING`, which is where Windows
    /// applies an app's minimum size; this lets us shrink windows below it.
    ///
    /// Whenever the size changes the old contents are discarded, so apps like Explorer don't
    /// keep stale fragments while resizing. `settled` marks the last placement of a move (not
    /// an animation frame): the frame is recomputed and the whole window repainted.
    pub fn place(
        &mut self,
        id: WindowId,
        target: Rect,
        force: bool,
        settled: bool,
    ) -> Result<(), PlaceError> {
        let Self {
            handles, insets, ..
        } = self;
        let hwnd = handles.get(&id).ok_or(PlaceError::Gone)?;
        if hwnd.IsZoomed() {
            let _ = hwnd.ShowWindowAsync(co::SW::RESTORE);
        }
        let insets = measure_insets(insets, id, hwnd);
        let how = Placement {
            force,
            settled,
            sync: false,
        };
        set_frame(hwnd, target, insets, how).map_err(|e| match e {
            co::ERROR::ACCESS_DENIED => PlaceError::AccessDenied,
            co::ERROR::INVALID_WINDOW_HANDLE => PlaceError::Gone,
            other => PlaceError::Other(other),
        })
    }

    pub fn show(&self, id: WindowId) {
        if let Some(hwnd) = self.handles.get(&id) {
            let _ = hwnd.ShowWindowAsync(co::SW::SHOWNA);
        }
    }

    pub fn hide(&self, id: WindowId) {
        if let Some(hwnd) = self.handles.get(&id) {
            let _ = hwnd.ShowWindowAsync(co::SW::HIDE);
        }
    }

    /// Brings a window to the foreground. Returns `false` if Windows refused.
    pub fn focus(&self, id: WindowId) -> bool {
        self.handles.get(&id).is_some_and(focus_hwnd)
    }

    /// Asks the window to close, like clicking its close button.
    pub fn close(&mut self, id: WindowId) {
        if !self.handles.contains_key(&id) {
            return;
        }
        if self.automation.is_none() {
            match UIAutomation::new() {
                Ok(a) => self.automation = Some(a),
                Err(e) => {
                    tracing::error!("UI Automation unavailable: {e}");
                    return;
                }
            }
        }
        let Some(automation) = &self.automation else {
            return;
        };
        let result = automation
            .element_from_handle(Handle::from(id.0))
            .and_then(|el| el.get_pattern::<UIWindowPattern>())
            .and_then(|pattern| pattern.close());
        if let Err(e) = result {
            tracing::warn!("failed to close {id:?}: {e}");
        }
    }

    /// Shows or hides the window's title bar. Returns `false` if nothing changed
    /// (e.g. apps that draw their own title bar have no `WS_CAPTION` to remove).
    pub fn set_decorated(&self, id: WindowId, on: bool) -> bool {
        self.handles.get(&id).is_some_and(|h| set_caption(h, on))
    }

    /// Sets the DWM border color (Windows 11+). `None` restores the system default.
    pub fn set_border(&self, id: WindowId, color: Option<Color>) {
        let Some(hwnd) = self.handles.get(&id) else {
            return;
        };
        if let Err(e) = hwnd.DwmSetWindowAttribute(w::DwmAttr::BorderColor(colorref(color))) {
            tracing::trace!("border color not supported: {e}");
        }
    }

    /// Whether the window's border currently has `color`; `None` if DWM won't say.
    /// Apps (Chromium/Electron especially) can overwrite the color we set.
    pub fn border_is(&self, id: WindowId, color: Option<Color>) -> Option<bool> {
        let hwnd = self.handles.get(&id)?;
        match hwnd.DwmGetWindowAttribute(co::DWMWA::BORDER_COLOR) {
            Ok(w::DwmAttr::BorderColor(current)) => Some(current == colorref(color)),
            _ => None,
        }
    }
}

/// The invisible borders of `hwnd`, measured while it is visible and otherwise the last good
/// measurement in `saved`: hidden windows report bogus frame bounds.
fn measure_insets(saved: &mut HashMap<WindowId, Insets>, id: WindowId, hwnd: &w::HWND) -> Insets {
    if hwnd.IsWindowVisible()
        && let (Ok(outer), Some(frame)) = (hwnd.GetWindowRect(), frame_of(hwnd))
    {
        let measured = Insets {
            left: frame.x - outer.left,
            top: frame.y - outer.top,
            right: outer.right - frame.right(),
            bottom: outer.bottom - frame.bottom(),
        };
        let plausible = [measured.left, measured.top, measured.right, measured.bottom]
            .iter()
            .all(|v| (0..=64).contains(v));
        if plausible {
            saved.insert(id, measured);
        }
    }
    saved.get(&id).copied().unwrap_or_default()
}

/// Adds or removes the title bar (`WS_CAPTION`). Returns whether the style changed.
fn set_caption(hwnd: &w::HWND, on: bool) -> bool {
    let style = hwnd.style();
    let caption = co::WS::CAPTION;
    let has_caption = style & caption == caption;
    if on == has_caption {
        return false;
    }
    hwnd.set_style(if on {
        style | caption
    } else {
        style & !caption
    });
    // Make Windows recompute the non-client area for the new style.
    let flags = co::SWP::NOMOVE
        | co::SWP::NOSIZE
        | co::SWP::NOZORDER
        | co::SWP::NOACTIVATE
        | co::SWP::NOOWNERZORDER
        | co::SWP::FRAMECHANGED
        | co::SWP::ASYNCWINDOWPOS;
    let _ = hwnd.SetWindowPos(
        w::HwndPlace::None,
        w::POINT::default(),
        w::SIZE::default(),
        flags,
    );
    true
}

/// Puts back windows a previous instance changed: re-adds title bars and re-shows hidden
/// windows. Used on startup, from the panic hook and by the watchdog process.
pub fn restore_windows(hidden: &[isize], undecorated: &[isize]) {
    for &raw in undecorated {
        if let Some(hwnd) = tile_sys::hwnd(raw) {
            set_caption(&hwnd, true);
        }
    }
    for &raw in hidden {
        if let Some(hwnd) = tile_sys::hwnd(raw)
            && !hwnd.IsWindowVisible()
        {
            let _ = hwnd.ShowWindowAsync(co::SW::SHOWNA);
        }
    }
}

/// Blocks until the process exits. `false` if it can't be watched, so may still be running.
pub fn wait_for_exit(pid: u32) -> bool {
    w::HPROCESS::OpenProcess(co::PROCESS::SYNCHRONIZE, false, pid)
        .is_ok_and(|process| process.WaitForSingleObject(None).is_ok())
}

/// Whether a top-level window belongs to the shell (desktop, taskbar, Start, task view).
/// Focus moving there shouldn't count as leaving the tiled windows.
pub fn is_shell_window(raw: isize) -> bool {
    const SHELL_CLASSES: [&str; 6] = [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "XamlExplorerHostIslandWindow",
        "Windows.UI.Core.CoreWindow",
    ];
    tile_sys::class_name(raw).is_some_and(|class| SHELL_CLASSES.contains(&class.as_str()))
}

/// The current foreground window's raw handle.
pub fn foreground() -> Option<isize> {
    w::HWND::GetForegroundWindow().map(|h| h.ptr() as isize)
}

pub fn cursor_pos() -> (i32, i32) {
    w::GetCursorPos().map(|p| (p.x, p.y)).unwrap_or_default()
}

/// Moves the pointer to `(x, y)` in physical screen pixels.
pub fn set_cursor_pos((x, y): (i32, i32)) {
    if let Err(e) = w::SetCursorPos(x, y) {
        tracing::debug!("SetCursorPos failed: {e}");
    }
}

/// A mouse button (left, right, middle or a side button) is held down.
pub fn mouse_button_down() -> bool {
    [
        co::VK::LBUTTON,
        co::VK::RBUTTON,
        co::VK::MBUTTON,
        co::VK::XBUTTON1,
        co::VK::XBUTTON2,
    ]
    .into_iter()
    .any(w::GetAsyncKeyState)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_app_has_no_target_file() {
        let _com = init_com();
        assert_eq!(
            apps_folder_target(r"shell:AppsFolder\Tile.NoSuchApp!App"),
            None
        );
    }

    #[test]
    fn a_desktop_app_resolves_to_a_real_file() {
        let _com = init_com();
        let resolved = apps_folder()
            .unwrap()
            .into_iter()
            .find_map(|app| apps_folder_target(&format!(r"shell:AppsFolder\{}", app.id)));
        if let Some(path) = resolved {
            assert!(std::path::Path::new(&path).is_file());
        }
    }
}
