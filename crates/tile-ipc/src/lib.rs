//! Line-delimited JSON over the `\\.\pipe\tile.sock` named pipe.
//!
//! A client sends one [`Request`] line and reads [`Response`] lines until the server closes
//! the connection (or forever, for [`Request::Subscribe`]).

use std::io;

pub mod launcher;

use interprocess::{
    local_socket::{
        GenericNamespaced, ListenerOptions, Name, ToNsName,
        tokio::{Listener, Stream, prelude::*},
    },
    os::windows::{local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor},
};
use serde::{Deserialize, Serialize};
use tile_core::{
    LayoutKind,
    state::{MonitorSnapshot, WindowSnapshot, WorkspaceSnapshot},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};

pub const SOCKET_NAME: &str = "tile.sock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Run a dispatcher, e.g. `workspace 3`.
    Dispatch {
        command: String,
    },
    Query {
        what: Query,
    },
    /// Stream [`Event`]s until the client disconnects.
    Subscribe,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Query {
    Monitors,
    Workspaces,
    Windows,
    ActiveWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error { message: String },
    Monitors { monitors: Vec<MonitorSnapshot> },
    Workspaces { workspaces: Vec<WorkspaceSnapshot> },
    Windows { windows: Vec<WindowSnapshot> },
    ActiveWindow { window: Option<WindowSnapshot> },
    Event { event: Event },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Workspace {
        id: u8,
    },
    ActiveWindow {
        id: Option<isize>,
        title: Option<String>,
        exe: Option<String>,
    },
    Layout {
        layout: LayoutKind,
    },
}

/// A request received by the server, with a channel for the responses.
/// Dropping `reply` closes the client connection.
#[derive(Debug)]
pub struct Incoming {
    pub request: Request,
    pub reply: mpsc::UnboundedSender<Response>,
}

fn socket_name() -> io::Result<Name<'static>> {
    SOCKET_NAME.to_ns_name::<GenericNamespaced>()
}

/// Whether a daemon is already listening.
pub async fn is_running() -> bool {
    match socket_name() {
        Ok(name) => Stream::connect(name).await.is_ok(),
        Err(_) => false,
    }
}

/// The daemon side of the pipe. Must be created inside a Tokio runtime.
pub struct Server(Listener);

impl Server {
    /// Creates the pipe. With `user_sid`, only that user (and SYSTEM and Administrators) can
    /// connect, including from a normal, non-elevated process when Tile runs elevated.
    pub fn bind(user_sid: Option<&str>) -> io::Result<Self> {
        let mut options = ListenerOptions::new().name(socket_name()?);
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
    let mut reader = BufReader::new(&conn);
    let mut writer = &conn;
    let mut line = String::new();
    reader.read_line(&mut line).await?;
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
        let mut json = serde_json::to_string(&response).map_err(io::Error::other)?;
        json.push('\n');
        writer.write_all(json.as_bytes()).await?;
    }
    writer.flush().await
}

/// A connection to the daemon.
pub struct Client {
    reader: BufReader<Stream>,
}

impl Client {
    pub async fn send(request: &Request) -> io::Result<Self> {
        let conn = Stream::connect(socket_name()?).await?;
        let mut json = serde_json::to_string(request).map_err(io::Error::other)?;
        json.push('\n');
        (&conn).write_all(json.as_bytes()).await?;
        Ok(Self {
            reader: BufReader::new(conn),
        })
    }

    /// The next response, or `None` once the daemon closes the connection.
    pub async fn next(&mut self) -> io::Result<Option<Response>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
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
