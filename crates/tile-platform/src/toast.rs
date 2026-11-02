//! Popups for Windows toast notifications: a small window, bottom-right by default, showing the
//! app, title and body of each one in turn, then fading out. Like the workspace indicator, it
//! runs on its own thread with its window made up front, never takes focus, and a click
//! dismisses it early.

use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use tile_config::{IndicatorPosition, NotificationStyle, Notifications as Config};
use tile_core::Rect;
use tile_sys::notifications::Notification;
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use crate::gui::{
    Mailbox, WindowThread, colorref, make_font, paint_buffered, scale, set_alpha, style_popup,
};

const FADE_TIMER: usize = 1;
const FRAME_MS: u32 = 15;
/// Lines of body text shown before the rest is cut off with an ellipsis.
const BODY_LINES: i32 = 3;

/// The `notifications` config section, with its look resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastSettings {
    /// The section as configured.
    pub config: Config,
    /// Its look, with the theme filled in.
    pub style: NotificationStyle,
}

enum Request {
    Show(Notification, Rect),
    Settings(Box<ToastSettings>),
    Quit,
}

/// The notification popup window, on its own thread.
pub struct Toast(WindowThread<Request>);

impl Toast {
    /// Starts the toast thread.
    pub fn spawn(settings: ToastSettings) -> Self {
        Self(WindowThread::spawn("toast", move |requests, mailbox| {
            run(settings, requests, mailbox);
        }))
    }

    fn send(&self, request: Request) {
        self.0.send(request);
    }

    /// Queues `notification` to show in `area` (a monitor's work area), unless popups are off.
    pub fn show(&self, notification: Notification, area: Rect) {
        self.send(Request::Show(notification, area));
    }

    /// Applies new settings. Already-queued notifications keep showing.
    pub fn set_settings(&self, settings: ToastSettings) {
        self.send(Request::Settings(Box::new(settings)));
    }

    /// Closes the window and stops the thread.
    pub fn shutdown(&mut self) {
        self.0.shutdown(Request::Quit);
    }
}

/// Where a `w` by `h` popup goes in `area`.
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

/// How opaque the popup is `elapsed` after it was shown, from 0 to 1; `None` once gone.
fn fade(elapsed: Duration, hold: Duration, fade: Duration) -> Option<f32> {
    let Some(fading) = elapsed.checked_sub(hold) else {
        return Some(1.0);
    };
    (fading < fade).then(|| 1.0 - fading.as_secs_f32() / fade.as_secs_f32())
}

struct Shown {
    notification: Notification,
    area: Rect,
    shown_at: Instant,
}

struct State {
    settings: ToastSettings,
    queue: VecDeque<Shown>,
    dpi: u32,
    font: Option<DeleteObjectGuard<w::HFONT>>,
    small_font: Option<DeleteObjectGuard<w::HFONT>>,
}

impl State {
    fn config(&self) -> &Config {
        &self.settings.config
    }

    fn scale(&self, px: u32) -> i32 {
        scale(px as i32, self.dpi)
    }

    fn font(&mut self, px: i32, bold: bool) -> &w::HFONT {
        let slot = if bold {
            &mut self.font
        } else {
            &mut self.small_font
        };
        if slot.is_none() {
            let face = self.settings.style.font.clone();
            *slot = Some(make_font(&face, px, bold).unwrap_or_else(|e| {
                tracing::warn!("toast font `{face}`: {e}");
                w::HFONT::CreateFontIndirect(&w::LOGFONT::default())
                    .expect("the default font exists")
            }));
        }
        slot.as_ref().expect("the font was just made")
    }
}

fn style_window(hwnd: &w::HWND, settings: &ToastSettings) {
    style_popup(hwnd, settings.style.corners, settings.style.background);
}

/// The popup's height at `dpi`: room for the app name, the title and up to [`BODY_LINES`]
/// lines of body text. Fixed rather than measured, so a long body is clipped with an ellipsis
/// instead of growing the window.
fn height(dpi: u32) -> i32 {
    let line = scale(20, dpi);
    let padding = scale(16, dpi);
    padding * 2 + line * (2 + BODY_LINES)
}

