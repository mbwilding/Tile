use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

/// `%LOCALAPPDATA%\Tile`, for logs and runtime state.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Tile")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Changes made to other apps' windows that must be undone if we die.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restore {
    /// Windows we hid (inactive workspaces, scrolled-off columns).
    pub hidden: Vec<isize>,
    /// Windows whose title bar we removed.
    pub undecorated: Vec<isize>,
    /// Windows we cut to their monitor (the scrolling layout).
    #[serde(default)]
    pub clipped: Vec<isize>,
    /// We disabled Win+L (the `DisableLockWorkstation` policy) to use it as a keybind.
    #[serde(default)]
    pub lock_disabled: bool,
    /// We hid the pointer (`cursor.hide_on_key`) by blanking the system cursors.
    #[serde(default)]
    pub cursor_hidden: bool,
    /// We turned the SmartScreen app check off while hosting the shell.
    #[serde(default)]
    pub smartscreen_disabled: bool,
}

impl Restore {
    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty()
            && self.undecorated.is_empty()
            && self.clipped.is_empty()
            && !self.lock_disabled
            && !self.cursor_hidden
            && !self.smartscreen_disabled
    }

    /// Undoes everything recorded here.
    pub fn apply(&self) {
        tile_platform::restore_windows(&self.hidden, &self.undecorated, &self.clipped);
        if self.lock_disabled {
            tile_platform::set_lock_disabled(false);
        }
        if self.cursor_hidden {
            tile_input::restore_cursors();
        }
        if self.smartscreen_disabled {
            tile_platform::set_smartscreen_disabled(false);
        }
    }
}

fn restore_file() -> PathBuf {
    data_dir().join("restore.json")
}

/// Persists the pending restore actions, removing the file when there are none.
pub fn save_restore(restore: &Restore) {
    let path = restore_file();
    let result = if restore.is_empty() {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        fs::create_dir_all(data_dir())
            .and_then(|()| fs::write(&path, serde_json::to_string(restore).unwrap_or_default()))
    };
    if let Err(e) = result {
        tracing::warn!("failed to persist restore state: {e}");
    }
}

pub fn load_restore() -> Restore {
    fs::read_to_string(restore_file())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_recorded_is_empty() {
        assert!(Restore::default().is_empty());
    }

    #[test]
    fn any_recorded_change_makes_it_non_empty() {
        let cases = [
            Restore {
                hidden: vec![1],
                ..Default::default()
            },
            Restore {
                undecorated: vec![1],
                ..Default::default()
            },
            Restore {
                lock_disabled: true,
                ..Default::default()
            },
            Restore {
                cursor_hidden: true,
                ..Default::default()
            },
            Restore {
                smartscreen_disabled: true,
                ..Default::default()
            },
        ];
        for restore in cases {
            assert!(!restore.is_empty(), "{restore:?}");
        }
    }

    #[test]
    fn restore_state_survives_json() {
        let restore = Restore {
            hidden: vec![1, 2],
            undecorated: vec![3],
            clipped: vec![4],
            lock_disabled: true,
            cursor_hidden: true,
            smartscreen_disabled: true,
        };
        let json = serde_json::to_string(&restore).unwrap();
        assert_eq!(serde_json::from_str::<Restore>(&json).unwrap(), restore);
    }

    #[test]
    fn files_from_older_versions_still_load() {
        let old: Restore = serde_json::from_str(r#"{"hidden":[5],"undecorated":[]}"#).unwrap();
        assert_eq!(old.hidden, vec![5]);
        assert!(!old.lock_disabled && !old.cursor_hidden);
    }

    #[test]
    fn data_lives_under_a_tile_folder() {
        assert!(data_dir().ends_with("Tile"));
        assert!(log_dir().starts_with(data_dir()));
    }
}
