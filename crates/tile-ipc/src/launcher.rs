//! The launcher: `tile.exe --launcher`, running as the normal user, starts programs for an
//! elevated Tile so they don't inherit its admin rights. Each connection carries one JSON
//! [`LaunchRequest`] line over `\\.\pipe\tile-launcher.sock`.

use std::io;

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, Name, ToNsName,
    tokio::{Listener, Stream, prelude::*},
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const SOCKET_NAME: &str = "tile-launcher.sock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaunchRequest {
    /// Run a command line, like the `exec` dispatcher.
    Exec {
        command_line: String,
    },
    /// Start a Store app by its app user model id, like `Microsoft.WindowsStore_8wekyb3d8bbwe!App`.
    Activate {
        aumid: String,
    },
    Exit,
}

fn socket_name() -> io::Result<Name<'static>> {
    SOCKET_NAME.to_ns_name::<GenericNamespaced>()
}

/// Whether a launcher is listening.
pub async fn is_running() -> bool {
    match socket_name() {
        Ok(name) => Stream::connect(name).await.is_ok(),
        Err(_) => false,
    }
}

/// Sends `request` to the launcher.
pub async fn send(request: &LaunchRequest) -> io::Result<()> {
    let conn = Stream::connect(socket_name()?).await?;
    let mut json = serde_json::to_string(request).map_err(io::Error::other)?;
    json.push('\n');
    (&conn).write_all(json.as_bytes()).await
}

/// The launcher side of the pipe. Must be created inside a Tokio runtime.
pub struct LaunchServer(Listener);

impl LaunchServer {
    pub fn bind() -> io::Result<Self> {
        ListenerOptions::new()
            .name(socket_name()?)
            .create_tokio()
            .map(Self)
    }

    /// Waits for the next request, skipping connections that send nothing ([`is_running`]).
    pub async fn next(&self) -> io::Result<LaunchRequest> {
        loop {
            let conn = self.0.accept().await?;
            let mut line = String::new();
            if BufReader::new(&conn).read_line(&mut line).await? > 0 {
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