fn run(settings: ToastSettings, requests: mpsc::Receiver<Request>, mailbox: Mailbox<Request>) {
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile notification",
        class_name: "tile-toast",
        class_bg_brush: gui::Brush::None,
        style: co::WS::POPUP,
        ex_style: co::WS_EX::TOOLWINDOW
            | co::WS_EX::TOPMOST
            | co::WS_EX::LAYERED
            | co::WS_EX::NOACTIVATE,
        ..Default::default()
    });
    let state = Rc::new(RefCell::new(State {
        settings,
        queue: VecDeque::new(),
        dpi: 96,
        font: None,
        small_font: None,
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

    let present = {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move || {
            let hwnd = wnd.hwnd();
            let mut s = state.borrow_mut();
            let Some(shown) = s.queue.front() else {
                drop(s);
                hide();
                return;
            };
            let area = shown.area;
            let (cx, cy) = area.center();
            let dpi = tile_shell::monitor_dpi(cx, cy);
            if dpi != s.dpi {
                s.dpi = dpi;
                s.font = None;
                s.small_font = None;
            }
            let config = *s.config();
            let w = s.scale(config.width.clamp(120, 2000)).min(area.w);
            let h = height(s.dpi).min(area.h);
            let gap = s.scale(config.gap.min(1000));
            let (x, y) = position(area, w, h, gap, config.position);
            s.queue.front_mut().expect("just checked").shown_at = Instant::now();
            drop(s);
            if let Err(e) = set_alpha(hwnd, 255) {
                tracing::warn!("toast opacity: {e}");
            }
            let placed = hwnd.SetWindowPos(
                w::HwndPlace::Place(co::HWND_PLACE::TOPMOST),
                w::POINT::with(x, y),
                w::SIZE::with(w, h),
                co::SWP::SHOWWINDOW | co::SWP::NOACTIVATE,
            );
            if let Err(e) = placed {
                tracing::warn!("couldn't show a notification popup: {e}");
                return;
            }
            let _ = hwnd.InvalidateRect(None, false);
            if let Err(e) = hwnd.SetTimer(FADE_TIMER, FRAME_MS, None) {
                tracing::warn!("toast fade: {e}");
            }
        }
    };

    let advance = {
        let (state, present) = (state.clone(), present.clone());
        move || {
            state.borrow_mut().queue.pop_front();
            present();
        }
    };

    wnd.on().wm(co::WM::APP, {
        let (wnd, state, present) = (wnd.clone(), state.clone(), present.clone());
        move |_| {
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Show(notification, area) => {
                        let mut s = state.borrow_mut();
                        if !s.config().enabled {
                            continue;
                        }
                        let was_empty = s.queue.is_empty();
                        s.queue.push_back(Shown {
                            notification,
                            area,
                            shown_at: Instant::now(),
                        });
                        drop(s);
                        if was_empty {
                            present();
                        }
                    }
                    Request::Settings(settings) => {
                        let mut s = state.borrow_mut();
                        s.settings = *settings;
                        s.font = None;
                        s.small_font = None;
                        style_window(wnd.hwnd(), &s.settings);
                        drop(s);
                    }
                    Request::Quit => w::PostQuitMessage(0),
                }
            }
            Ok(0)
        }
    });

    wnd.on().wm_timer(FADE_TIMER, {
        let (wnd, state, advance) = (wnd.clone(), state.clone(), advance.clone());
        move || {
            let s = state.borrow();
            let config = s.config();
            let shown_at = s.queue.front().map(|s| s.shown_at);
            let amount = shown_at.and_then(|at| {
                fade(
                    at.elapsed(),
                    Duration::from_millis(config.duration_ms),
                    Duration::from_millis(config.fade_ms),
                )
            });
            drop(s);
            match amount {
                Some(amount) => {
                    let alpha = (amount.clamp(0.0, 1.0) * 255.0).round() as u8;
                    let _ = set_alpha(wnd.hwnd(), alpha);
                }
                None => advance(),
            }
            Ok(())
        }
    });

    wnd.on().wm_l_button_down({
        let advance = advance.clone();
        move |_| {
            advance();
            Ok(())
        }
    });

    wnd.on().wm_paint({
        let (wnd, state) = (wnd.clone(), state.clone());
        move || {
            paint_buffered(wnd.hwnd(), |dc, client| {
                let mut s = state.borrow_mut();
                let Some(shown) = s.queue.front() else {
                    return Ok(());
                };
                let (background, foreground, dimmed) = (
                    s.settings.style.background,
                    s.settings.style.foreground,
                    s.settings.style.dimmed,
                );
                let brush = w::HBRUSH::CreateSolidBrush(colorref(background))?;
                dc.FillRect(client, &brush)?;
                dc.SetBkMode(co::BKMODE::TRANSPARENT)?;
                let padding = s.scale(16);
                let line = s.scale(20);
                let (app, title, body) = (
                    shown.notification.app.clone(),
                    shown.notification.title.clone(),
                    shown.notification.body.clone(),
                );
                let title_px = s.scale(14 * 72 / 96);
                let small_px = s.scale(12 * 72 / 96);

                let mut row = w::RECT {
                    left: client.left + padding,
                    top: client.top + padding,
                    right: client.right - padding,
                    bottom: client.top + padding + line,
                };
                dc.SetTextColor(colorref(dimmed))?;
                let _font = dc.SelectObject(s.font(small_px, false))?;
                dc.DrawText(
                    &app,
                    &mut row,
                    co::DT::LEFT | co::DT::SINGLELINE | co::DT::END_ELLIPSIS,
                )?;

                row.top += line;
                row.bottom += line;
                dc.SetTextColor(colorref(foreground))?;
                let _font = dc.SelectObject(s.font(title_px, true))?;
                dc.DrawText(
                    &title,
                    &mut row,
                    co::DT::LEFT | co::DT::SINGLELINE | co::DT::END_ELLIPSIS,
                )?;

                let mut body_rect = w::RECT {
                    left: client.left + padding,
                    top: row.bottom,
                    right: client.right - padding,
                    bottom: client.bottom - padding,
                };
                dc.SetTextColor(colorref(dimmed))?;
                let _font = dc.SelectObject(s.font(small_px, false))?;
                dc.DrawText(
                    &body,
                    &mut body_rect,
                    co::DT::LEFT | co::DT::WORDBREAK | co::DT::END_ELLIPSIS,
                )?;
                Ok(())
            })?;
            Ok(())
        }
    });

    if let Err(e) = wnd.run_main(Some(co::SW::HIDE)) {
        tracing::error!("toast message loop failed: {e}");
    }
    mailbox.detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_against_the_edges() {
        let area = Rect::new(100, 0, 1000, 800);
        let at = |p| position(area, 300, 80, 16, p);
        assert_eq!(at(IndicatorPosition::BottomRight), (784, 704));
        assert_eq!(at(IndicatorPosition::TopLeft), (116, 16));
    }

    #[test]
    fn holds_then_fades() {
        let (hold, out) = (Duration::from_millis(5000), Duration::from_millis(400));
        assert_eq!(fade(Duration::from_millis(100), hold, out), Some(1.0));
        assert_eq!(fade(Duration::from_millis(5200), hold, out), Some(0.5));
        assert_eq!(fade(Duration::from_millis(5500), hold, out), None);
    }
}
