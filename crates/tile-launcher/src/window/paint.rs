//! Drawing the launcher, and the fonts and icons kept between paints.

use std::{collections::HashMap, path::PathBuf};

use tile_platform::{colorref, fill_round, make_font, mix, scale, text};
use tile_shell::{Icon, notify::IconImage};
use winsafe::{self as w, co, guard::DeleteObjectGuard};

use super::{
    layout::{BAR_BUTTONS, BarAction, FontSizes},
    state::{RowIcon, State},
};
use crate::system::{format_date, format_time};

/// The launcher's fonts, at the sizes of a [`FontSizes`].
struct Fonts {
    main: DeleteObjectGuard<w::HFONT>,
    /// Larger, for row titles with no description.
    title: DeleteObjectGuard<w::HFONT>,
    small: DeleteObjectGuard<w::HFONT>,
    /// Bold, for the bar's time, which sits over its date within the bar's height.
    bold: DeleteObjectGuard<w::HFONT>,
    /// Smaller, for the bar's date.
    date: DeleteObjectGuard<w::HFONT>,
    /// Windows' icon font, for the bar's buttons.
    icons: DeleteObjectGuard<w::HFONT>,
}

impl Fonts {
    /// The fonts in `face`, or in the font Windows picks when that fails.
    fn new(face: &str, sizes: FontSizes) -> Self {
        Self::in_face(face, sizes).unwrap_or_else(|e| {
            tracing::warn!("launcher font `{face}`: {e}");
            Self::in_face("", sizes).unwrap_or_else(|_| {
                let plain = || {
                    w::HFONT::CreateFontIndirect(&w::LOGFONT::default())
                        .expect("the default font exists")
                };
                Self {
                    main: plain(),
                    title: plain(),
                    small: plain(),
                    bold: plain(),
                    date: plain(),
                    icons: plain(),
                }
            })
        })
    }

    fn in_face(face: &str, sizes: FontSizes) -> w::SysResult<Self> {
        Ok(Self {
            main: make_font(face, sizes.main, false)?,
            title: make_font(face, sizes.title, false)?,
            small: make_font(face, sizes.small, false)?,
            bold: make_font(face, sizes.time, true)?,
            date: make_font(face, sizes.date, false)?,
            icons: make_font(icon_face(), sizes.icons, false)?,
        })
    }
}

