//! Where the launcher's parts go, in client pixels, from its settings and the monitor's DPI.
//! Every length here is at 96 DPI and scaled with [`scale`].

use tile_config::{Launcher as Config, SearchPosition};
use tile_platform::scale;
use winsafe as w;

use crate::system::{Group, Power};

/// Space inside the window's edges, and inside the search box and rows.
const PAD: i32 = 12;
/// Space between the sidebar's parts, and between the search box and the results.
const GAP: i32 = 8;
/// Space between the search column and the sidebar.
const GUTTER: i32 = 16;
/// How many buttons or tray icons a row of the sidebar holds.
const SIDE_COLUMNS: i32 = 5;
/// The least width left for the search column beside the sidebar.
const MAIN_MIN: i32 = 260;
/// Space between neighbouring bar buttons.
const BUTTON_GAP: i32 = 6;
/// A tray icon's size, and the space around it in its cell.
const TRAY_ICON: i32 = 24;
const TRAY_CELL_PAD: i32 = 12;
/// How much taller than the main font the bar is.
const BAR_PADDING: i32 = 16;
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

/// The buttons of the sidebar, in reading order: an icon in Windows' icon font and what it does. The
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
    /// The glyphs standing in for a row's missing picture.
    pub(super) row_icons: i32,
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
            row_icons: main * 16 / 10,
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
    /// The rows of tray icons, empty without any.
    pub(super) strip: w::RECT,
    /// One tray icon's cell width, and the icon's size inside it.
    pub(super) cell: i32,
    pub(super) tray_icon: i32,
    /// How many tray icons there are, and how many rows of the strip they fill.
    tray_icons: usize,
    tray_rows: usize,
    /// The clock at the top of the sidebar, empty when the bar is off.
    pub(super) bar: w::RECT,
    /// The bar's buttons under the clock, in the order of [`BAR_BUTTONS`]; empty without the bar.
    pub(super) buttons: [w::RECT; BAR_BUTTONS.len()],
    /// Where the bar's date starts: the time is above, the date below.
    pub(super) date_top: i32,
    /// The bottom of the sidebar's content, which the window must be tall enough for.
    pub(super) side_end: i32,
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
        let (row, icon) = if config.two_lines() {
            (main + small + scale(22), scale(32))
        } else {
            (sizes.title + scale(18), scale(24))
        };
        let icon = if config.show_icons { icon } else { 0 };
        let (left, right) = (client.left + pad, client.right - pad);
        let tray_icon = scale(TRAY_ICON);
        let cell = tray_icon + scale(TRAY_CELL_PAD);
        let pitch = sizes.bar + scale(BUTTON_GAP);
        let bar_h = if config.bar { sizes.bar } else { 0 };
        // The clock, the buttons and the tray icons stack in a column at the right; the search
        // box and the results take the rest.
        let side_w = if bar_h > 0 || tray_icons > 0 {
            SIDE_COLUMNS * pitch.max(cell)
        } else {
            0
        };
        let side_left = right - side_w;
        let main_right = if side_w > 0 {
            (side_left - scale(GUTTER)).max(left + 1)
        } else {
            right
        };
        let top = client.top + pad;
        let bar = w::RECT {
            left: side_left,
            top,
            right,
            bottom: top + bar_h,
        };
        let mut y = bar.bottom;
        let mut buttons = [w::RECT::default(); BAR_BUTTONS.len()];
        if bar_h > 0 {
            y += gap;
            for group in [0..QUICK_BUTTONS, QUICK_BUTTONS..BAR_BUTTONS.len()] {
                let width = group.len() as i32 * pitch - scale(BUTTON_GAP);
                let first = side_left + (side_w - width) / 2;
                for (k, i) in group.enumerate() {
                    let x = first + k as i32 * pitch;
                    buttons[i] = w::RECT {
                        left: x,
                        top: y,
                        right: x + bar_h,
                        bottom: y + bar_h,
                    };
                }
                y += pitch;
            }
            y -= scale(BUTTON_GAP);
        }
        // Icons wrap onto as many rows as the sidebar needs.
        let fit = (side_w / cell.max(1)).max(1) as usize;
        let tray_rows = tray_icons.div_ceil(fit);
        if tray_icons > 0 && bar_h > 0 {
            y += gap;
        }
        let strip = w::RECT {
            left: side_left,
            top: y,
            right,
            bottom: y + tray_rows as i32 * cell,
        };
        let side_end = strip.bottom;
        let search_h = main + scale(24);
        let (search, list) = match config.search_position {
            SearchPosition::Top => {
                let search = w::RECT {
                    left,
                    top,
                    right: main_right,
                    bottom: top + search_h,
                };
                let list = w::RECT {
                    left,
                    top: search.bottom + gap,
                    right: main_right,
                    bottom: client.bottom - pad,
                };
                (search, list)
            }
            SearchPosition::Bottom => {
                let search = w::RECT {
                    left,
                    top: client.bottom - pad - search_h,
                    right: main_right,
                    bottom: client.bottom - pad,
                };
                let list = w::RECT {
                    left,
                    top,
                    right: main_right,
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
            tray_icons,
            tray_rows,
            bar,
            buttons,
            date_top: bar.top + bar_h * TIME_SHARE / SHARES,
            side_end,
            list,
            row,
            icon,
            visible,
        }
    }

    /// The tray icons on strip row `row`, and where the first one's cell starts so the row is
    /// centred. The icons are split evenly over the rows, the first rows taking the extra.
    pub(super) fn tray_row(&self, row: usize) -> (std::ops::Range<usize>, i32) {
        let rows = self.tray_rows.max(1);
        let (base, extra) = (self.tray_icons / rows, self.tray_icons % rows);
        let first = row * base + row.min(extra);
        let end = (first + base + usize::from(row < extra)).min(self.tray_icons);
        let first = first.min(end);
        let used = (end - first) as i32 * self.cell;
        let x = self.strip.left + ((self.strip.right - self.strip.left - used) / 2).max(0);
        (first..end, x)
    }

    /// The top of the strip's row `row`.
    pub(super) fn tray_top(&self, row: usize) -> i32 {
        self.strip.top + row as i32 * self.cell
    }

    /// The bar button at a point.
    pub(super) fn bar_button_at(&self, x: i32, y: i32) -> Option<usize> {
        self.buttons.iter().position(|b| contains(b, x, y))
    }

    /// Which tray icon is at a point of the strip.
    pub(super) fn tray_at(&self, x: i32, y: i32) -> Option<usize> {
        if !contains(&self.strip, x, y) {
            return None;
        }
        let cell = self.cell.max(1);
        let (icons, start) = self.tray_row(((y - self.strip.top) / cell) as usize);
        if x < start {
            return None;
        }
        let index = icons.start + ((x - start) / cell) as usize;
        icons.contains(&index).then_some(index)
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

/// The least window width that holds the sidebar beside a usable search column.
pub(super) const fn sidebar_width(sizes: FontSizes, dpi: u32) -> i32 {
    let pitch = sizes.bar + scale(BUTTON_GAP, dpi);
    let cell = scale(TRAY_ICON, dpi) + scale(TRAY_CELL_PAD, dpi);
    let column = if pitch > cell { pitch } else { cell };
    2 * scale(PAD, dpi) + SIDE_COLUMNS * column + scale(GUTTER, dpi) + scale(MAIN_MIN, dpi)
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
                row_icons: 22,
                bar: 30,
            }
        );
        let at_144 = FontSizes::new(11, 144);
        assert_eq!((at_144.main, at_144.bar, at_144.time), (22, 46, 25));
        assert_eq!(FontSizes::new(100, 96), FontSizes::new(48, 96));
    }

    /// The pixel positions of the sidebar and the search column, so the constants can't drift.
    #[test]
    fn lays_out_the_sidebar_search_and_list() {
        let config = Config {
            bar: true,
            compact: false,
            descriptions: true,
            ..Config::default()
        };
        let sizes = FontSizes::new(11, 96);
        let layout = Layout::new(&config, sizes, 96, 3, client(560, 400));
        assert_eq!(sides(layout.bar), (368, 12, 548, 42));
        assert_eq!(layout.date_top, 28);
        assert_eq!(sides(layout.buttons[0]), (389, 50, 419, 80));
        assert_eq!(sides(layout.buttons[3]), (497, 50, 527, 80));
        assert_eq!(sides(layout.buttons[4]), (371, 86, 401, 116));
        assert_eq!(sides(layout.buttons[8]), (515, 86, 545, 116));
        assert_eq!(sides(layout.strip), (368, 124, 548, 160));
        assert_eq!((layout.cell, layout.tray_icon), (36, 24));
        assert_eq!(layout.tray_row(0), (0..3, 404));
        assert_eq!(layout.side_end, 160);
        assert_eq!(sides(layout.search), (12, 12, 352, 50));
        assert_eq!(sides(layout.list), (12, 58, 352, 388));
        assert_eq!((layout.row, layout.icon, layout.visible), (47, 32, 7));
        assert_eq!(sidebar_width(sizes, 96), 24 + 180 + 16 + 260);
    }

    #[test]
    fn wraps_many_tray_icons_onto_even_rows() {
        let config = Config {
            bar: false,
            ..Config::default()
        };
        let layout = Layout::new(&config, FontSizes::new(11, 96), 96, 17, client(560, 400));
        // Five cells fit a row, so 17 icons take four rows of 5, 4, 4 and 4.
        assert_eq!(sides(layout.strip), (368, 12, 548, 156));
        assert_eq!(layout.tray_row(0), (0..5, 368));
        assert_eq!(layout.tray_row(1), (5..9, 386));
        assert_eq!(layout.tray_row(3), (13..17, 386));
        assert_eq!(layout.tray_at(387, 60), Some(5));
        assert_eq!(layout.tray_at(369, 20), Some(0));
        assert_eq!(layout.tray_at(369, 60), None);
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
        assert_eq!(layout.bar.bottom, layout.bar.top);
        assert!(layout.buttons.iter().all(|b| sides(*b) == (0, 0, 0, 0)));
        assert_eq!(layout.strip.bottom, layout.strip.top);
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
        assert_eq!(layout.bar_button_at(520, 90), Some(8));
        assert_eq!(layout.bar_button_at(510, 90), None);
        assert_eq!(layout.tray_at(405, 130), Some(0));
        assert_eq!(layout.tray_at(480, 130), Some(2));
        assert_eq!(layout.tray_at(520, 130), None);
        assert_eq!(layout.row_at(0, 10, 100, 60), Some(0));
        assert_eq!(layout.row_at(2, 10, 100, 110), Some(3));
        assert_eq!(layout.row_at(0, 1, 100, 110), None);
    }
}
