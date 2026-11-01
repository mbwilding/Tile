//! Key remapping: the engine on its own hook thread, physical keys in, keys to send out.

use std::{
    collections::HashSet,
    io,
    sync::mpsc,
    time::{Duration, Instant},
};

use tile_input::{Decision, Handler, Input, KeyboardHook, RawKey, SILENT_TAP, Waker};
use tile_remap::{Config, Engine, Key, KeyEvent, Out};
use tokio::sync::mpsc::UnboundedSender;

/// Sent from the daemon to the hook thread, followed by a wake.
#[derive(Debug)]
pub enum Command {
    Load(Config),
    Pause(bool),
    /// Forget held keys and layers, e.g. after the session was locked (no key events reach
    /// the hook on the lock screen, so releases are missed).
    Reset,
    Quit,
}

/// Sent from the hook thread to the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The emergency exit was typed.
    Panic,
}

/// The emergency exit: Backspace, Escape and Enter held together (physical keys).
const PANIC: [Key; 3] = [Key::BACKSPACE, Key::ESC, Key::ENTER];

/// The Shift Windows fakes around numpad keys while Num Lock is on (`E0 2A` / `E0 36`).
const FAKE_SHIFTS: [Key; 2] = [Key::new(0x2A, true), Key::new(0x36, true)];

struct Remapper {
    engine: Engine,
    paused: bool,
    commands: mpsc::Receiver<Command>,
    notices: UnboundedSender<Notice>,
    epoch: Instant,
    /// Physical keys down, for the emergency exit.
    physical: HashSet<Key>,
}

impl Remapper {
    fn new(
        config: Config,
        paused: bool,
        commands: mpsc::Receiver<Command>,
        notices: UnboundedSender<Notice>,
    ) -> Self {
        Self {
            engine: Engine::new(config),
            paused,
            commands,
            notices,
            epoch: Instant::now(),
            physical: HashSet::new(),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn reset(&mut self) {
        send(&self.engine.reset());
    }
}

fn send(out: &[Out]) {
    let scan = |key: Key, up| Input::Scan {
        scan: key.scan(),
        extended: key.extended(),
        up,
    };
    let inputs: Vec<Input> = out
        .iter()
        .flat_map(|&o| match o {
            Out::Press(key) => vec![scan(key, false)],
            Out::Release(key) => vec![scan(key, true)],
            Out::Guard => SILENT_TAP.to_vec(),
        })
        .collect();
    let sent = tile_input::send(&inputs);
    if sent != inputs.len() {
        tracing::debug!(
            "sent {sent} of {} keys (blocked by a higher-integrity window?)",
            inputs.len()
        );
    }
}

impl Handler for Remapper {
    fn key(&mut self, raw: RawKey) -> Decision {
        // Input from programs (ours included) and keys Windows synthesises go straight through.
        if raw.injected || raw.scan > 0xFF {
            return Decision::Pass;
        }
        let key = Key::new(raw.scan as u8, raw.extended);
        if FAKE_SHIFTS.contains(&key) {
            return Decision::Pass;
        }
        if raw.down {
            self.physical.insert(key);
        } else {
            self.physical.remove(&key);
        }
        if raw.down && PANIC.iter().all(|k| self.physical.contains(k)) {
            tracing::warn!("emergency exit (Backspace+Escape+Enter)");
            self.reset();
            self.paused = true;
            let _ = self.notices.send(Notice::Panic);
            return Decision::Pass;
        }
        if self.paused {
            return Decision::Pass;
        }
        let verdict = self.engine.event(KeyEvent {
            key,
            down: raw.down,
            time: self.now(),
        });
        send(&verdict.out);
        if verdict.pass {
            Decision::Pass
        } else {
            Decision::Block
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.engine
            .deadline()
            .map(|ms| self.epoch + Duration::from_millis(ms))
    }

    fn tick(&mut self) {
        let out = self.engine.tick(self.now());
        send(&out);
    }

    fn wake(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Load(config) => {
                    self.reset();
                    self.engine = Engine::new(config);
                }
                Command::Pause(paused) => {
                    if paused {
                        self.reset();
                    }
                    self.paused = paused;
                }
                Command::Reset => {
                    self.reset();
                    self.physical.clear();
                }
                Command::Quit => return false,
            }
        }
        true
    }
}

impl Drop for Remapper {
    /// Never leave a key or modifier held down behind us.
    fn drop(&mut self) {
        self.reset();
    }
}

/// The running remapper: its hook thread and the way to reach it.
pub struct Remap {
    commands: mpsc::Sender<Command>,
    waker: Waker,
    hook: KeyboardHook,
}

impl Remap {
    /// Installs the remapping hook. Start it after the hotkey hook, so it sees physical keys
    /// first and the hotkeys see what it sends.
    pub fn start(
        config: Config,
        paused: bool,
        notices: UnboundedSender<Notice>,
    ) -> io::Result<Self> {
        let (commands, command_rx) = mpsc::channel();
        let hook = tile_input::spawn("remap hook", move || {
            Remapper::new(config, paused, command_rx, notices)
        })?;
        Ok(Self {
            commands,
            waker: hook.waker(),
            hook,
        })
    }

    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            self.waker.wake();
        }
    }

    /// Removes the hook, releasing every key the remapper holds.
    pub fn stop(self) {
        self.send(Command::Quit);
        drop(self.hook);
    }
}
