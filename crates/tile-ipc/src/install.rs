//! The pipes of `tile install`: the admin step reporting back to the installer that started it,
//! and asking Key, the standalone remapper Tile absorbed, to exit.

use std::{io, time::Duration};

use interprocess::local_socket::tokio::{Listener, prelude::*};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, BufReader};

use crate::pipe;

/// Key's pipe, which takes a plain `exit` line.
const KEY_SOCKET: &str = "key.sock";

/// What the admin step of `tile install` or `uninstall` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Report {
    /// It worked.
    Done {
        /// What to tell the user, possibly empty.
        note: String,
    },
    /// It failed.
    Failed {
        /// Why, for the user to read.
        message: String,
    },
}

/// The installer's end of the report pipe, made before the admin step starts so it can't
/// report too early.
pub struct ReportListener {
    listener: Listener,
    name: String,
}

impl ReportListener {
    /// Creates a report pipe named for this process.
    pub fn bind() -> io::Result<Self> {
        let name = format!("tile-install-{}.sock", std::process::id());
        let listener = pipe::listen(&name)?;
        Ok(Self { listener, name })
    }

    /// The pipe's name, for the admin step's [`send_report`].
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Waits up to `timeout` for the report. Running out of time is a
    /// [`io::ErrorKind::TimedOut`] error.
    pub async fn receive(self, timeout: Duration) -> io::Result<Report> {
        let read = async {
            let conn = self.listener.accept().await?;
            let line = pipe::read_text(&mut BufReader::new(&conn))
                .await?
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
            serde_json::from_str(&line).map_err(io::Error::other)
        };
        tokio::time::timeout(timeout, read)
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    }
}

/// Sends `report` to the installer listening on the pipe `name`, and waits for it to hang up.
pub async fn send_report(name: &str, report: &Report) -> io::Result<()> {
    let conn = pipe::connect(name).await?;
    pipe::write_line(&conn, report).await?;
    // The write finishes in the background: exiting now could cancel it, so wait for the
    // installer to read the report and hang up.
    let _ = (&conn).read_to_end(&mut Vec::new()).await;
    Ok(())
}

/// Whether Key is running.
pub async fn key_running() -> bool {
    pipe::is_listening(KEY_SOCKET).await
}

/// Asks a running Key to exit and waits for its answer. It may still be exiting after.
pub async fn stop_key() -> io::Result<()> {
    let conn = pipe::connect(KEY_SOCKET).await?;
    pipe::write_text(&conn, "exit").await?;
    pipe::read_text(&mut BufReader::new(&conn)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_one_json_line() {
        let report = Report::Failed {
            message: "a\nb".into(),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains('\n'));
        assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), report);
        assert_eq!(
            serde_json::to_string(&Report::Done { note: "x".into() }).unwrap(),
            r#"{"type":"done","note":"x"}"#
        );
    }
}
