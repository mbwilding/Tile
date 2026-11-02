//! The workspace indicator: a small square showing the workspace number that flashes up when
//! the workspace changes, then fades out. Like the launcher, it runs on its own thread with its
//! window made up front, and it never takes focus or clicks.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use tile_config::{IndicatorPosition, IndicatorStyle, WorkspaceIndicator as Config};
use tile_core::Rect;
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use crate::gui::{
    Mailbox, WindowThread, colorref, make_font, paint_buffered, scale, set_alpha, style_popup,
};

const FADE_TIMER: usize = 1;
const FRAME_MS: u32 = 15;

/// The `workspace_indicator` config section, with its look resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct IndicatorSettings {
    /// The section as configured.
    pub config: Config,
    /// Its look, with the theme filled in.
    pub style: IndicatorStyle,
}

enum Request {
    Show(String, Rect),
    Settings(Box<IndicatorSettings>),
    Quit,
}

/// The workspace indicator window, on its own thread.
pub struct Indicator(WindowThread<Request>);

impl Indicator {
    /// Starts the indicator thread.
    pub fn spawn(settings: IndicatorSettings) -> Self {
        Self(WindowThread::spawn(
            "indicator",
            move |requests, mailbox| {
                run(settings, requests, mailbox);
            },
        ))
    }

    fn send(&self, request: Request) {
        self.0.send(request);
    }

    /// Flashes `workspace` in `area` (a monitor's work area), unless the indicator is off.
    pub fn show(&self, workspace: u8, area: Rect) {
        self.send(Request::Show(workspace.to_string(), area));
    }

    /// As [`show`](Self::show), with any `text` (the layout's name, say) in place of the number.
    pub fn show_text(&self, text: &str, area: Rect) {
        self.send(Request::Show(text.to_owned(), area));
    }

    /// Applies new settings, hiding the indicator until it is next shown.
    pub fn set_settings(&self, settings: IndicatorSettings) {
        self.send(Request::Settings(Box::new(settings)));
    }

    /// Closes the window and stops the thread.
    pub fn shutdown(&mut self) {
        self.0.shutdown(Request::Quit);
    }
}

/// Where a `w` by `h` indicator goes in `area`.
fn position(area: Rect, w: i32, h: i32, gap: i32, at: IndicatorPosition) -> (i32, i32) {
    let (left, top) = (area.x + gap, area.y + gap);
    let (right, bottom) = (area.x + area.w - gap - w, area.y + area.h - gap - h);
    let (cx, cy) = (area.x + (area.w - w) / 2, area.y + (area.h - h) / 2);
    match at {
        IndicatorPosition::Center => (cx, cy),
        IndicatorPosition::Top => (cx, top),
        IndicatorPosition::TopLeft => (left, top),
        IndicatorPosition::TopRight => (right, top),
        IndicatorPosition::Bottom => (cx, bottom),
        IndicatorPosition::BottomLeft => (left, bottom),
        IndicatorPosition::BottomRight => (right, bottom),
        IndicatorPosition::Left => (left, cy),
        IndicatorPosition::Right => (right, cy),
    }
}

/// How opaque the indicator is `elapsed` after it was shown, from 0 to 1; `None` once gone.
fn fade(elapsed: Duration, hold: Duration, fade: Duration) -> Option<f32> {
    let Some(fading) = elapsed.checked_sub(hold) else {
        return Some(1.0);
    };
    (fading < fade).then(|| 1.0 - fading.as_secs_f32() / fade.as_secs_f32())
}

struct State {
    settings: IndicatorSettings,
    label: String,
    shown_at: Instant,
    dpi: u32,
    font: Option<DeleteObjectGuard<w::HFONT>>,
}

impl State {
    fn config(&self) -> &Config {
        &self.settings.config
    }

    fn scale(&self, px: u32) -> i32 {
        scale(px as i32, self.dpi)
    }

    fn font(&mut self) -> &w::HFONT {
        let config = &self.settings.config;
        let px = (config.font_size.clamp(6, 200) * self.dpi / 72) as i32;
        let face = &self.settings.style.font;
        self.font.get_or_insert_with(|| {
            make_font(face, px, config.bold).unwrap_or_else(|e| {
                tracing::warn!("indicator font `{face}`: {e}");
                w::HFONT::CreateFontIndirect(&w::LOGFONT::default())
                    .expect("the default font exists")
            })
        })
    }

