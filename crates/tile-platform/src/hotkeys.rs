use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tile_config::{Keybind, Modifiers};
use tile_input::{Decision, Handler, Input, RawKey};
use tokio::sync::mpsc;
use win_hotkeys::VKey;
use winsafe::{self as w, co};

/// An unassigned virtual key: tapping it while Win is down stops Win's release opening Start.
pub(crate) const SILENT_VK: u16 = 0xE8;

/// Set while Win is held and has been used for a mouse drag, so releasing it must not open Start.
pub(crate) static WIN_USED: AtomicBool = AtomicBool::new(false);

const LWIN: u16 = 0x5B;
const RWIN: u16 = 0x5C;

/// Shift, Ctrl, Alt and Win (generic, left and right): pressing one alone isn't typing.
const MODIFIER_VKS: [u16; 11] = [
    0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
];

/// What the hook reports to the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The bind with this index was pressed (and swallowed).
    Bind(usize),
    /// A key that is neither a bind nor a lone modifier went down, while typing is reported
    /// ([`Hotkeys::set_report_typing`]).
    Typed,
}

#[derive(Debug, Clone)]
struct Binding {
    vk: u16,
    modifiers: Modifiers,
    index: usize,
    /// Still fires while paused, like `togglepause` and `exec`.
    always: bool,
}

/// Global hotkeys via a `WH_KEYBOARD_LL` hook, including Win-key combos.
///
/// Matched keys are swallowed (with the Win key's Start menu suppressed) and the index of
/// the bind is sent to the daemon. The bind table can be swapped at any time. Other keys can
/// be reported as typing.
///
/// Start this before the remapper: Windows calls the newest hook first, so the remapper's
/// hook then sees physical keys and this one sees the keys it sends.
#[derive(Clone)]
pub struct Hotkeys {
    table: Arc<RwLock<Vec<Binding>>>,
    paused: Arc<AtomicBool>,
    report_typing: Arc<AtomicBool>,
    disable_start_menu: Arc<AtomicBool>,
}

/// Matches key presses against the bind table, on the hook thread.
struct Matcher {
    hotkeys: Hotkeys,
    tx: mpsc::UnboundedSender<HotkeyEvent>,
    /// The modifiers each held key was pressed with, and when it was last seen.
    chords: HashMap<u16, (Modifiers, Instant)>,
}

/// Auto-repeat of a held key arrives within this long of the last one.
const REPEAT_WINDOW: Duration = Duration::from_millis(1100);

impl Matcher {
    /// The modifiers for a key press. A key that is held keeps the ones it was pressed with, so
    /// letting go of Ctrl before the letter doesn't turn its auto-repeat into a different bind.
    fn chord(&mut self, vk: u16) -> Modifiers {
        let now = Instant::now();
        if let Some((mods, seen)) = self.chords.get_mut(&vk)
            && now.duration_since(*seen) < REPEAT_WINDOW
        {
            *seen = now;
            return *mods;
        }
        let mods = held();
        self.chords.insert(vk, (mods, now));
        mods
    }
}

fn held() -> Modifiers {
    // The generic codes (SHIFT, CONTROL, MENU) are not always set for keys the remapper sends,
    // so each side is asked for itself, as for Win.
    let down = w::GetAsyncKeyState;
    Modifiers {
        super_key: down(co::VK::LWIN) || down(co::VK::RWIN),
        shift: down(co::VK::LSHIFT) || down(co::VK::RSHIFT) || down(co::VK::SHIFT),
        ctrl: down(co::VK::LCONTROL) || down(co::VK::RCONTROL) || down(co::VK::CONTROL),
        alt: down(co::VK::LMENU) || down(co::VK::RMENU) || down(co::VK::MENU),
    }
}

