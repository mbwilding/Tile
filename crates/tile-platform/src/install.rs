//! Where `tile install` puts Tile.

use std::path::PathBuf;

/// The program's file name.
pub const EXE: &str = "Tile.exe";

/// `C:\Program Files\Tile`. Tile runs elevated, so it must live where only admins can write.
pub fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
        .join("Tile")
}

/// The installed `Tile.exe`, where the sign-in shell has to live.
pub fn installed_exe() -> PathBuf {
    install_dir().join(EXE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_lives_in_the_install_dir() {
        assert!(install_dir().ends_with("Tile"));
        assert_eq!(installed_exe().parent(), Some(install_dir().as_path()));
        assert!(installed_exe().ends_with(EXE));
    }
}
