//! Where the launcher's parts go, in client pixels, from its settings and the monitor's DPI.
//! Every length here is at 96 DPI and scaled with [`scale`].

use tile_config::{Launcher as Config, SearchPosition};
use tile_platform::scale;
use winsafe as w;

use crate::system::{Group, Power};

/// Space inside the window's edges, and inside the search box and rows.
const PAD: i32 = 12;
/// Space between the bar, the tray icons, the search box and the results.
const GAP: i32 = 8;
/// Space between neighbouring bar buttons.
const BUTTON_GAP: i32 = 6;
/// Extra space between the panels' buttons and the power buttons.
const GROUP_GAP: i32 = 14;
/// A tray icon's size, and the space around it in its cell.
const TRAY_ICON: i32 = 24;
const TRAY_CELL_PAD: i32 = 12;
/// How much taller than the main font the bar is.
const BAR_PADDING: i32 = 16;
/// The space the bar leaves beside its clock, besides [`PAD`] at either end.
const CLOCK_SPACE: i32 = 16;
/// The bar's height is split [`TIME_SHARE`] to [`DATE_SHARE`] (of [`SHARES`]) between the time
/// and the date under it.
const TIME_SHARE: i32 = 11;
const DATE_SHARE: i32 = 8;
const SHARES: i32 = 20;

/// What a bar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BarAction {
    /// Lists a group of controls in place of the results.
    Panel(Group),
    /// Ends the session or puts the machine away.
    Power(Power),
}

/// The buttons of the bar, left to right: an icon in Windows' icon font and what it does. The
/// first [`QUICK_BUTTONS`] open panels, the rest are power buttons.
pub(super) const BAR_BUTTONS: [(char, BarAction); 9] = [
    ('\u{E767}', BarAction::Panel(Group::Output)),
    ('\u{E720}', BarAction::Panel(Group::Input)),
    ('\u{E701}', BarAction::Panel(Group::Wifi)),
    ('\u{E702}', BarAction::Panel(Group::Bluetooth)),
    ('\u{E748}', BarAction::Power(Power::SwitchUser)),
    ('\u{F3B1}', BarAction::Power(Power::SignOut)),
    ('\u{E72E}', BarAction::Power(Power::Lock)),
    ('\u{E777}', BarAction::Power(Power::Restart)),
    ('\u{E7E8}', BarAction::Power(Power::ShutDown)),
];
/// How many of the bar's buttons open panels.
const QUICK_BUTTONS: usize = 4;

/// The pixel heights of the launcher's fonts, and of the bar they decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FontSizes {
    /// The search box and the row titles.
    pub(super) main: i32,
    /// The row titles when rows have no description under them.
    pub(super) title: i32,
    /// The row subtitles and the tray icons' names.
    pub(super) small: i32,
    /// The bar's time.
    pub(super) time: i32,
    /// The bar's date, under the time.
    pub(super) date: i32,
    /// The bar buttons' icons.
    pub(super) icons: i32,
    /// The bar's height, and its square buttons' width.
    pub(super) bar: i32,
}

impl FontSizes {
    /// The sizes for a `font_size` in points at `dpi`.
    pub(super) const fn new(font_size: u32, dpi: u32) -> Self {
        let main = (clamp(font_size, 6, 48) * dpi / 72) as i32;
        let bar = main + scale(BAR_PADDING, dpi);
        Self {
            main,
            title: main * 6 / 5,
            small: main * 4 / 5,
            time: bar * TIME_SHARE / SHARES,
            date: bar * DATE_SHARE / SHARES,
            icons: main * 12 / 10,
            bar,
        }
    }
}

/// `u32::clamp`, which isn't `const`.
const fn clamp(value: u32, min: u32, max: u32) -> u32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// Where things go, in client pixels.
pub(super) struct Layout {
    /// The space inside the edges, and inside the search box and rows.
    pub(super) pad: i32,
    pub(super) search: w::RECT,
    /// The row of tray icons, empty without any.
    pub(super) strip: w::RECT,
    /// One tray icon's cell width, and the icon's size inside it.
    pub(super) cell: i32,
    pub(super) tray_icon: i32,
    /// Where the first tray icon's cell starts, so the row is centred.
    pub(super) tray_x: i32,
    /// The bar with the clock and the power buttons, empty when it is off.
    pub(super) bar: w::RECT,
    /// The bar's buttons, in the order of [`BAR_BUTTONS`]; empty without the bar.
    pub(super) buttons: [w::RECT; BAR_BUTTONS.len()],
    /// Where the bar's date starts: the time is above, the date below.
    pub(super) date_top: i32,
    pub(super) list: w::RECT,
    /// A result row's height, and its icon's size (0 without icons).
    pub(super) row: i32,
    pub(super) icon: i32,
    /// How many rows fit in the list.
    pub(super) visible: usize,
}