impl Handler for Matcher {
    fn key(&mut self, key: RawKey) -> Decision {
        let win_up = !key.down && matches!(key.vk, LWIN | RWIN);
        if win_up
            && (WIN_USED.swap(false, Ordering::Relaxed)
                || (!key.injected && self.hotkeys.disable_start_menu.load(Ordering::Relaxed)))
        {
            // Windows opens Start when Win goes up with no other key since it went down. Hold
            // the real release back, press a key that does nothing, then send the release, so
            // the order is guaranteed.
            let sent = tile_input::send(&[
                Input::Vk {
                    vk: SILENT_VK,
                    up: false,
                },
                Input::Vk {
                    vk: SILENT_VK,
                    up: true,
                },
                Input::Vk {
                    vk: key.vk,
                    up: true,
                },
            ]);
            // If nothing was sent, let the real release through rather than stick Win down.
            return if sent == 3 {
                Decision::Block
            } else {
                Decision::Pass
            };
        }
        if !key.down {
            self.chords.remove(&key.vk);
        }
        if !key.down || key.vk == SILENT_VK {
            return Decision::Pass;
        }
        let mods = if MODIFIER_VKS.contains(&key.vk) {
            held()
        } else {
            self.chord(key.vk)
        };
        let paused = self.hotkeys.paused.load(Ordering::Relaxed);
        let matched = self.hotkeys.table.read().ok().and_then(|t| {
            t.iter()
                .find(|b| b.vk == key.vk && b.modifiers == mods && (b.always || !paused))
                .map(|b| b.index)
        });
        let Some(event) = matched.map(HotkeyEvent::Bind) else {
            if mods.super_key && !MODIFIER_VKS.contains(&key.vk) {
                tracing::info!("no bind for vk {:#x} with {mods:?}", key.vk);
            }
            if self.hotkeys.report_typing.load(Ordering::Relaxed) && !MODIFIER_VKS.contains(&key.vk)
            {
                let _ = self.tx.send(HotkeyEvent::Typed);
            }
            return Decision::Pass;
        };
        if mods.super_key {
            tracing::info!("bind {event:?} for vk {:#x} with {mods:?}", key.vk);
            tile_input::send(&[
                Input::Vk {
                    vk: SILENT_VK,
                    up: false,
                },
                Input::Vk {
                    vk: SILENT_VK,
                    up: true,
                },
            ]);
        }
        let _ = self.tx.send(event);
        Decision::Block
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn tick(&mut self) {}

    fn wake(&mut self) -> bool {
        true
    }
}

impl Hotkeys {
    /// Installs the keyboard hook on its own thread. Returns once it is installed.
    pub fn start(tx: mpsc::UnboundedSender<HotkeyEvent>) -> io::Result<Self> {
        let hotkeys = Self {
            table: Arc::new(RwLock::new(Vec::new())),
            paused: Arc::new(AtomicBool::new(false)),
            report_typing: Arc::new(AtomicBool::new(false)),
            disable_start_menu: Arc::new(AtomicBool::new(false)),
        };
        let matcher = Matcher {
            hotkeys: hotkeys.clone(),
            tx,
            chords: HashMap::new(),
        };
        tile_input::spawn("hotkey hook", move || matcher)?.detach();
        Ok(hotkeys)
    }

    /// Replaces the bind table. The `bool` marks binds that also work while paused.
    /// Returns a message for every bind whose key isn't recognised.
    pub fn set_binds(&self, binds: &[(Keybind, bool)]) -> Vec<String> {
        let mut errors = Vec::new();
        let table = binds
            .iter()
            .enumerate()
            .filter_map(
                |(index, (bind, always))| match VKey::from_keyname(&bind.key) {
                    Ok(key) => Some(Binding {
                        vk: key.to_vk_code(),
                        modifiers: bind.modifiers,
                        index,
                        always: *always,
                    }),
                    Err(e) => {
                        errors.push(format!("bind `{bind}`: {e}"));
                        None
                    }
                },
            )
            .collect();
        if let Ok(mut t) = self.table.write() {
            *t = table;
        }
        errors
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Stops releasing Win on its own from opening the Start menu (or allows it again).
    pub fn set_disable_start_menu(&self, disable: bool) {
        self.disable_start_menu.store(disable, Ordering::Relaxed);
    }

    /// Sends [`HotkeyEvent::Typed`] for typed keys from now on (or stops).
    pub fn set_report_typing(&self, report: bool) {
        self.report_typing.store(report, Ordering::Relaxed);
    }
}
