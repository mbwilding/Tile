use std::path::{Path, PathBuf};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Calls `on_change` whenever the file at `path` is written, created or replaced.
///
/// The parent directory is watched because many editors save by replacing the file.
/// Keep the returned watcher alive for as long as notifications are wanted.
pub fn watch(
    path: &Path,
    on_change: impl Fn() + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let target: PathBuf = path.to_path_buf();
    let file_name = target.file_name().map(ToOwned::to_owned);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }
        if event
            .paths
            .iter()
            .any(|p| p.file_name() == file_name.as_deref())
        {
            on_change();
        }
    })?;
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::mpsc,
        time::{Duration, Instant},
    };

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tile-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `path` until a notification arrives, since a watcher may start a moment late.
    fn changed(rx: &mpsc::Receiver<()>, path: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            fs::write(path, Instant::now().elapsed().as_nanos().to_string()).unwrap();
            if rx.recv_timeout(Duration::from_millis(200)).is_ok() {
                return true;
            }
        }
        false
    }

    #[test]
    fn reports_writes_to_the_file() {
        let dir = scratch("target");
        let path = dir.join("config.yml");
        let (tx, rx) = mpsc::channel();
        let _watcher = watch(&path, move || {
            let _ = tx.send(());
        })
        .unwrap();
        assert!(changed(&rx, &path));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_other_files_in_the_folder() {
        let dir = scratch("other");
        let (tx, rx) = mpsc::channel();
        let _watcher = watch(&dir.join("config.yml"), move || {
            let _ = tx.send(());
        })
        .unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
