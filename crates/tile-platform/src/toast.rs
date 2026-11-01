//! Popups for Windows toast notifications, styled like the launcher's cards: a rounded,
//! bordered panel per notification, up to `notifications.max_visible` stacked at once, the rest
//! queued. Each popup counts down with a thin progress bar, pausing while the pointer hovers it;
//! a click dismisses it early. Like the launcher, it runs on its own thread with its window made
//! up front, and never takes focus.

use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use tile_config::{Color, IndicatorPosition, NotificationStyle, Notifications as Config};
use tile_core::Rect;
use tile_sys::notifications::Notification;
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use crate::gui::{
    Mailbox, WindowThread, colorref, fill_round_bordered, make_font, paint_buffered, scale,
    set_transparent, style_popup, text, text_wrapped,
};

const FADE_TIMER: usize = 1;
const FRAME_MS: u32 = 15;
/// Lines of body text shown before the rest is cut off with an ellipsis.
const BODY_LINES: i32 = 3;
/// Space between stacked cards, at 100% scaling. Separate from `notifications.gap`, which is
/// the space to the work area's edge.
const CARD_GAP: u32 = 8;
/// The countdown bar's height, and the gap above it, at 100% scaling.
const BAR_HEIGHT: u32 = 3;
const BAR_GAP: u32 = 10;
/// A colour vanishingly unlikely to be a real theme colour, used as this window's transparent
/// colour key so the gaps between stacked cards show whatever is behind them.
const TRANSPARENT_KEY: Color = Color::rgb(1, 2, 3);

/// The `notifications` config section, with its look resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastSettings {
    /// Whether a popup should be shown at all right now. This is the caller's resolved decision,
    /// not `config.enabled` directly: that setting can be unset, deferring to whether the shell
    /// is hosted, which this module has no way to know.
    pub enabled: bool,
    /// The section as configured.
    pub config: Config,
    /// Its look, with the theme filled in.
    pub style: NotificationStyle,
}

