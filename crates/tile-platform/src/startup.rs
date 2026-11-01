//! What Explorer starts at sign-in, for when Tile is the shell: the `RunOnce` and `Run`
//! registry keys and the Startup folders. Windows only does this in Explorer, so with another
//! shell the programs it lists would never start.

use std::{fs, path::PathBuf};

use winsafe::{self as w, co};

const CURRENT: &str = r"Software\Microsoft\Windows\CurrentVersion";
/// Where Task Manager and Settings record which startup programs are switched off.
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
/// A volatile key (gone at sign-out) that says the programs were started this session.
const SESSION_KEY: &str = r"Volatile Environment\Tile";

/// The shared and the user's Startup folders, each with the registry root that records which of
/// its entries are switched off.
fn startup_folders() -> Vec<(PathBuf, w::HKEY)> {
    let tail = r"Microsoft\Windows\Start Menu\Programs\Startup";
    [
        ("ProgramData", w::HKEY::LOCAL_MACHINE),
        ("APPDATA", w::HKEY::CURRENT_USER),
    ]
    .into_iter()
    .filter_map(|(var, root)| Some((PathBuf::from(std::env::var_os(var)?).join(tail), root)))
    .collect()
}

/// Whether the startup entry is switched off in Task Manager (its first byte is odd).
fn disabled(root: &w::HKEY, folder: &str, name: &str) -> bool {
    matches!(
        root.RegGetValue(
            Some(&format!(r"{APPROVED}\{folder}")),
            Some(name),
            co::RRF::RT_REG_BINARY,
        ),
        Ok(w::RegistryValue::Binary(bytes)) if bytes.first().is_some_and(|b| b & 1 == 1)
    )
}

/// The `(name, command)` values of a `Run` or `RunOnce` key.
fn values(root: &w::HKEY, sub_key: &str) -> Vec<(String, String)> {
    let Ok(key) = root.RegOpenKeyEx(Some(sub_key), co::REG_OPTION::default(), co::KEY::READ) else {
        return Vec::new();
    };
    let Ok(names) = key.RegEnumValue() else {
        return Vec::new();
    };
    let names: Vec<String> = names.filter_map(Result::ok).map(|(name, _)| name).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let flags = co::RRF::RT_REG_SZ | co::RRF::RT_REG_EXPAND_SZ;
            match key.RegGetValue(None, Some(&name), flags) {
                Ok(w::RegistryValue::Sz(c) | w::RegistryValue::ExpandSz(c)) => {
                    let command = w::ExpandEnvironmentStrings(&c).unwrap_or(c);
                    (!command.trim().is_empty()).then_some((name, command))
                }
                _ => None,
            }
        })
        .collect()
}

/// Quotes the program of a command line that has an unquoted path with spaces, which `Run`
/// values may have and `start` would split.
pub(crate) fn quote_program(command: &str) -> String {
    let command = command.trim();
    if command.starts_with('"') {
        return command.to_owned();
    }
    let lower = command.to_ascii_lowercase();
    for extension in [".exe", ".bat", ".cmd", ".com", ".lnk"] {
        let end = lower
            .find(&format!("{extension} "))
            .map(|i| i + extension.len())
            .or_else(|| lower.ends_with(extension).then_some(command.len()));
        if let Some(end) = end {
            let (program, rest) = command.split_at(end);
            return if program.contains(' ') {
                format!("\"{program}\"{rest}")
            } else {
                command.to_owned()
            };
        }
    }
    command.to_owned()
}

/// Whether the command is Tile's own helper, which the shell instance of Tile already is.
fn is_tile_helper(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    command.contains("tile.exe") && command.contains("--launcher")
}

/// The command lines Explorer would start at sign-in, in its order, minus those switched off.
/// `RunOnce` entries are removed as they are returned.
pub fn commands() -> Vec<String> {
    let mut out = Vec::new();
    for (root, root_name) in [
        (w::HKEY::LOCAL_MACHINE, "HKLM"),
        (w::HKEY::CURRENT_USER, "HKCU"),
    ] {
        let once = format!(r"{CURRENT}\RunOnce");
        for (name, command) in values(&root, &once) {
            tracing::debug!("startup: {root_name} RunOnce {name}");
            out.push(quote_program(&command));
            let removed = root
                .RegOpenKeyEx(Some(&once), co::REG_OPTION::default(), co::KEY::SET_VALUE)
                .and_then(|key| key.RegDeleteValue(Some(&name)));
            if let Err(e) = removed {
                tracing::debug!("couldn't remove RunOnce value {name}: {e}");
            }
        }
    }
    let mut runs = vec![(w::HKEY::LOCAL_MACHINE, format!(r"{CURRENT}\Run"))];
    runs.push((
        w::HKEY::LOCAL_MACHINE,
        r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run".to_owned(),
    ));
    runs.push((w::HKEY::CURRENT_USER, format!(r"{CURRENT}\Run")));
    for (root, sub_key) in runs {
        for (name, command) in values(&root, &sub_key) {
            if disabled(&root, "Run", &name) || is_tile_helper(&command) {
                continue;
            }
            out.push(quote_program(&command));
        }
    }
    for (folder, root) in startup_folders() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if path.is_dir() || name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            if !disabled(&root, "StartupFolder", &name) {
                out.push(format!("\"{}\"", path.display()));
            }
        }
    }
    out
}

/// Whether this is the first time this sign-in session asks, and notes that it has. Tile starting
/// again in the same session (after a crash) then leaves the programs alone.
pub fn first_this_session() -> bool {
    w::HKEY::CURRENT_USER
        .RegCreateKeyEx(
            SESSION_KEY,
            None,
            co::REG_OPTION::VOLATILE,
            co::KEY::READ,
            None,
        )
        .is_ok_and(|(_key, disposition)| disposition == co::REG_DISPOSITION::CREATED_NEW_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(
            quote_program(r"C:\Program Files\App\app.exe -min"),
            r#""C:\Program Files\App\app.exe" -min"#
        );
        assert_eq!(
            quote_program(r"C:\Program Files\App\app.EXE"),
            r#""C:\Program Files\App\app.EXE""#
        );
    }

    #[test]
    fn leaves_other_commands_alone() {
        assert_eq!(
            quote_program(r"C:\app\app.exe -min"),
            r"C:\app\app.exe -min"
        );
        assert_eq!(
            quote_program(r#""C:\Program Files\App\app.exe" -min"#),
            r#""C:\Program Files\App\app.exe" -min"#
        );
        assert_eq!(
            quote_program("rundll32 foo.dll,Bar"),
            "rundll32 foo.dll,Bar"
        );
    }

    #[test]
    fn spots_tiles_own_helper() {
        assert!(is_tile_helper(
            r#""C:\Program Files\Tile\Tile.exe" --launcher"#
        ));
        assert!(!is_tile_helper(r"C:\Program Files\Tile\Tile.exe"));
        assert!(!is_tile_helper(r"C:\other\app.exe --launcher"));
    }
}
