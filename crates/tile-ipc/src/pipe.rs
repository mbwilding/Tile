//! What every pipe here shares: naming, connecting and writing one line at a time.

use std::io;

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, Name, ToNsName,
    tokio::{Listener, Stream, prelude::*},
};
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};

/// `\\.\pipe\{socket}`.
pub fn name(socket: &str) -> io::Result<Name<'_>> {
    socket.to_ns_name::<GenericNamespaced>()
}

/// Options for listening on `\\.\pipe\{socket}`.
pub fn listener(socket: &str) -> io::Result<ListenerOptions<'_>> {
    Ok(ListenerOptions::new().name(name(socket)?))
}

/// Creates the pipe `\\.\pipe\{socket}` with the default security.
pub fn listen(socket: &str) -> io::Result<Listener> {
    listener(socket)?.create_tokio()
}

/// Connects to `\\.\pipe\{socket}`.
pub async fn connect(socket: &str) -> io::Result<Stream> {
    Stream::connect(name(socket)?).await
}

/// Whether something is listening on `\\.\pipe\{socket}`. The probe connects and hangs up
/// without sending anything.
pub async fn is_listening(socket: &str) -> bool {
    connect(socket).await.is_ok()
}

/// Writes `line` and a newline.
pub async fn write_text(conn: &Stream, line: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    let mut writer = conn;
    writer.write_all(&bytes).await
}

/// Writes `value` as one JSON line.
pub async fn write_line(conn: &Stream, value: &impl Serialize) -> io::Result<()> {
    let json = serde_json::to_string(value).map_err(io::Error::other)?;
    write_text(conn, &json).await
}

/// Reads the next line, or `None` once the other end has hung up.
pub async fn read_text(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<String>> {
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}