enum Request {
    /// A notification to show, the monitor work area to show it in, and a duration override from
    /// `notifications.filter`, if any matched.
    Show(Notification, Rect, Option<u64>),
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
    /// `duration_override` replaces `notifications.duration_ms` for this one popup.
    pub fn show(&self, notification: Notification, area: Rect, duration_override: Option<u64>) {
        self.send(Request::Show(notification, area, duration_override));
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

/// Where a `w` by `h` stack goes in `area`.
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

/// How tall a card is shown, from 1 (full) to 0 (gone); `None` once its time is fully up. Several
/// cards share one window and one layered alpha, so this shrinks a card away instead of fading
/// its own alpha.
fn fade(elapsed: Duration, hold: Duration, fade: Duration) -> Option<f32> {
    let Some(fading) = elapsed.checked_sub(hold) else {
        return Some(1.0);
    };
    (fading < fade).then(|| 1.0 - fading.as_secs_f32() / fade.as_secs_f32())
}

/// A notification waiting its turn, not shown yet.
struct Pending {
    notification: Notification,
    area: Rect,
    duration_override: Option<u64>,
}

/// A notification currently shown, with its own countdown.
struct Card {
    notification: Notification,
    duration_override: Option<u64>,
    shown_at: Instant,
    /// Set while the pointer is over this card, freezing its countdown.
    hover_started: Option<Instant>,
    /// Its bounds within the window, as last laid out.
    row: w::RECT,
}

impl Card {
    fn new(pending: Pending) -> Self {
        Self {
            notification: pending.notification,
            duration_override: pending.duration_override,
            shown_at: Instant::now(),
            hover_started: None,
            row: w::RECT::default(),
        }
    }

    fn hold(&self, config: &Config) -> Duration {
        Duration::from_millis(self.duration_override.unwrap_or(config.duration_ms))
    }

    /// Time since shown, frozen at the moment hovering began while the pointer is still over it.
    fn elapsed(&self) -> Duration {
        match self.hover_started {
            Some(h) => h.duration_since(self.shown_at),
            None => self.shown_at.elapsed(),
        }
    }

    fn amount(&self, config: &Config) -> Option<f32> {
        fade(
            self.elapsed(),
            self.hold(config),
            Duration::from_millis(config.fade_ms),
        )
    }

    /// Fraction of its hold time left, for the countdown bar; `None` once it is shrinking away
    /// (the bar has nothing more useful to show by then) or it never had a hold time.
    fn remaining(&self, config: &Config) -> Option<f32> {
        let hold = self.hold(config);
        if hold.is_zero() {
            return None;
        }
        let elapsed = self.elapsed();
        (elapsed < hold).then(|| 1.0 - elapsed.as_secs_f32() / hold.as_secs_f32())
    }

    /// Pauses the countdown while `hovering`, resuming from where it left off once it isn't.
    fn set_hover(&mut self, hovering: bool) {
        match (hovering, self.hover_started) {
            (true, None) => self.hover_started = Some(Instant::now()),
            (false, Some(since)) => {
                self.shown_at += since.elapsed();
                self.hover_started = None;
            }
            _ => {}
        }
    }
}

struct State {
    settings: ToastSettings,
    queue: VecDeque<Pending>,
    visible: Vec<Card>,
    /// The monitor area the visible cards are shown against, set from whichever notification
    /// started the stack.
    area: Rect,
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

    /// A card's full height, before any shrinking: room for the app name (unless switched off),
    /// the title, up to [`BODY_LINES`] lines of body text, and the countdown bar.
    fn card_height(&self) -> i32 {
        let line = self.scale(20);
        let padding = self.scale(16);
        let rows = i32::from(self.config().show_app_name) + 1 + BODY_LINES;
        let bar = self.scale(BAR_GAP + BAR_HEIGHT);
        padding * 2 + line * rows + bar
    }

    /// Moves cards from the queue into `visible`, up to `max_visible`. The first one promoted
    /// into an empty stack sets which monitor the whole stack shows on.
    fn refill(&mut self) {
        let max_visible = self.config().max_visible.clamp(1, 8) as usize;
        while self.visible.len() < max_visible {
            let Some(pending) = self.queue.pop_front() else {
                break;
            };
            if self.visible.is_empty() {
                self.area = pending.area;
            }
            self.visible.push(Card::new(pending));
        }
    }

    /// Drops cards whose time is fully up, then tops back up from the queue.
    fn expire(&mut self) {
        let config = self.config().clone();
        self.visible.retain(|c| c.amount(&config).is_some());
        self.refill();
    }

    /// Lays out the visible cards top to bottom in a window `w` pixels wide, caching each one's
    /// bounds, and returns the stack's total height.
    fn layout(&mut self, w: i32) -> i32 {
        let card_h = self.card_height();
        let gap = self.scale(CARD_GAP);
        let config = self.config().clone();
        let mut y = 0;
        for card in &mut self.visible {
            let amount = card.amount(&config).unwrap_or(1.0).clamp(0.0, 1.0);
            let h = (card_h as f32 * amount).round() as i32;
            card.row = w::RECT {
                left: 0,
                top: y,
                right: w,
                bottom: y + h,
            };
            y += h + gap;
        }
        (y - gap).max(0)
    }

    /// Whichever card's row, in window-local coordinates, `(x, y)` falls in.
    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.visible.iter().position(|c| {
            (c.row.left..c.row.right).contains(&x) && (c.row.top..c.row.bottom).contains(&y)
        })
    }
}

fn style_window(hwnd: &w::HWND, settings: &ToastSettings) {
    style_popup(hwnd, settings.style.corners, settings.style.border);
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
        visible: Vec::new(),
        area: Rect::default(),
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

    // Resizes and repositions the window to fit what's visible, or hides it if nothing is.
    let relayout = {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move || {
            let hwnd = wnd.hwnd();
            let mut s = state.borrow_mut();
            if s.visible.is_empty() {
                drop(s);
                hide();
                return;
            }
            let area = s.area;
            let (cx, cy) = area.center();
            let dpi = tile_shell::monitor_dpi(cx, cy);
            if dpi != s.dpi {
                s.dpi = dpi;
                s.font = None;
                s.small_font = None;
            }
            let width = s.config().width.clamp(120, 2000);
            let w = s.scale(width).min(area.w);
            let total_h = s.layout(w);
            let h = total_h.min(area.h);
            let gap = s.scale(s.config().gap.min(1000));
            let position_at = s.config().position;
            let (x, y) = position(area, w, h, gap, position_at);
            drop(s);
            if let Err(e) = set_transparent(hwnd, TRANSPARENT_KEY, 255) {
                tracing::warn!("toast transparency: {e}");
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
                tracing::warn!("toast timer: {e}");
            }
        }
    };

    // Checks the pointer against every visible card's row, for hover-to-pause.
    let update_hover = {
        let (wnd, state) = (wnd.clone(), state.clone());
        move || {
            let hwnd = wnd.hwnd();
            let Ok(window_rect) = hwnd.GetWindowRect() else {
                return;
            };
            let cursor = w::GetCursorPos().unwrap_or_default();
            let (local_x, local_y) = (cursor.x - window_rect.left, cursor.y - window_rect.top);
            let mut s = state.borrow_mut();
            let hit = s.hit(local_x, local_y);
            for (i, card) in s.visible.iter_mut().enumerate() {
                card.set_hover(Some(i) == hit);
            }
        }
    };

    wnd.on().wm(co::WM::APP, {
        let (state, relayout) = (state.clone(), relayout.clone());
        move |_| {
            let mut changed = false;
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Show(notification, area, duration_override) => {
                        let mut s = state.borrow_mut();
                        if !s.settings.enabled {
                            continue;
                        }
                        s.queue.push_back(Pending {
                            notification,
                            area,
                            duration_override,
                        });
                        s.refill();
                        changed = true;
                    }
                    Request::Settings(settings) => {
                        let mut s = state.borrow_mut();
                        s.settings = *settings;
                        s.font = None;
                        s.small_font = None;
                        changed = true;
                    }
                    Request::Quit => w::PostQuitMessage(0),
                }
            }
            if changed {
                relayout();
            }
            Ok(0)
        }
    });

    wnd.on().wm_timer(FADE_TIMER, {
        let (state, update_hover, relayout) =
            (state.clone(), update_hover.clone(), relayout.clone());
        move || {
            update_hover();
            state.borrow_mut().expire();
            relayout();
            Ok(())
        }
    });

    wnd.on().wm_l_button_down({
        let (state, relayout) = (state.clone(), relayout.clone());
        move |p| {
            let mut s = state.borrow_mut();
            if let Some(i) = s.hit(p.coords.x, p.coords.y) {
                s.visible.remove(i);
                s.refill();
                drop(s);
                relayout();
            }
            Ok(())
        }
    });

    wnd.on().wm_paint({
        let (wnd, state) = (wnd.clone(), state.clone());
        move || {
            paint_buffered(wnd.hwnd(), |dc, client| {
                let mut s = state.borrow_mut();
                let key_brush = w::HBRUSH::CreateSolidBrush(colorref(TRANSPARENT_KEY))?;
                dc.FillRect(client, &key_brush)?;
                dc.SetBkMode(co::BKMODE::TRANSPARENT)?;
                let style = s.settings.style.clone();
                let show_app_name = s.config().show_app_name;
                let border_width = s.scale(s.config().border_width);
                let radius = s.scale(12);
                let padding = s.scale(16);
                let line = s.scale(20);
                let title_px = s.scale(14 * 72 / 96);
                let small_px = s.scale(12 * 72 / 96);
                let bar_h = s.scale(BAR_HEIGHT);
                let bar_gap = s.scale(BAR_GAP);
                let rows: Vec<(w::RECT, String, String, String, Option<f32>)> = s
                    .visible
                    .iter()
                    .map(|c| {
                        (
                            c.row,
                            c.notification.app.clone(),
                            c.notification.title.clone(),
                            c.notification.body.clone(),
                            c.remaining(s.config()),
                        )
                    })
                    .collect();
                for (row, app, title, body, remaining) in rows {
                    if row.bottom <= row.top {
                        continue;
                    }
                    fill_round_bordered(
                        dc,
                        row,
                        radius,
                        style.background,
                        style.border,
                        border_width,
                    )?;
                    let mut cursor_y = row.top + padding;
                    let text_bounds = |top: i32, bottom: i32| w::RECT {
                        left: row.left + padding,
                        top,
                        right: row.right - padding,
                        bottom,
                    };
                    if show_app_name {
                        text(
                            dc,
                            s.font(small_px, false),
                            style.dimmed,
                            &app,
                            text_bounds(cursor_y, cursor_y + line),
                            co::DT::LEFT,
                        )?;
                        cursor_y += line;
                    }
                    text(
                        dc,
                        s.font(title_px, true),
                        style.foreground,
                        &title,
                        text_bounds(cursor_y, cursor_y + line),
                        co::DT::LEFT,
                    )?;
                    cursor_y += line;
                    let body_bottom = row.bottom - padding - bar_gap - bar_h;
                    text_wrapped(
                        dc,
                        s.font(small_px, false),
                        style.dimmed,
                        &body,
                        text_bounds(cursor_y, body_bottom),
                        co::DT::LEFT,
                    )?;
                    if let Some(remaining) = remaining {
                        let track = w::RECT {
                            left: row.left + padding,
                            top: row.bottom - padding - bar_h,
                            right: row.right - padding,
                            bottom: row.bottom - padding,
                        };
                        fill_round_bordered(dc, track, bar_h / 2, style.dimmed, style.dimmed, 0)?;
                        let filled_w = ((track.right - track.left) as f32
                            * remaining.clamp(0.0, 1.0))
                        .round() as i32;
                        if filled_w > 0 {
                            let filled = w::RECT {
                                right: track.left + filled_w,
                                ..track
                            };
                            fill_round_bordered(
                                dc,
                                filled,
                                bar_h / 2,
                                style.border,
                                style.border,
                                0,
                            )?;
                        }
                    }
                }
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

    fn notification(app: &str) -> Notification {
        Notification {
            app: app.into(),
            title: "title".into(),
            body: "body".into(),
        }
    }

    fn pending(app: &str) -> Pending {
        Pending {
            notification: notification(app),
            area: Rect::new(0, 0, 1000, 800),
            duration_override: None,
        }
    }

    #[test]
    fn stacks_up_to_max_visible_and_queues_the_rest() {
        let config = Config {
            max_visible: 2,
            ..Config::default()
        };
        let mut state = State {
            settings: ToastSettings {
                enabled: true,
                config,
                style: NotificationStyle {
                    font: String::new(),
                    corners: tile_config::Corners::Round,
                    background: Color::rgb(0, 0, 0),
                    foreground: Color::rgb(0, 0, 0),
                    dimmed: Color::rgb(0, 0, 0),
                    border: Color::rgb(0, 0, 0),
                },
            },
            queue: VecDeque::new(),
            visible: Vec::new(),
            area: Rect::default(),
            dpi: 96,
            font: None,
            small_font: None,
        };
        for app in ["one", "two", "three"] {
            state.queue.push_back(pending(app));
        }
        state.refill();
        assert_eq!(state.visible.len(), 2);
        assert_eq!(state.queue.len(), 1);
        state.visible.remove(0);
        state.refill();
        assert_eq!(state.visible.len(), 2);
        assert!(state.queue.is_empty());
    }

    #[test]
    fn hovering_pauses_the_countdown() {
        let mut card = Card::new(pending("app"));
        card.shown_at = Instant::now() - Duration::from_millis(100);
        card.set_hover(true);
        let frozen = card.elapsed();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(card.elapsed(), frozen);
        card.set_hover(false);
        assert!(card.elapsed() >= frozen);
    }
}
