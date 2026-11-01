//! What apps send the taskbar, byte by byte: `Shell_NotifyIcon`'s icon data, app bar messages
//! and `Shell_NotifyIconGetRect`'s question, and how the answers are packed.

use windows_sys::Win32::Foundation::LRESULT;

/// The `dwData` of a `WM_COPYDATA` that carries a tray icon message.
pub(super) const COPYDATA_NOTIFY: usize = 1;
/// The `dwData` of an app bar message (`SHAppBarMessage`): apps asking about the taskbar.
pub(super) const COPYDATA_APPBAR: usize = 0;
/// The `dwData` of the question `Shell_NotifyIconGetRect` asks: where is this icon?
pub(super) const COPYDATA_RECT: usize = 3;
/// The question is asked twice: once for the icon's top left corner, once for its size.
pub(super) const RECT_POSITION: u32 = 1;
pub(super) const RECT_SIZE: u32 = 2;
/// The first field of what `Shell_NotifyIcon` sends.
pub(super) const MAGIC: u32 = 0x3475_3423;

pub(super) const NIM_ADD: u32 = 0;
pub(super) const NIM_MODIFY: u32 = 1;
pub(super) const NIM_DELETE: u32 = 2;
pub(super) const NIM_SETFOCUS: u32 = 3;
pub(super) const NIM_SETVERSION: u32 = 4;

pub(super) const NIF_MESSAGE: u32 = 0x01;
pub(super) const NIF_ICON: u32 = 0x02;
pub(super) const NIF_TIP: u32 = 0x04;
pub(super) const NIF_STATE: u32 = 0x08;
pub(super) const NIF_GUID: u32 = 0x20;

pub(super) const NIS_HIDDEN: u32 = 0x01;

pub(super) const ABM_NEW: u32 = 0;
pub(super) const ABM_REMOVE: u32 = 1;
pub(super) const ABM_QUERYPOS: u32 = 2;
pub(super) const ABM_SETPOS: u32 = 3;
pub(super) const ABM_GETSTATE: u32 = 4;
pub(super) const ABM_GETTASKBARPOS: u32 = 5;
pub(super) const ABM_ACTIVATE: u32 = 6;
pub(super) const ABM_GETAUTOHIDEBAR: u32 = 7;
pub(super) const ABM_SETAUTOHIDEBAR: u32 = 8;
pub(super) const ABM_WINDOWPOSCHANGED: u32 = 9;
pub(super) const ABM_SETSTATE: u32 = 10;
/// `ABE_BOTTOM`.
pub(super) const EDGE_BOTTOM: u32 = 3;
/// Where in the sender's block the shell writes the edge, then the rectangle.
pub(super) const ABM_EDGE_AT: u64 = 28;

/// A decoded `NOTIFYICONDATA`, as `Shell_NotifyIcon` sends it (handles are 32 bits wide).
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Nid {
    pub(super) hwnd: isize,
    pub(super) id: u32,
    pub(super) flags: u32,
    pub(super) callback: u32,
    pub(super) icon: isize,
    pub(super) tip: String,
    pub(super) state: u32,
    pub(super) state_mask: u32,
    pub(super) version: u32,
    pub(super) guid: [u8; 16],
}

