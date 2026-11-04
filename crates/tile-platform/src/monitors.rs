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

/// The hardware path of every monitor, which names its model and port; see
/// `tile_config::monitor_key`. Monitors Windows can't name are left out.
pub fn monitor_paths() -> Vec<(MonitorId, String)> {
    let mut out = Vec::new();
    let _ = w::HDC::NULL.EnumDisplayMonitors(None, |hmon: w::HMONITOR, _, _| {
        if let Ok(info) = hmon.GetMonitorInfo()
            && let Some(path) = tile_sys::monitor_interface(&info.szDevice())
        {
            out.push((MonitorId(hmon.ptr() as isize), path));
        }
        true
    });
    out
}
