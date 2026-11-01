use tile_core::{MonitorId, MonitorInfo, Rect};
use winsafe::{self as w, prelude::*};

/// A Win32 rectangle as a [`Rect`].
pub const fn rect(r: w::RECT) -> Rect {
    Rect::from_ltrb(r.left, r.top, r.right, r.bottom)
}

/// All connected monitors, ordered left to right, then top to bottom.
pub fn monitors() -> Vec<MonitorInfo> {
    let mut out = Vec::new();
    let result = w::HDC::NULL.EnumDisplayMonitors(None, |hmon: w::HMONITOR, _, _| {
        match hmon.GetMonitorInfo() {
            Ok(info) => out.push(MonitorInfo {
                id: MonitorId(hmon.ptr() as isize),
                rect: rect(info.rcMonitor),
                work_area: rect(info.rcWork),
            }),
            Err(e) => tracing::warn!("GetMonitorInfo failed: {e}"),
        }
        true
    });
    if let Err(e) = result {
        tracing::error!("EnumDisplayMonitors failed: {e}");
    }
    out.sort_by_key(|m| (m.rect.x, m.rect.y));
    out
}
