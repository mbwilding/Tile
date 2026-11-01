//! Line-delimited JSON over the `\\.\pipe\tile.sock` named pipe.
//!
//! A client sends one [`Request`] line and reads [`Response`] lines until the server closes
//! the connection (or forever, for [`Request::Subscribe`]).

use std::io;

pub mod install;
pub mod launcher;
mod pipe;

use interprocess::{
    local_socket::tokio::{Listener, Stream, prelude::*},
    os::windows::{local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor},
};
use serde::{Deserialize, Serialize};
use tile_core::{
    LayoutKind,
    state::{MonitorSnapshot, WindowSnapshot, WorkspaceSnapshot},
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    sync::mpsc,
};

/// The daemon's pipe, `\\.\pipe\tile.sock`.
pub const SOCKET_NAME: &str = "tile.sock";

/// What a client asks the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Run a dispatcher, e.g. `workspace 3`.
    Dispatch {
        /// The dispatcher and its arguments, as written in a bind.
        command: String,
    },
    /// Answer with what the daemon knows about `what`.
    Query {
        /// What to describe.
        what: Query,
    },
    /// Stream [`Event`]s until the client disconnects.
    Subscribe,
}

/// What a [`Request::Query`] asks about.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Query {
    /// Every monitor, answered with [`Response::Monitors`].
    Monitors,
    /// Every workspace, answered with [`Response::Workspaces`].
    Workspaces,
    /// Every managed window, answered with [`Response::Windows`].
    Windows,
    /// The focused window, answered with [`Response::ActiveWindow`].
    ActiveWindow,
}

/// A line the daemon sends back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The request was carried out.
    Ok,
    /// The request failed.
    Error {
        /// Why, for a person to read.
        message: String,
    },
    /// The answer to [`Query::Monitors`].
    Monitors {
        /// Every monitor.
        monitors: Vec<MonitorSnapshot>,
    },
    /// The answer to [`Query::Workspaces`].
    Workspaces {
        /// Every workspace.
        workspaces: Vec<WorkspaceSnapshot>,
    },
    /// The answer to [`Query::Windows`].
    Windows {
        /// Every managed window.
        windows: Vec<WindowSnapshot>,
    },
    /// The answer to [`Query::ActiveWindow`].
    ActiveWindow {
        /// The focused window, if Tile manages it.
        window: Option<WindowSnapshot>,
    },
    /// Something happened, for a [`Request::Subscribe`] client.
    Event {
        /// What happened.
        event: Event,
    },
}

/// Something that happened, streamed to subscribers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The focused monitor shows another workspace.
    Workspace {
        /// The workspace now shown.
        id: u8,
    },
    /// Focus moved to another window.
    ActiveWindow {
        /// Its handle, or `None` when nothing is focused.
        id: Option<isize>,
        /// Its title.
        title: Option<String>,
        /// The file name of its program.
        exe: Option<String>,
    },
    /// The focused workspace's layout changed.
    Layout {
        /// The layout now in use.
        layout: LayoutKind,
    },
}

/// A request received by the server, with a channel for the responses.
/// Dropping `reply` closes the client connection.
#[derive(Debug)]
pub struct Incoming {
    /// What the client asked.
    pub request: Request,
    /// Sends a [`Response`] line to the client.
    pub reply: mpsc::UnboundedSender<Response>,
}

/// Whether a daemon is already listening.
pub async fn is_running() -> bool {
    pipe::is_listening(SOCKET_NAME).await
}

/// The daemon side of the pipe. Must be created inside a Tokio runtime.
pub struct Server(Listener);

impl Server {
    /// Creates the pipe. With `user_sid`, only that user (and SYSTEM and Administrators) can
    /// connect, including from a normal, non-elevated process when Tile runs elevated.
    pub fn bind(user_sid: Option<&str>) -> io::Result<Self> {
        let mut options = pipe::listener(SOCKET_NAME)?;
        if let Some(sid) = user_sid {
            options = options.security_descriptor(user_only(sid)?);
        }
        options.create_tokio().map(Self)
    }

    /// Accepts connections forever, forwarding each request to `tx`.
    pub async fn serve(self, tx: mpsc::UnboundedSender<Incoming>) {
        serve(self.0, tx).await;
    }
}

/// Full access for SYSTEM, Administrators and the user `sid`. An elevated process's pipe is
/// labelled high integrity by default, which shuts out normal processes; the medium label
/// lets the user's non-elevated `tile` in.
fn user_only(sid: &str) -> io::Result<SecurityDescriptor> {
    let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})S:(ML;;NW;;;ME)");
    let sddl = widestring::U16CString::from_str(sddl).map_err(io::Error::other)?;
    SecurityDescriptor::deserialize(&sddl)
}

