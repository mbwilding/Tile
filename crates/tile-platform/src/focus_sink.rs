//! A one pixel window kept off screen, which takes the focus when a monitor has nothing to focus
//! and there is no desktop window (Explorer isn't running) to put it on instead.

use std::{
    sync::atomic::{AtomicIsize, Ordering},
    thread,
};

use winsafe::{self as w, co, gui, prelude::*};

static SINK: AtomicIsize = AtomicIsize::new(0);

/// Starts the window on a thread of its own.
pub fn start_focus_sink() {
    let spawned = thread::Builder::new().name("focus sink".into()).spawn(run);
    if let Err(e) = spawned {
        tracing::warn!("couldn't start the focus sink: {e}");
    }
}

/// The window, once it exists.
pub(crate) fn focus_sink() -> Option<w::HWND> {
    match SINK.load(Ordering::Acquire) {
        0 => None,
        raw => tile_sys::hwnd(raw),
    }
}

fn run() {
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile focus",
        class_name: "tile-focus",
        class_bg_brush: gui::Brush::None,
        size: (1, 1),
        style: co::WS::POPUP,
        ex_style: co::WS_EX::TOOLWINDOW | co::WS_EX::LAYERED,
        ..Default::default()
    });
    wnd.on().wm_create({
        let wnd = wnd.clone();
        move |_| {
            let hwnd = wnd.hwnd();
            if let Err(e) = crate::set_alpha(hwnd, 0) {
                tracing::warn!("focus sink opacity: {e}");
            }
            let _ = hwnd.SetWindowPos(
                w::HwndPlace::None,
                w::POINT::with(-32000, -32000),
                w::SIZE::with(1, 1),
                co::SWP::NOACTIVATE | co::SWP::NOZORDER,
            );
            SINK.store(hwnd.ptr() as isize, Ordering::Release);
            Ok(0)
        }
    });
    if let Err(e) = wnd.run_main(Some(co::SW::SHOWNOACTIVATE)) {
        tracing::error!("focus sink message loop failed: {e}");
    }
    SINK.store(0, Ordering::Release);
}