/// Offsets into the `COPYDATASTRUCT`'s data: an 8 byte header, then the `NOTIFYICONDATAW`.
mod at {
    pub const MESSAGE: usize = 4;
    pub const NID: usize = 8;
    pub const HWND: usize = NID + 4;
    pub const ID: usize = NID + 8;
    pub const FLAGS: usize = NID + 12;
    pub const CALLBACK: usize = NID + 16;
    pub const ICON: usize = NID + 20;
    pub const TIP: usize = NID + 24;
    pub const TIP_CHARS: usize = 128;
    pub const STATE: usize = TIP + TIP_CHARS * 2;
    pub const STATE_MASK: usize = STATE + 4;
    pub const INFO: usize = STATE_MASK + 4;
    pub const VERSION: usize = INFO + 256 * 2;
    pub const GUID: usize = VERSION + 4 + 64 * 2 + 4;
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// A 32 bit handle as sent, sign-extended.
fn handle(value: u32) -> isize {
    i64::from(value as i32) as isize
}

/// Decodes the data of a tray `WM_COPYDATA`: the message (`NIM_*`) and the icon data. Fields
/// past the end of a short (older) structure stay empty.
pub(super) fn parse(data: &[u8]) -> Option<(u32, Nid)> {
    if u32_at(data, 0)? != MAGIC {
        return None;
    }
    let message = u32_at(data, at::MESSAGE)?;
    let mut nid = Nid {
        hwnd: handle(u32_at(data, at::HWND)?),
        id: u32_at(data, at::ID)?,
        flags: u32_at(data, at::FLAGS).unwrap_or(0),
        callback: u32_at(data, at::CALLBACK).unwrap_or(0),
        icon: u32_at(data, at::ICON).map_or(0, handle),
        ..Nid::default()
    };
    if let Some(bytes) = data.get(at::TIP..at::TIP + at::TIP_CHARS * 2) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .take_while(|&u| u != 0)
            .collect();
        nid.tip = String::from_utf16_lossy(&units);
    }
    nid.state = u32_at(data, at::STATE).unwrap_or(0);
    nid.state_mask = u32_at(data, at::STATE_MASK).unwrap_or(0);
    nid.version = u32_at(data, at::VERSION).unwrap_or(0);
    if let Some(guid) = data.get(at::GUID..at::GUID + 16) {
        nid.guid.copy_from_slice(guid);
    }
    Some((message, nid))
}

/// The first bytes of `data` as hex, for the log.
pub(super) fn hex(data: &[u8]) -> String {
    data.iter()
        .take(48)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A decoded app bar message.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AppBarRequest {
    /// `ABM_*`.
    pub(super) message: u32,
    /// An address in the sender's process of the block the answer goes into.
    pub(super) shared: u64,
    pub(super) pid: u32,
}

/// Decodes the data of a `dwData` 0 `WM_COPYDATA`: the `APPBARDATA`, then the message, and where
/// in which process the answer is written.
pub(super) fn parse_appbar(data: &[u8]) -> Option<AppBarRequest> {
    Some(AppBarRequest {
        message: u32_at(data, 40)?,
        shared: u64::from_le_bytes(data.get(48..56)?.try_into().ok()?),
        pid: u32_at(data, 56)?,
    })
}

/// A decoded `Shell_NotifyIconGetRect` question.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RectRequest {
    /// [`RECT_POSITION`] or [`RECT_SIZE`].
    pub(super) what: u32,
    pub(super) hwnd: isize,
    pub(super) id: u32,
    pub(super) guid: Option<[u8; 16]>,
}

/// Decodes the data of a `dwData` 3 `WM_COPYDATA`: the magic and which half is asked for, then
/// the identifier of the icon (`NOTIFYICONIDENTIFIER` with 32 bit handles).
pub(super) fn parse_rect_request(data: &[u8]) -> Option<RectRequest> {
    if u32_at(data, 0)? != MAGIC {
        return None;
    }
    let guid: [u8; 16] = data.get(24..40)?.try_into().ok()?;
    Some(RectRequest {
        what: u32_at(data, 4)?,
        hwnd: handle(u32_at(data, 16)?),
        id: u32_at(data, 20)?,
        guid: (guid != [0; 16]).then_some(guid),
    })
}

/// Two screen values packed the way the shell answers: the first in the low 16 bits, the second
/// in the next 16, each signed.
pub(super) fn pack_pair(a: i32, b: i32) -> LRESULT {
    let word = |v: i32| u32::from(v as i16 as u16);
    (word(a) | (word(b) << 16)) as i32 as LRESULT
}

/// Test messages built the way apps build them.
#[cfg(test)]
pub(super) mod build {
    use super::{MAGIC, at};

