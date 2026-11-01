# Configuration

Tile reads `%APPDATA%\Tile\config.yml`, or `config.yaml` if that is the one there (`tile
config` prints the path). Tile runs on its built-in defaults, so the file only needs the settings
you want to change. On first run it creates `config.yml` holding just `tray`, the tray's
switches, which Tile saves there whenever you change them. Add a section and key to change
anything else; the [default config](#the-default-config) below lists them all to copy from. Saved changes apply immediately,
and `tile check [file]` reports what is wrong with a config. A bad config is not applied; the
previous one is kept, and the tray icon turns red until the file is fixed (see
[the tray](windows-integration.md#the-tray)).

Whole configs to start from are in [`examples/`](../examples): `default.yml` is the defaults
written out in full, every bind included (with `default_binds: false`), and `mbwilding.yml` is
the author's own Dvorak setup with home-row mods.

These sections **replace** the built-in set entirely when you write them, so copy the defaults
from the end of this page as a starting point:

- `rules`
- `remap.layers`

`binds` add to the [default binds](keybinds.md#default-keys) instead (see [`binds`](#binds)).
Every other section merges: a key you leave out keeps its default. Unknown keys are an error.

YAML needs quotes around a key or value that starts with punctuation (`"#89b4fa"`, `"/"`, `"!"`)
or contains `: ` or ` #`, and around numbers meant as text (`"2"`).

## Reference

### `general`

| Key | Default | Meaning |
| --- | --- | --- |
| `tiling` | `true` | Manage windows. `false` leaves them alone and gives Win+L back; turning it on again manages the open windows. |
| `default_binds` | `true` | Start from the [default binds](keybinds.md#default-keys), which `binds` add to. `false` leaves only your own `binds`. |
| `remap` | `true` | Run the key remapper with the `remap` layers. |
| `gaps_in` | `10` | Pixels between neighbouring tiled windows. |
| `gaps_out` | `10` | Pixels between tiled windows and the monitor edge. |
| `independent_workspaces` | `true` | Every monitor has workspaces of its own, so `workspace 2` is workspace 2 on the monitor you are on and never moves focus to another monitor. `false` syncs the monitors: switching workspace switches them all together. See [layouts](layouts.md#workspaces-and-monitors). |
| `layout` | `master` | Layout of new workspaces: `dwindle`, `master`, `monocle`, `scrolling` or `even`. New workspaces on a portrait monitor start with `even` instead. See [layouts](layouts.md). |
| `borders` | `true` | Colour the window border to show focus (Windows 11). |
| `border_active` | `theme.accent` | Border colour of the focused window. |
| `border_inactive` | `theme.inactive` | Border colour of every other tiled window. |
| `focus_new_windows` | `true` | Focus a window when it opens. |
| `decorations` | `false` | Keep title bars on tiled windows. Floating windows always keep theirs; a rule can override this per app. |
| `override_win_l` | `true` | Take over Win+L so it can be bound. See [Windows integration](windows-integration.md). |
| `disable_click_to_do` | `true` | Switch off Click to Do, whose glow shows whenever Win is held (Copilot+ PCs). |
| `disable_start_menu` | `false` | Releasing Win on its own doesn't open the Start menu. Win combos and binds are unaffected. See [Windows integration](windows-integration.md). |

### `theme`

The font, corners and colours shared by everything Tile draws: the focus borders, the
[launcher](launcher.md) and the [workspace indicator](#workspace_indicator). Change them here to
restyle everything at once. Each of those sections has its own keys for the same things, unset
by default; set one there to override the theme for just that part.

| Key | Default | Meaning |
| --- | --- | --- |
| `font` | `Segoe UI` | Font family. |
| `corners` | `round` | Window corners: `round`, `small` or `square` (Windows 11). |
| `antialiasing` | `true` | Smooth the edges of the rounded shapes Tile draws (launcher rows and buttons, notification popups). |
| `background` | `"#1e1e2e"` | The launcher's window. |
| `foreground` | `"#cdd6f4"` | Text. |
| `dimmed` | `"#7f849c"` | Secondary text: the launcher's app ids and placeholder. |
| `accent` | `"#89b4fa"` | The focused window's border, the launcher's outline, caret and selected row, and the workspace indicator. |
| `accent_foreground` | `"#1e1e2e"` | Text on `accent`: the launcher's selected row and the workspace number. |
| `inactive` | `"#45475a"` | The border of tiled windows without focus. |

### `tray`

The tray menu's switches. Tile writes these itself when you change one, and reads them at
startup, so you rarely need to edit them. It writes only the switch you changed, so a config
with an error keeps the switches it has.

| Key | Default | Meaning |
| --- | --- | --- |
| `tile` | `false` | The window manager is on (the tray's Tile item). Off until switched on. |
| `remap` | `false` | The key remapper is on (the tray's Remap item). Off until switched on. |
| `shell` | `false` | Tile is the shell (the tray's Shell item): it hosts the notification area when no taskbar is running, keeps the launcher bound (to `Win+R`, or `Win+D` if that is taken, when you haven't bound it) and shows the tray icons in a row of their own at its top. See [shell mode](shell.md). Turning it on also makes Tile your sign-in shell in place of Explorer from the next sign-in, and Tile keeps that in step with the file: a config with `shell: true` sets it up the first time Tile runs, and editing it back puts Explorer back. Tile started with `--shell`, which that sign-in shell uses, is the shell whatever this says. |
| `log` | `false` | Log to the log file (the tray's Log item). |

These are separate from `general.tiling` and `general.remap`, which switch the features off
entirely: a feature runs only when both its `general` setting and its tray switch are on.

### `shell`

Tile as the Windows shell, in place of Explorer's taskbar. See [shell mode](shell.md). Whether
Tile is the shell is the tray's Shell switch, [`tray.shell`](#tray).

| Key | Default | Meaning |
| --- | --- | --- |
| `run_startup` | `true` | When Tile starts as the shell, run the `Run` and `RunOnce` registry keys and the Startup folders, as Explorer would. |

### `cursor`

| Key | Default | Meaning |
| --- | --- | --- |
| `warp` | `true` | Whenever a keybind or dispatch moves focus, put the pointer on the focused window (the monitor's centre if it is empty), unless it is already over it. The pointer is then hidden, as typing hides it (needs `hide_on_key`), until the mouse moves. |
| `hide_on_key` | `true` | Hide the pointer while typing, until the mouse moves or clicks. Modifiers on their own and keybinds don't count as typing. |
| `drag` | `true` | Hold Win and drag a window with the left button, from anywhere on it. On release the window is re-tiled where it is dropped. |

### `animations`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Ease windows into place. |
| `duration_ms` | `180` | Length of a move or resize. |
| `bezier` | `[0.0, 0.75, 0.15, 1.0]` | Cubic-bezier control points `x1, y1, x2, y2`. |

### Layout sections

| Section | Key | Default | Meaning |
| --- | --- | --- | --- |
| `dwindle` | `split_ratio` | `0.5` | Share of a split given to the existing window. |
| `master` | `mfact` | `0.5` | Width of the master area, as a share of the monitor. |
| `master` | `count` | `1` | Windows in the master area. |
| `master` | `new_is_master` | `false` | New windows become the master. |
| `scrolling` | `proportion` | `0.333` | Width of new columns, as a share of the monitor. |
| `scrolling` | `proportion_presets` | `[0.25, 0.333, 0.5, 1.0]` | Widths `cycleproportion` steps through. |
| `scrolling` | `center_focused` | `on_overflow` | When to centre the focused column: `always`, `on_overflow` (when it would otherwise be partly off screen) or `never`. |

### `launcher`

The app launcher, opened by the `launcher` dispatcher. See [launcher](launcher.md).

| Key | Default | Meaning |
| --- | --- | --- |
| `width` | `560` | Width in pixels at 100% scaling (scaled with the monitor). |
| `max_visible` | `6` | The most result rows shown at once. The window is exactly as tall as its content, up to this many rows, so it shrinks as you type; more results scroll. |
| `search_position` | `top` | Where the search box sits: `top` or `bottom`. |
| `show_icons` | `true` | Show each app's icon. |
| `compact` | `true` | One line per row, with the description after the name instead of under it. |
| `descriptions` | `false` | Show what each row is (an app's id, a command's explanation). Off, rows are just names. |
| `max_results` | `50` | The most apps listed. |
| `track_usage` | `true` | Remember launches in `%LOCALAPPDATA%\Tile\launcher.json`. |
| `sort_by_usage` | `true` | List recent launches first, and let often launched apps win close matches. |
| `ignore_mouse` | `false` | The mouse doesn't select or launch. |
| `pinned` | `[]` | App names or ids listed first, in this order, before anything is typed. |
| `hide` | `[]` | Regexes (case-insensitive) on the app name or id; matching apps are never listed. |
| `font` | `theme.font` | Font family. |
| `font_size` | `11` | Font size in points. |
| `corners` | `theme.corners` | Window corners: `round`, `small` or `square` (Windows 11). |
| `opacity` | `1.0` | How opaque the whole window is, from `0.1` to `1.0`. |
| `background` | `theme.background` | Window colour. |
| `border` | `theme.accent` | The window's outline (Windows 11). |
| `foreground` | `theme.foreground` | Text colour. |
| `dimmed` | `theme.dimmed` | The app id line and the placeholder. |
| `selected` | `theme.accent` | The selected row and the caret. |
| `selected_foreground` | `theme.accent_foreground` | Text on the selected row. |
| `placeholder` | `""` | Shown in the empty search box. Empty shows `Search apps, actions, equations and commands`, leaving out what is switched off. |
| `calculator` | `true` | Show the value of a search that reads as a calculation first; Enter copies it. See [calculator](launcher.md#calculator). |
| `commands` | `true` | Offer Tile's dispatchers and running the text as a command line among the results. See [commands](launcher.md#commands). |
| `system_items` | `true` | List the system items (volume and the audio devices, microphone, network, Wi-Fi, Bluetooth, battery, clock, power) and tray icons that match what is typed, after the apps. See [system items](launcher.md#system-items-and-tray-icons). |
| `bar` | `true` | Show the bar at the top of the launcher: the time (bold, with seconds) and date, and icon buttons for switch user, log out, lock, restart and shut down. See [the bar](launcher.md#the-bar). |
| `fix_popups` | `true` | Move the popup of a tray icon's app next to the pointer when the app opens it in a monitor's corner (as G HUB does, for want of a taskbar to place it by). See [tray icons](launcher.md#tray-icons). |

### `workspace_indicator`

A small square showing the workspace number that flashes up on the focused monitor when a
`workspace` or `movetoworkspace` dispatcher runs (even onto the workspace you are already on),
then fades out. It never takes focus and clicks go through it.

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Show it. |
| `position` | `bottom_right` | Where it sits on the monitor's work area: `center`, `top`, `top_left`, `top_right`, `bottom`, `bottom_left`, `bottom_right`, `left` or `right`. |
| `gap` | `48` | Pixels from the edges it sits against, at 100% scaling. Unused for `center`. |
| `size` | `96` | Width and height in pixels at 100% scaling. It widens for a number that doesn't fit. |
| `duration_ms` | `400` | How long it stays fully shown. |
| `fade_ms` | `250` | How long it then takes to fade out. `0` hides it at once. |
| `font` | `theme.font` | Font family of the number. |
| `font_size` | `36` | Font size in points. |
| `bold` | `true` | Bold number. |
| `corners` | `theme.corners` | Corners: `round`, `small` or `square` (Windows 11). |
| `opacity` | `0.95` | How opaque it is when fully shown, from `0.1` to `1.0`. |
| `background` | `theme.accent` | Its colour. |
| `foreground` | `theme.accent_foreground` | The number's colour. |

### `notifications`

Popups for Windows toast notifications, read through `UserNotificationListener`. See
[Windows integration](windows-integration.md#notifications).

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | unset | Show a popup for each new notification. Unset follows shell mode: on while Tile hosts the shell, off otherwise. `true` or `false` overrides that. |
| `position` | `top_right` | Where it sits on the monitor's work area: `center`, `top`, `top_left`, `top_right`, `bottom`, `bottom_left`, `bottom_right`, `left` or `right`. |
| `gap` | `16` | Pixels from the edges it sits against, at 100% scaling. Unused for `center`. |
| `width` | `380` | Width in pixels at 100% scaling. It grows tall enough for the icon and the text. |
| `duration_ms` | `5000` | How long it stays shown before it fades out. Hovering a popup pauses its countdown; it resumes when the pointer leaves. |
| `fade_ms` | `400` | How long it then takes to fade out. `0` hides it at once. |
| `max_visible` | `3` | How many popups are shown stacked at once; the rest wait their turn. |
| `show_app_name` | `true` | Show the posting app's name above the title. |
| `border` | `theme.accent` | The card's outline colour. |
| `border_width` | `1` | The outline's thickness in pixels at 100% scaling. `0` draws no outline. |
| `filter` | `[]` | Mutes, or overrides the duration of, notifications from apps whose name matches. Each entry has `app` (a regex, matched the same way as a [window rule](window-rules.md)'s patterns), `mute` and `duration_ms`. The first matching entry wins each field. |

### `binds`

Keybinds, as `<keys>: <dispatcher>`. They add to the [default binds](keybinds.md#default-keys):
a key that is already bound gets your dispatcher instead, and an empty value (`""` or `~`)
unbinds it. Set `general.default_binds: false` to start from nothing.

```yaml
binds:
  super+t: exec wtd.exe -w new          # replaces the default Win+T
  super+e: focusorexec code.cmd         # focus VS Code, or start it
  super+z: ~                            # unbind the default Win+Z
```

Launching (`exec`, `exechidden`, `focusorexec`), closing, locking, desktops, `reload`, the `toggle*` dispatchers and `exit` work even with tiling
off or paused; the rest only run while Tile is on (the tray's Tile item). See
[keybinds](keybinds.md) for the key names and every dispatcher.

### `rules`

Per-window overrides. See [window rules](window-rules.md).

### `remap`

| Key | Default | Meaning |
| --- | --- | --- |
| `modifier_guard` | `true` | A layer's lone Alt or Win press doesn't open a menu or Start. |
| `layers.<name>` | see below | Remap layers. See [key remapping](key-remapping.md). |

## The default config

This is the configuration Tile runs with when the file is empty or missing, so it can be copied
as it is. A test checks that this block lists every setting at its real default.

<!-- default-config:start -->
```yaml
---
general:
  tiling: true                  # manage windows; false leaves them alone
  default_binds: true           # start from the default binds; false leaves only yours
  remap: true                   # remap keys with the remap layers
  gaps_in: 10                   # px between neighbouring windows
  gaps_out: 10                  # px between windows and the monitor edge
  independent_workspaces: true  # each monitor has its own workspaces; false syncs them
  layout: master                # dwindle | master | monocle | scrolling | even
  borders: true                 # colour the window border to show focus (Windows 11)
  # border_active: "#89b4fa"    # unset follows theme.accent
  # border_inactive: "#45475a"  # unset follows theme.inactive
  focus_new_windows: true
  decorations: false            # keep title bars on tiled windows
  override_win_l: true          # take over Win+L
  disable_click_to_do: true     # switch off Click to Do
  disable_start_menu: false     # Win on its own doesn't open Start

# Shared by the borders, the launcher and the workspace indicator, unless they set their own.
theme:
  font: Segoe UI
  corners: round                # round | small | square
  antialiasing: true             # smooth the edges of rounded shapes
  background: "#1e1e2e"
  foreground: "#cdd6f4"
  dimmed: "#7f849c"             # secondary text
  accent: "#89b4fa"             # focus border, launcher outline and selection, workspace indicator
  accent_foreground: "#1e1e2e"  # text on accent
  inactive: "#45475a"           # border of unfocused windows

# The tray's switches. Tile saves these when you change them in the tray.
tray:
  tile: false                   # window manager on (starts off)
  remap: false                  # key remapper on (starts off)
  shell: false                  # Tile is the shell: hosts tray icons, keeps the launcher bound
  log: false

# Tile as the Windows shell; see shell.md.
shell:
  run_startup: true             # as the shell, run the Run keys and Startup folders

cursor:
  warp: true                    # move the pointer onto the window a keybind focuses, then hide it
  hide_on_key: true             # typing hides the pointer until the mouse moves or clicks
  drag: true                    # hold Win and drag a window with the left button

animations:
  enabled: true
  duration_ms: 180
  bezier: [0.0, 0.75, 0.15, 1.0]   # cubic-bezier control points (x1, y1, x2, y2)

dwindle:
  split_ratio: 0.5              # share of a split given to the existing window

master:
  mfact: 0.5                    # width of the master area
  count: 1                      # windows in the master area
  new_is_master: false          # new windows become master

scrolling:
  proportion: 0.333             # width of new columns, as a share of the monitor
  proportion_presets: [0.25, 0.333, 0.5, 1.0]   # widths `cycleproportion` steps through
  center_focused: on_overflow   # always | on_overflow | never

remap:
  modifier_guard: true          # a layer's lone Alt or Win doesn't open a menu or Start
  layers: {}                    # none; see key-remapping.md

launcher:
  width: 560                    # px at 100% scaling
  max_visible: 6                # result rows shown at once; the window is as tall as its content
  search_position: top          # top | bottom
  show_icons: true
  compact: true                 # one line per row, the description after the name
  descriptions: false           # show what each row is, besides its name
  max_results: 50
  track_usage: true             # remember launches
  sort_by_usage: true           # recent first, often used win close matches
  ignore_mouse: false
  pinned: []                    # names or ids listed first
  hide: []                      # regexes on name or id never listed
  # font: Segoe UI              # unset follows theme.font
  font_size: 11                 # points
  # corners: round              # unset follows theme.corners
  opacity: 1.0                  # 0.1 to 1.0
  # background, border, foreground, dimmed, selected and selected_foreground
  # are unset and follow theme; set any of them to override it here
  placeholder: ""                # "" for "Search apps, actions, equations and commands"
  calculator: true              # `12 * 3` shows = 36; Enter copies it
  commands: true                # offer Tile dispatchers and running the text as a command line
  system_items: true            # offer audio, microphone, network, Wi-Fi, Bluetooth, battery, power and tray icons in results
  bar: true                     # top bar: clock, date and power buttons
  fix_popups: true              # move a tray app's popup from a monitor's corner to the pointer

workspace_indicator:
  enabled: true                 # flash the workspace number when switching
  position: bottom_right        # center | top | top_left | top_right | bottom | bottom_left | bottom_right | left | right
  gap: 48                       # px from the edges, unused for center
  size: 96                      # px at 100% scaling
  duration_ms: 400              # fully shown
  fade_ms: 250                  # then fades out
  # font: Segoe UI              # unset follows theme.font
  font_size: 36                 # points
  bold: true
  # corners: round              # unset follows theme.corners
  opacity: 0.95                 # 0.1 to 1.0
  # background: "#89b4fa"       # unset follows theme.accent
  # foreground: "#1e1e2e"       # unset follows theme.accent_foreground

notifications:
  enabled: ~                    # unset follows shell mode; true/false always shows or never does
  position: top_right           # center | top | top_left | top_right | bottom | bottom_left | bottom_right | left | right
  gap: 16                       # px from the edges, unused for center
  width: 380                    # px at 100% scaling
  duration_ms: 5000             # fully shown
  fade_ms: 400                  # then fades out
  max_visible: 3                # popups stacked at once; the rest wait their turn
  show_app_name: true           # above the title
  # border: "#89b4fa"           # unset follows theme.accent
  border_width: 1               # px at 100% scaling; 0 draws no border
  filter: []                    # mute, or override the duration of, notifications from an app

binds: {}                       # added to the default binds; see keybinds.md

rules:
  # Standard dialogs float.
  - class: "^#32770$"
    float: true
    ignore: false
  # Shell windows are left alone.
  - class: '^(Windows\.UI\.Core\.CoreWindow|XamlExplorerHostIslandWindow|Shell_TrayWnd)$'
    ignore: true
  # So are PowerToys and ShareX.
  - exe: '^(PowerToys.*|ShareX)\.exe$'
    ignore: true
```
<!-- default-config:end -->