fn contains(r: &w::RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

impl Layout {
    /// Lays out the client area `client` for `tray_icons` tray icons.
    pub(super) fn new(
        config: &Config,
        sizes: FontSizes,
        dpi: u32,
        tray_icons: usize,
        client: w::RECT,
    ) -> Self {
        let scale = |px| scale(px, dpi);
        let (main, small) = (sizes.main, sizes.small);
        let pad = scale(PAD);
        let gap = scale(GAP);
        let search_h = main + scale(24);
        let (row, icon) = if config.two_lines() {
            (main + small + scale(22), scale(32))
        } else {
            (sizes.title + scale(18), scale(24))
        };
        let icon = if config.show_icons { icon } else { 0 };
        let (left, right) = (client.left + pad, client.right - pad);
        let tray_icon = scale(TRAY_ICON);
        let cell = tray_icon + scale(TRAY_CELL_PAD);
        let strip_h = if tray_icons == 0 { 0 } else { cell };
        let tray_x = left + ((right - left - tray_icons as i32 * cell) / 2).max(0);
        // Top to bottom: the bar, the tray icons, then the search box and the results.
        let bar_h = if config.bar { sizes.bar } else { 0 };
        let bar = w::RECT {
            left,
            top: client.top + pad,
            right,
            bottom: client.top + pad + bar_h,
        };
        let below_bar = if bar_h > 0 {
            bar.bottom + gap
        } else {
            bar.bottom
        };
        let strip = w::RECT {
            left,
            top: below_bar,
            right,
            bottom: below_bar + strip_h,
        };
        let content = if strip_h > 0 {
            strip.bottom + gap
        } else {
            below_bar
        };
        // Square buttons from the right edge.
        let mut buttons = [w::RECT::default(); BAR_BUTTONS.len()];
        if bar_h > 0 {
            let mut x = bar.right;
            for (i, button) in buttons.iter_mut().enumerate().rev() {
                if i + 1 == QUICK_BUTTONS {
                    x -= scale(GROUP_GAP);
                }
                *button = w::RECT {
                    left: x - bar_h,
                    top: bar.top,
                    right: x,
                    bottom: bar.bottom,
                };
                x -= bar_h + scale(BUTTON_GAP);
            }
        }
        let (search, list) = match config.search_position {
            SearchPosition::Top => {
                let search = w::RECT {
                    left,
                    top: content,
                    right,
                    bottom: content + search_h,
                };
                let list = w::RECT {
                    left,
                    top: search.bottom + gap,
                    right,
                    bottom: client.bottom - pad,
                };
                (search, list)
            }
            SearchPosition::Bottom => {
                let search = w::RECT {
                    left,
                    top: client.bottom - pad - search_h,
                    right,
                    bottom: client.bottom - pad,
                };
                let list = w::RECT {
                    left,
                    top: content,
                    right,
                    bottom: search.top - gap,
                };
                (search, list)
            }
        };
        let visible = ((list.bottom - list.top) / row.max(1)).max(1) as usize;
        Self {
            pad,
            search,
            strip,
            cell,
            tray_icon,
            tray_x,
            bar,
            buttons,
            date_top: bar.top + bar_h * TIME_SHARE / SHARES,
            list,
            row,
            icon,
            visible,
        }
    }

    /// The width that holds `icons` tray icons in a row.
    pub(super) fn tray_width(&self, icons: usize) -> i32 {
        icons as i32 * self.cell + 2 * self.pad
    }

    /// The bar button at a point.
    pub(super) fn bar_button_at(&self, x: i32, y: i32) -> Option<usize> {
        self.buttons.iter().position(|b| contains(b, x, y))
    }

    /// Which of `icons` tray icons is at a point of the strip.
    pub(super) fn tray_at(&self, icons: usize, x: i32, y: i32) -> Option<usize> {
        let strip = self.strip;
        if strip.bottom <= strip.top || x < self.tray_x || !contains(&strip, x, y) {
            return None;
        }
        let index = ((x - self.tray_x) / self.cell.max(1)) as usize;
        (index < icons).then_some(index)
    }

    /// Which of `results` result rows is at a point of the list, scrolled by `scroll` rows.
    pub(super) fn row_at(&self, scroll: usize, results: usize, x: i32, y: i32) -> Option<usize> {
        if !contains(&self.list, x, y) {
            return None;
        }
        let index = scroll + ((y - self.list.top) / self.row) as usize;
        (index < results && index < scroll + self.visible).then_some(index)
    }
}

/// The width the bar needs for its clock, date and buttons.
pub(super) const fn bar_width(sizes: FontSizes, dpi: u32) -> i32 {
    let buttons =
        BAR_BUTTONS.len() as i32 * (sizes.bar + scale(BUTTON_GAP, dpi)) + scale(GROUP_GAP, dpi);
    // About 8 characters of the time or 24 of the date, a character being 11/20 of its height.
    let time = 8 * sizes.time;
    let date = 24 * sizes.date;
    let clock = (if time > date { time } else { date }) * 11 / 20;
    2 * scale(PAD, dpi) + clock + scale(CLOCK_SPACE, dpi) + buttons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(width: i32, height: i32) -> w::RECT {
        w::RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        }
    }

    fn sides(r: w::RECT) -> (i32, i32, i32, i32) {
        (r.left, r.top, r.right, r.bottom)
    }

    #[test]
    fn font_sizes_follow_the_dpi() {
        let at_96 = FontSizes::new(11, 96);
        assert_eq!(
            at_96,
            FontSizes {
                main: 14,
                title: 16,
                small: 11,
                time: 16,
                date: 12,
                icons: 16,
                bar: 30,
            }
        );
        let at_144 = FontSizes::new(11, 144);
        assert_eq!((at_144.main, at_144.bar, at_144.time), (22, 46, 25));
        assert_eq!(FontSizes::new(100, 96), FontSizes::new(48, 96));
    }

    /// The pixel positions the launcher has always had, so the constants can't drift.
    #[test]
    fn lays_out_the_bar_tray_search_and_list() {
        let config = Config {
            bar: true,
            compact: false,
            descriptions: true,
            ..Config::default()
        };
        let sizes = FontSizes::new(11, 96);
        let layout = Layout::new(&config, sizes, 96, 3, client(560, 400));
        assert_eq!(sides(layout.bar), (12, 12, 548, 42));
        assert_eq!(sides(layout.buttons[8]), (518, 12, 548, 42));
        assert_eq!(sides(layout.buttons[4]), (374, 12, 404, 42));
        assert_eq!(sides(layout.buttons[3]), (324, 12, 354, 42));
        assert_eq!(sides(layout.buttons[0]), (216, 12, 246, 42));
        assert_eq!(layout.date_top, 28);
        assert_eq!(sides(layout.strip), (12, 50, 548, 86));
        assert_eq!(
            (layout.cell, layout.tray_icon, layout.tray_x),
            (36, 24, 226)
        );
        assert_eq!(sides(layout.search), (12, 94, 548, 132));
        assert_eq!(sides(layout.list), (12, 140, 548, 388));
        assert_eq!((layout.row, layout.icon, layout.visible), (47, 32, 5));
        assert_eq!(layout.tray_width(3), 3 * 36 + 24);
        assert_eq!(bar_width(sizes, 96), 24 + 158 + 16 + 9 * 36 + 14);
    }

    #[test]
    fn lays_out_without_the_bar_or_tray_and_with_the_search_below() {
        let config = Config {
            bar: false,
            compact: true,
            show_icons: false,
            search_position: SearchPosition::Bottom,
            ..Config::default()
        };
        let layout = Layout::new(&config, FontSizes::new(11, 144), 144, 0, client(840, 600));
        assert_eq!(sides(layout.bar), (18, 18, 822, 18));
        assert!(layout.buttons.iter().all(|b| sides(*b) == (0, 0, 0, 0)));
        assert_eq!(sides(layout.strip), (18, 18, 822, 18));
        assert_eq!(sides(layout.search), (18, 524, 822, 582));
        assert_eq!(sides(layout.list), (18, 18, 822, 512));
        assert_eq!((layout.row, layout.icon), (53, 0));
    }

    #[test]
    fn finds_what_is_under_a_point() {
        let config = Config {
            bar: true,
            compact: false,
            descriptions: true,
            ..Config::default()
        };
        let layout = Layout::new(&config, FontSizes::new(11, 96), 96, 3, client(560, 400));
        assert_eq!(layout.bar_button_at(520, 20), Some(8));
        assert_eq!(layout.bar_button_at(515, 20), None);
        assert_eq!(layout.tray_at(3, 226, 60), Some(0));
        assert_eq!(layout.tray_at(3, 300, 60), Some(2));
        assert_eq!(layout.tray_at(3, 340, 60), None);
        assert_eq!(layout.row_at(0, 10, 100, 140), Some(0));
        assert_eq!(layout.row_at(2, 10, 100, 190), Some(3));
        assert_eq!(layout.row_at(0, 1, 100, 190), None);
    }
}
