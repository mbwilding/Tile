//! The current user's registry values (`HKEY_CURRENT_USER`) that Tile reads and changes: the
//! sign-in shell, the Win+L policy, Click to Do and the `Run` key.

use std::io;

use winsafe::{self as w, co};

fn failed(action: &str, key: &str, name: &str, e: co::ERROR) -> io::Error {
    io::Error::other(format!("couldn't {action} {key}\\{name}: {e}"))
}

/// The `REG_DWORD` value `name` under `key`, if there is one.
pub fn dword(key: &str, name: &str) -> Option<u32> {
    match w::HKEY::CURRENT_USER.RegGetValue(Some(key), Some(name), co::RRF::RT_REG_DWORD) {
        Ok(w::RegistryValue::Dword(v)) => Some(v),
        _ => None,
    }
}

/// Sets the `REG_DWORD` value `name` under `key`, creating the key if needed.
pub fn set_dword(key: &str, name: &str, value: u32) -> io::Result<()> {
    w::HKEY::CURRENT_USER
        .RegSetKeyValue(Some(key), Some(name), w::RegistryValue::Dword(value))
        .map_err(|e| failed("write", key, name, e))
}

/// The `REG_SZ` value `name` under `key`, if there is one.
pub fn sz(key: &str, name: &str) -> Option<String> {
    match w::HKEY::CURRENT_USER.RegGetValue(Some(key), Some(name), co::RRF::RT_REG_SZ) {
        Ok(w::RegistryValue::Sz(text)) => Some(text),
        _ => None,
    }
}

/// Sets the `REG_SZ` value `name` under `key`, creating the key if needed.
pub fn set_sz(key: &str, name: &str, value: &str) -> io::Result<()> {
    w::HKEY::CURRENT_USER
        .RegSetKeyValue(
            Some(key),
            Some(name),
            w::RegistryValue::Sz(value.to_owned()),
        )
        .map_err(|e| failed("write", key, name, e))
}

/// Removes the value `name` under `key`. A value or key that isn't there is already removed.
pub fn delete_value(key: &str, name: &str) -> io::Result<()> {
    delete_value_in(&w::HKEY::CURRENT_USER, key, name)
}

/// [`delete_value`] under another root key.
pub(crate) fn delete_value_in(root: &w::HKEY, key: &str, name: &str) -> io::Result<()> {
    match root
        .RegOpenKeyEx(Some(key), co::REG_OPTION::default(), co::KEY::SET_VALUE)
        .and_then(|k| k.RegDeleteValue(Some(name)))
    {
        Ok(()) | Err(co::ERROR::FILE_NOT_FOUND | co::ERROR::PATH_NOT_FOUND) => Ok(()),
        Err(e) => Err(failed("remove", key, name, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = r"Software\Tile\Tests";

    #[test]
    fn values_round_trip_and_delete_twice() {
        let name = format!("registry-{}", std::process::id());
        set_dword(KEY, &name, 7).unwrap();
        assert_eq!(dword(KEY, &name), Some(7));
        assert_eq!(sz(KEY, &name), None);
        set_sz(KEY, &name, "seven").unwrap();
        assert_eq!(sz(KEY, &name).as_deref(), Some("seven"));
        delete_value(KEY, &name).unwrap();
        delete_value(KEY, &name).unwrap();
        assert_eq!(sz(KEY, &name), None);
        delete_value(r"Software\Tile\NoSuchKey", &name).unwrap();
    }
}