    fn alpha(&self, amount: f32) -> u8 {
        (self.config().opacity.clamp(0.1, 1.0) * amount.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

fn style_window(hwnd: &w::HWND, settings: &IndicatorSettings) {
    style_popup(hwnd, settings.style.corners, settings.style.background);
}

fn run(settings: IndicatorSettings, requests: mpsc::Receiver<Request>, mailbox: Mailbox<Request>) {
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile workspace indicator",
        class_name: "tile-indicator",
        class_bg_brush: gui::Brush::None,
        size: (settings.config.size as i32, settings.config.size as i32),
        style: co::WS::POPUP,
        ex_style: co::WS_EX::TOOLWINDOW
            | co::WS_EX::TOPMOST
            | co::WS_EX::LAYERED
            | co::WS_EX::TRANSPARENT
            | co::WS_EX::NOACTIVATE,
        ..Default::default()
    });
    let state = Rc::new(RefCell::new(State {
        settings,
        label: String::new(),
        shown_at: Instant::now(),
        dpi: 96,
        font: None,
    }));

    wnd.on().wm_create({
        let (wnd, state, mailbox) = (wnd.clone(), state.clone(), mailbox.clone());
        move |_| {
            style_window(wnd.hwnd(), &state.borrow().settings);
            mailbox.attach(wnd.hwnd());
            Ok(0)
        }
    });

    let hide = {
        let wnd = wnd.clone();
        move || {
            let _ = wnd.hwnd().KillTimer(FADE_TIMER);
            wnd.hwnd().ShowWindow(co::SW::HIDE);
        }
    };

    let show = {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move |label: String, area: Rect| {
            let hwnd = wnd.hwnd();
            let mut s = state.borrow_mut();
            if !s.config().enabled {
                drop(s);
                hide();
                return;
            }
            let (cx, cy) = area.center();
            let dpi = tile_shell::monitor_dpi(cx, cy);
            if dpi != s.dpi {
                s.dpi = dpi;
                s.font = None;
            }
            let text_w = hwnd
                .GetDC()
                .and_then(|dc| {
                    let _font = dc.SelectObject(s.font())?;
                    dc.GetTextExtentPoint32(&label)
                })
                .map_or(0, |size| size.cx);
            let config = s.config().clone();
            let size = s.scale(config.size.clamp(16, 1000));
            let w = size.max(text_w + s.scale(32)).min(area.w);
            let h = size.min(area.h);
            let gap = s.scale(config.gap.min(1000));
            let (x, y) = position(area, w, h, gap, config.position);
            let alpha = s.alpha(1.0);
            s.label = label;
            s.shown_at = Instant::now();
            drop(s);
            if let Err(e) = set_alpha(hwnd, alpha) {
                tracing::warn!("indicator opacity: {e}");
            }
            let placed = hwnd.SetWindowPos(
                w::HwndPlace::Place(co::HWND_PLACE::TOPMOST),
                w::POINT::with(x, y),
                w::SIZE::with(w, h),
                co::SWP::SHOWWINDOW | co::SWP::NOACTIVATE,
            );
            if let Err(e) = placed {
                tracing::warn!("couldn't show the workspace indicator: {e}");
                return;
            }
            let _ = hwnd.InvalidateRect(None, false);
            if let Err(e) = hwnd.SetTimer(FADE_TIMER, FRAME_MS, None) {
                tracing::warn!("indicator fade: {e}");
            }
        }
    };

    wnd.on().wm(co::WM::APP, {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move |_| {
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Show(label, area) => show(label, area),
                    Request::Settings(settings) => {
                        let mut s = state.borrow_mut();
                        s.settings = *settings;
                        s.font = None;
                        style_window(wnd.hwnd(), &s.settings);
                        drop(s);
                        hide();
                    }
                    Request::Quit => w::PostQuitMessage(0),
                }
            }
            Ok(0)
        }
    });

    wnd.on().wm_timer(FADE_TIMER, {
        let (wnd, state) = (wnd.clone(), state.clone());
        move || {
            let s = state.borrow();
            let config = s.config();
            let amount = fade(
                s.shown_at.elapsed(),
                Duration::from_millis(config.duration_ms),
                Duration::from_millis(config.fade_ms),
            );
            let alpha = amount.map(|a| s.alpha(a));
            drop(s);
            match alpha {
                Some(alpha) => {
                    let _ = set_alpha(wnd.hwnd(), alpha);
                }
                None => hide(),
            }
            Ok(())
        }
    });

    wnd.on().wm_paint({
        let (wnd, state) = (wnd.clone(), state.clone());
        move || {
            paint_buffered(wnd.hwnd(), |dc, mut client| {
                let mut s = state.borrow_mut();
                let (background, foreground) =
                    (s.settings.style.background, s.settings.style.foreground);
                let brush = w::HBRUSH::CreateSolidBrush(colorref(background))?;
                dc.FillRect(client, &brush)?;
                dc.SetBkMode(co::BKMODE::TRANSPARENT)?;
                dc.SetTextColor(colorref(foreground))?;
                let label = s.label.clone();
                let _font = dc.SelectObject(s.font())?;
                dc.DrawText(
                    &label,
                    &mut client,
                    co::DT::CENTER | co::DT::VCENTER | co::DT::SINGLELINE | co::DT::NOPREFIX,
                )?;
                Ok(())
            })?;
            Ok(())
        }
    });

    if let Err(e) = wnd.run_main(Some(co::SW::HIDE)) {
        tracing::error!("indicator message loop failed: {e}");
    }
    mailbox.detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_against_the_edges() {
        let area = Rect::new(100, 0, 1000, 800);
        let at = |p| position(area, 100, 50, 20, p);
        assert_eq!(at(IndicatorPosition::Center), (550, 375));
        assert_eq!(at(IndicatorPosition::TopLeft), (120, 20));
        assert_eq!(at(IndicatorPosition::Top), (550, 20));
        assert_eq!(at(IndicatorPosition::BottomRight), (980, 730));
        assert_eq!(at(IndicatorPosition::Left), (120, 375));
    }

    #[test]
    fn holds_then_fades() {
        let (hold, out) = (Duration::from_millis(400), Duration::from_millis(200));
        assert_eq!(fade(Duration::from_millis(100), hold, out), Some(1.0));
        assert_eq!(fade(Duration::from_millis(500), hold, out), Some(0.5));
        assert_eq!(fade(Duration::from_millis(600), hold, out), None);
        assert_eq!(fade(Duration::from_millis(400), hold, Duration::ZERO), None);
    }
}
