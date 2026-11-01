//! Logging to a daily `<name>.<date>` file in the log folder. Off unless switched on from the
//! tray (remembered across restarts), by `--log` or by `RUST_LOG`.

use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::paths;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// The log file, created on the first write so that nothing is created while logging is off.
struct LazyFile {
    name: String,
    file: Option<RollingFileAppender>,
}

impl io::Write for LazyFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(
                RollingFileAppender::builder()
                    .rotation(Rotation::DAILY)
                    .filename_prefix(&self.name)
                    .build(paths::log_dir())
                    .map_err(io::Error::other)?,
            ),
        };
        file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), io::Write::flush)
    }
}

/// Sets up logging to `name`, on if the tray's switch `switched_on` says so. `filter` (from
/// `--log`) also switches logging on.
pub fn init(name: &str, filter: Option<&str>, switched_on: bool) -> WorkerGuard {
    let enabled = filter.is_some() || std::env::var_os("RUST_LOG").is_some() || switched_on;
    ENABLED.store(enabled, Ordering::Relaxed);
    let (file, guard) = tracing_appender::non_blocking(LazyFile {
        name: name.to_string(),
        file: None,
    });
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(filter.unwrap_or("info")));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(file)
                .with_ansi(false)
                .with_filter(filter_fn(|_| ENABLED.load(Ordering::Relaxed))),
        )
        .with(cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr)))
        .init();
    guard
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Switches logging to the file on or off.
pub fn set_enabled(on: bool) {
    if !on {
        tracing::info!("logging off");
    }
    ENABLED.store(on, Ordering::Relaxed);
    if on {
        tracing::info!("logging on");
    }
}