async fn serve(listener: Listener, tx: mpsc::UnboundedSender<Incoming>) {
    loop {
        let conn = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("ipc accept failed: {e}");
                continue;
            }
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(conn, tx).await {
                tracing::debug!("ipc connection ended: {e}");
            }
        });
    }
}

async fn handle(conn: Stream, tx: mpsc::UnboundedSender<Incoming>) -> io::Result<()> {
    // An `is_running` probe hangs up without sending anything.
    let Some(line) = pipe::read_text(&mut BufReader::new(&conn)).await? else {
        return Ok(());
    };
    let (reply, mut replies) = mpsc::unbounded_channel();
    match serde_json::from_str::<Request>(&line) {
        Ok(request) => {
            if tx.send(Incoming { request, reply }).is_err() {
                return Ok(());
            }
        }
        Err(e) => {
            let _ = reply.send(Response::Error {
                message: format!("bad request: {e}"),
            });
        }
    }
    while let Some(response) = replies.recv().await {
        pipe::write_line(&conn, &response).await?;
    }
    (&conn).flush().await
}

/// A connection to the daemon.
pub struct Client {
    reader: BufReader<Stream>,
}

impl Client {
    /// Connects to the daemon and sends `request`, for [`Client::next`] to read the answers.
    pub async fn send(request: &Request) -> io::Result<Self> {
        let conn = pipe::connect(SOCKET_NAME).await?;
        pipe::write_line(&conn, request).await?;
        Ok(Self {
            reader: BufReader::new(conn),
        })
    }

    /// The next response, or `None` once the daemon closes the connection.
    pub async fn next(&mut self) -> io::Result<Option<Response>> {
        let Some(line) = pipe::read_text(&mut self.reader).await? else {
            return Ok(None);
        };
        serde_json::from_str(&line)
            .map(Some)
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape() {
        let json = serde_json::to_string(&Request::Dispatch {
            command: "workspace 2".into(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"dispatch","command":"workspace 2"}"#);
        let query: Request =
            serde_json::from_str(r#"{"type":"query","what":"active_window"}"#).unwrap();
        assert!(matches!(
            query,
            Request::Query {
                what: Query::ActiveWindow
            }
        ));
    }

    fn json(value: &impl Serialize) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[test]
    fn subscribe_and_every_query_round_trip() {
        assert_eq!(json(&Request::Subscribe), r#"{"type":"subscribe"}"#);
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"type":"subscribe"}"#),
            Ok(Request::Subscribe)
        ));
        for (name, query) in [
            ("monitors", Query::Monitors),
            ("workspaces", Query::Workspaces),
            ("windows", Query::Windows),
            ("active_window", Query::ActiveWindow),
        ] {
            assert_eq!(json(&query), format!("\"{name}\""));
            let request = format!(r#"{{"type":"query","what":"{name}"}}"#);
            assert!(matches!(
                serde_json::from_str::<Request>(&request),
                Ok(Request::Query { .. })
            ));
        }
    }

    #[test]
    fn responses_have_a_stable_shape() {
        assert_eq!(json(&Response::Ok), r#"{"type":"ok"}"#);
        assert_eq!(
            json(&Response::Error {
                message: "no".into()
            }),
            r#"{"type":"error","message":"no"}"#
        );
        assert_eq!(
            json(&Response::ActiveWindow { window: None }),
            r#"{"type":"active_window","window":null}"#
        );
        assert_eq!(
            json(&Response::Monitors { monitors: vec![] }),
            r#"{"type":"monitors","monitors":[]}"#
        );
    }

    #[test]
    fn events_round_trip() {
        let events = [
            Event::Workspace { id: 3 },
            Event::ActiveWindow {
                id: Some(42),
                title: Some("t".into()),
                exe: None,
            },
            Event::Layout {
                layout: LayoutKind::Scrolling,
            },
        ];
        for event in events {
            let wrapped = json(&Response::Event {
                event: event.clone(),
            });
            match serde_json::from_str::<Response>(&wrapped).unwrap() {
                Response::Event { event: back } => assert_eq!(back, event),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            json(&Event::Workspace { id: 3 }),
            r#"{"kind":"workspace","id":3}"#
        );
    }

    #[test]
    fn garbage_is_not_a_request() {
        assert!(serde_json::from_str::<Request>("not json").is_err());
        assert!(serde_json::from_str::<Request>(r#"{"type":"nope"}"#).is_err());
    }
}