/// The icon font this Windows has: Fluent Icons on Windows 11, else the older MDL2 Assets.
fn icon_face() -> &'static str {
    let fonts = std::env::var_os("WINDIR").map_or_else(
        || PathBuf::from(r"C:\Windows\Fonts"),
        |dir| PathBuf::from(dir).join("Fonts"),
    );
    if fonts.join("SegoeIcons.ttf").exists() {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

/// The fonts last made, remade when the face or sizes change.
#[derive(Default)]
struct FontCache(Option<(String, FontSizes, Fonts)>);

impl FontCache {
    fn get(&mut self, face: &str, sizes: FontSizes) -> &Fonts {
        let stale = self
            .0
            .as_ref()
            .is_none_or(|(f, s, _)| f != face || *s != sizes);
        if stale {
            self.0 = Some((face.to_owned(), sizes, Fonts::new(face, sizes)));
        }
        &self.0.as_ref().expect("the fonts were just made").2
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum IconKey {
    /// An app, by its `shell:AppsFolder` id.
    App(String),
    /// A program file, by its path.
    File(String),
    /// A tray icon's image, by its handle, at a size.
    Tray { handle: isize, size: i32 },
}

/// The icons drawn so far, made at the size shown. `None` for one that couldn't be made.
#[derive(Default)]
pub(super) struct IconCache(HashMap<IconKey, Option<Icon>>);

impl IconCache {
    /// Forgets every icon, for a new size.
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// Forgets the tray icons' images, whose handles may since have been reused for others.
    pub(super) fn forget_tray(&mut self) {
        self.0.retain(|key, _| !matches!(key, IconKey::Tray { .. }));
    }

    fn get(&mut self, key: IconKey, make: impl FnOnce() -> Option<Icon>) -> Option<&Icon> {
        self.0.entry(key).or_insert_with(make).as_ref()
    }

    /// The icon of the app with this `shell:AppsFolder` id.
    fn app(&mut self, id: &str, size: i32) -> Option<&Icon> {
        self.get(IconKey::App(id.to_owned()), || {
            tile_shell::shell_icon(&format!("shell:AppsFolder\\{id}"), size)
        })
    }

    /// The icon of a program file.
    fn file(&mut self, path: &str, size: i32) -> Option<&Icon> {
        self.get(IconKey::File(path.to_owned()), || {
            tile_shell::shell_icon(path, size)
        })
    }

    /// A tray icon drawn at exactly `size` pixels.
    fn tray(&mut self, image: &IconImage, size: i32) -> Option<&Icon> {
        let handle = image.handle();
        self.get(IconKey::Tray { handle, size }, || image.render(size))
    }
}

/// The GDI objects kept between paints.
#[derive(Default)]
pub(super) struct Resources {
    fonts: FontCache,
    pub(super) icons: IconCache,
}

/// Draws the whole launcher into `dc`, whose size is `client`.
pub(super) fn paint(
    s: &State,
    res: &mut Resources,
    dc: &w::HDC,
    client: w::RECT,
) -> w::SysResult<()> {
    let (config, style) = (&s.settings.config, &s.settings.style);
    let sizes = s.font_sizes();
    let fonts = res.fonts.get(&style.font, sizes);
    let icons = &mut res.icons;
    let layout = s.layout(client);
    let scale = |px| scale(px, s.dpi);
    let radius = scale(16);
    let pad = layout.pad;
    let background = w::HBRUSH::CreateSolidBrush(colorref(style.background))?;
    dc.FillRect(client, &background)?;
    dc.SetBkMode(co::BKMODE::TRANSPARENT)?;

    // The search box.
    let field = mix(style.background, style.foreground, 20);
    fill_round(dc, layout.search, radius, field)?;
    let mut inner = layout.search;
    inner.left += pad;
    inner.right -= pad;
    let (main, small) = (&*fonts.main, &*fonts.small);
    let left = co::DT::LEFT | co::DT::VCENTER;
    let centred = co::DT::CENTER | co::DT::VCENTER;
    if s.query.text().is_empty() {
        let placeholder = s
            .panel
            .map_or_else(|| config.placeholder_text(), |g| g.title().to_owned());
        text(dc, main, style.dimmed, &placeholder, inner, left)?;
    } else {
        text(dc, main, style.foreground, s.query.text(), inner, left)?;
    }
    let typed = {
        let _font = dc.SelectObject(main)?;
        dc.GetTextExtentPoint32(s.query.before_caret())?.cx
    };
    let caret_x = (inner.left + typed + 1).min(inner.right);
    let caret_h = sizes.main;
    let mid = (inner.top + inner.bottom) / 2;
    let caret = w::RECT {
        left: caret_x,
        top: mid - caret_h / 2,
        right: caret_x + scale(2).max(1),
        bottom: mid + caret_h / 2,
    };
    let caret_brush = w::HBRUSH::CreateSolidBrush(colorref(style.selected))?;
    dc.FillRect(caret, &caret_brush)?;

    // The tray icons: a row of their own.
    if layout.strip.bottom > layout.strip.top {
        let cell = layout.cell;
        let hover = mix(style.background, style.foreground, 25);
        let mut end = layout.tray_x;
        for (i, item) in s.tray.iter().enumerate() {
            let x = layout.tray_x + i as i32 * cell;
            if x + cell > layout.strip.right {
                break;
            }
            end = x + cell;
            let slot = w::RECT {
                left: x,
                right: x + cell,
                ..layout.strip
            };
            if s.tray_hover == Some(i) {
                fill_round(dc, slot, scale(10), hover)?;
            }
            let size = layout.tray_icon;
            let (ix, iy) = (
                x + (cell - size) / 2,
                slot.top + (slot.bottom - slot.top - size) / 2,
            );
            // A tray icon is usually 16 or 32 pixels, small for the size shown. One big enough is
            // shrunk smoothly; a smaller one gives way to the program's own icon, which Windows
            // has at larger sizes, and failing that is grown smoothly rather than stretched.
            let hdc = dc.ptr() as isize;
            let big_enough = item.icon.as_ref().filter(|_| item.icon_size >= size);
            if let Some(icon) = big_enough.and_then(|image| icons.tray(image, size)) {
                icon.draw(hdc, ix, iy, size);
            } else if let Some(icon) = (!item.path.is_empty())
                .then(|| icons.file(&item.path, size))
                .flatten()
            {
                icon.draw(hdc, ix, iy, size);
            } else if let Some(icon) = item.icon.as_ref().and_then(|i| icons.tray(i, size)) {
                icon.draw(hdc, ix, iy, size);
            } else {
                let mut buf = [0; 4];
                let initial = item
                    .name
                    .chars()
                    .next()
                    .map_or("", |c| c.encode_utf8(&mut buf));
                text(dc, small, style.dimmed, initial, slot, centred)?;
            }
        }
        if let Some(item) = s.tray_hover.and_then(|i| s.tray.get(i)) {
            let bounds = w::RECT {
                left: end + pad,
                ..layout.strip
            };
            text(dc, small, style.dimmed, &item.name, bounds, left)?;
        }
    }

    // The bar: the clock and date, and the power buttons.
    if layout.bar.bottom > layout.bar.top {
        let t = w::GetLocalTime();
        let time = format_time(t.wHour, t.wMinute, t.wSecond);
        let date = format_date(t.wYear, t.wMonth, t.wDay, t.wDayOfWeek);
        let bar = layout.bar;
        let inset = scale(6);
        let time_area = w::RECT {
            left: bar.left + inset,
            right: layout.buttons[0].left - pad,
            bottom: layout.date_top,
            ..bar
        };
        text(dc, &fonts.bold, style.foreground, &time, time_area, left)?;
        let date_area = w::RECT {
            top: layout.date_top,
            bottom: bar.bottom,
            ..time_area
        };
        text(dc, &fonts.date, style.dimmed, &date, date_area, left)?;
        let idle = mix(style.background, style.foreground, 20);
        let hovered = mix(style.background, style.foreground, 40);
        for (i, ((glyph, action), rect)) in BAR_BUTTONS.iter().zip(&layout.buttons).enumerate() {
            // A switch is lit while on and dim while off or missing.
            let active = match action {
                BarAction::Panel(group) => group.is_on(&s.status) == Some(true),
                BarAction::Power(_) => false,
            };
            let off = matches!(action, BarAction::Panel(_)) && !active;
            let (fill, colour) = if active {
                (style.selected, style.selected_foreground)
            } else if s.bar_hover == Some(i) {
                (hovered, if off { style.dimmed } else { style.foreground })
            } else {
                (idle, if off { style.dimmed } else { style.foreground })
            };
            fill_round(dc, *rect, scale(10), fill)?;
            let mut buf = [0; 4];
            let glyph = glyph.encode_utf8(&mut buf);
            text(dc, &fonts.icons, colour, glyph, *rect, centred)?;
            // A line under the button whose panel is open.
            if matches!(action, BarAction::Panel(group) if s.panel == Some(*group)) {
                let inset = (rect.right - rect.left) / 4;
                let line = w::RECT {
                    left: rect.left + inset,
                    top: rect.bottom - scale(4),
                    right: rect.right - inset,
                    bottom: rect.bottom - scale(2),
                };
                fill_round(dc, line, scale(2), colour)?;
            }
        }
    }

    // The results.
    if s.results.is_empty() {
        let message = if s.loading {
            "Loading apps…"
        } else {
            "No matches"
        };
        text(dc, main, style.dimmed, message, layout.list, centred)?;
        return Ok(());
    }
    let end = (s.scroll + layout.visible).min(s.results.len());
    for (slot, index) in (s.scroll..end).enumerate() {
        let (title, subtitle, icon) = s.describe(&s.results[index]);
        let top = layout.list.top + slot as i32 * layout.row;
        let row = w::RECT {
            left: layout.list.left,
            top,
            right: layout.list.right,
            bottom: top + layout.row,
        };
        let selected = index == s.selected;
        let (fg, dim) = if selected {
            fill_round(dc, row, radius, style.selected)?;
            (style.selected_foreground, style.selected_foreground)
        } else {
            (style.foreground, style.dimmed)
        };
        let mut x = row.left + pad;
        if layout.icon > 0 {
            let y = top + (layout.row - layout.icon) / 2;
            match icon {
                Some(RowIcon::App(id)) => {
                    if let Some(icon) = icons.app(id, layout.icon) {
                        icon.draw(dc.ptr() as isize, x, y, layout.icon);
                    }
                }
                Some(RowIcon::Tray(image)) => {
                    image.draw(dc.ptr() as isize, x, y, layout.icon);
                }
                None => {}
            }
            x += layout.icon + pad;
        }
        let bounds = w::RECT {
            left: x,
            right: row.right - pad,
            ..row
        };
        if !config.two_lines() {
            text(dc, &fonts.title, fg, &title, bounds, left)?;
            if config.descriptions {
                let used = {
                    let _font = dc.SelectObject(&*fonts.title)?;
                    dc.GetTextExtentPoint32(&title)?.cx
                };
                let after = w::RECT {
                    left: bounds.left + used + pad,
                    ..bounds
                };
                if after.left < after.right {
                    text(dc, small, dim, &subtitle, after, left)?;
                }
            }
        } else {
            let spacing = scale(2);
            let block = sizes.main + spacing + sizes.small;
            let name_top = top + (layout.row - block) / 2;
            let name = w::RECT {
                top: name_top,
                bottom: name_top + sizes.main + spacing,
                ..bounds
            };
            let id = w::RECT {
                top: name.bottom,
                bottom: name.bottom + sizes.small + spacing,
                ..bounds
            };
            text(dc, main, fg, &title, name, co::DT::LEFT | co::DT::TOP)?;
            text(dc, small, dim, &subtitle, id, co::DT::LEFT | co::DT::TOP)?;
        }
    }
    Ok(())
}
