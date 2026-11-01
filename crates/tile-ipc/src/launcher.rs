//! The launcher: `Tile.exe --launcher`, running as the normal user, starts programs for an
//! elevated Tile so they don't inherit its admin rights. Each connection carries one JSON
//! [`LaunchRequest`] line over `\\.\pipe\tile-launcher.sock`.

use std::{io, time::Duration};

use interprocess::local_socket::tokio::{Listener, prelude::*};
use serde::{Deserialize, Serialize};
use tokio::io::BufReader;

use crate::pipe;

/// The launcher's pipe, `\\.\pipe\tile-launcher.sock`.
pub const SOCKET_NAME: &str = "tile-launcher.sock";

/// How long a connection may take to send its request before it is dropped, so one that
/// never does can't hold up the rest.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// What Tile asks the launcher to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaunchRequest {
    /// Run a command line, like the `exec` dispatcher.
    Exec {
        /// The command line, run through `cmd /C start`.
        command_line: String,
    },
    /// Start a Store app by its app user model id, like `Microsoft.WindowsStore_8wekyb3d8bbwe!App`.
    Activate {
        /// The app user model id.
        aumid: String,
    },
    /// Stop the launcher.
    Exit,
}

/// Whether a launcher is listening.
pub async fn is_running() -> bool {
    pipe::is_listening(SOCKET_NAME).await
}

/// Sends `request` to the launcher.
pub async fn send(request: &LaunchRequest) -> io::Result<()> {
    let conn = pipe::connect(SOCKET_NAME).await?;
    pipe::write_line(&conn, request).await
}

/// The launcher side of the pipe. Must be created inside a Tokio runtime.
pub struct LaunchServer(Listener);

impl LaunchServer {
    /// Creates the pipe, failing if another launcher has it.
    pub fn bind() -> io::Result<Self> {
        pipe::listen(SOCKET_NAME).map(Self)
    }

    /// Waits for the next request, skipping connections that send nothing ([`is_running`]).
    /// A connection that doesn't send its line within a moment is an error, not a wait.
    pub async fn next(&self) -> io::Result<LaunchRequest> {
        loop {
            let conn = self.0.accept().await?;
            let mut reader = BufReader::new(&conn);
            let line = tokio::time::timeout(READ_TIMEOUT, pipe::read_text(&mut reader))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no request sent"))??;
            if let Some(line) = line {
                return serde_json::from_str(&line).map_err(io::Error::other);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_json_lines() {
        for request in [
            LaunchRequest::Exec {
                command_line: "notepad.exe".into(),
            },
            LaunchRequest::Activate {
                aumid: "Microsoft.WindowsStore_8wekyb3d8bbwe!App".into(),
            },
            LaunchRequest::Exit,
        ] {
            let json = serde_json::to_string(&request).unwrap();
            assert!(!json.contains('\n'));
            assert_eq!(
                serde_json::from_str::<LaunchRequest>(&json).unwrap(),
                request
            );
        }
        assert_eq!(
            serde_json::to_string(&LaunchRequest::Activate {
                aumid: "a!b".into()
            })
            .unwrap(),
            r#"{"type":"activate","aumid":"a!b"}"#
        );
    }
}