    /// What `Shell_NotifyIcon` sends for one icon.
    pub(in super::super) fn message(
        kind: u32,
        hwnd: i32,
        id: u32,
        flags: u32,
        callback: u32,
        tip: &str,
    ) -> Vec<u8> {
        let mut data = vec![0u8; at::GUID + 16 + 4];
        let mut put =
            |at: usize, value: u32| data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, MAGIC);
        put(at::MESSAGE, kind);
        put(at::HWND, hwnd as u32);
        put(at::ID, id);
        put(at::FLAGS, flags);
        put(at::CALLBACK, callback);
        for (i, unit) in tip.encode_utf16().enumerate() {
            let at = at::TIP + i * 2;
            data[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        data
    }

    /// What `Shell_NotifyIconGetRect` sends about the icon `id` of the window `hwnd`.
    pub(in super::super) fn rect_question(what: u32, hwnd: i32, id: u32) -> Vec<u8> {
        let mut data = vec![0u8; 40];
        let mut put =
            |at: usize, value: u32| data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, MAGIC);
        put(4, what);
        put(8, 32);
        put(16, hwnd as u32);
        put(20, id);
        data
    }

    /// What `SHAppBarMessage` sends: an `APPBARDATA`, then the message and where the answer goes.
    pub(in super::super) fn appbar_message(message: u32, shared: u64, pid: u32) -> Vec<u8> {
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&40u32.to_le_bytes());
        data[40..44].copy_from_slice(&message.to_le_bytes());
        data[48..56].copy_from_slice(&shared.to_le_bytes());
        data[56..60].copy_from_slice(&pid.to_le_bytes());
        data
    }
}

#[cfg(test)]
mod tests {
    use super::{build::*, *};

    #[test]
    fn parses_what_shell_notifyicon_sends() {
        let data = message(NIM_ADD, -5, 7, NIF_MESSAGE | NIF_TIP, 0x8001, "Discord");
        let (kind, nid) = parse(&data).unwrap();
        assert_eq!(kind, NIM_ADD);
        assert_eq!((nid.hwnd, nid.id, nid.callback), (-5, 7, 0x8001));
        assert_eq!(nid.tip, "Discord");
    }

    #[test]
    fn rejects_other_data() {
        assert_eq!(parse(&[0; 4]), None);
        let mut data = message(NIM_ADD, 1, 1, 0, 0, "");
        data[0] ^= 0xFF;
        assert_eq!(parse(&data), None);
    }

    #[test]
    fn short_structures_leave_later_fields_empty() {
        let mut data = message(NIM_MODIFY, 1, 2, NIF_TIP, 0, "x");
        data.truncate(at::TIP);
        let (_, nid) = parse(&data).unwrap();
        assert_eq!((nid.hwnd, nid.id, nid.tip.as_str()), (1, 2, ""));
    }

    #[test]
    fn parses_the_app_bar_message() {
        let data = appbar_message(ABM_GETTASKBARPOS, 0x1e4_b450_fe80, 41852);
        assert_eq!(
            parse_appbar(&data),
            Some(AppBarRequest {
                message: ABM_GETTASKBARPOS,
                shared: 0x1e4_b450_fe80,
                pid: 41852,
            })
        );
        assert_eq!(parse_appbar(&data[..50]), None);
    }

    #[test]
    fn parses_the_position_question() {
        let data = rect_question(RECT_SIZE, -9, 4);
        assert_eq!(
            parse_rect_request(&data),
            Some(RectRequest {
                what: RECT_SIZE,
                hwnd: -9,
                id: 4,
                guid: None,
            })
        );
        assert_eq!(parse_rect_request(&data[..20]), None);
    }

    #[test]
    fn packs_pairs_the_way_the_shell_answers() {
        assert_eq!(pack_pair(300, 500), 300 | (500 << 16));
        // Signed, for monitors left of or above the primary one.
        assert_eq!(pack_pair(-1, -2) & 0xFFFF, 0xFFFF);
        assert_eq!((pack_pair(-1, -2) >> 16) & 0xFFFF, 0xFFFE);
    }
}
