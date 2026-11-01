# Launcher

A search box for starting apps, like wofi or Noctalia's launcher: a window centred on the focused
monitor that lists everything Start lists and starts what you pick.

It opens on Tile's focused monitor, which is right even when that monitor has no windows. With
tiling off or paused, it opens on the monitor of the focused window (whatever that window is), or
else the monitor under the pointer.

Open it with the `launcher` dispatcher. It isn't bound by default; bind it in your config, for
example to `Win+R` (a bound `Win` key replaces Windows' own shortcut, here the Run dialog):

```yaml
binds:
  super+r: launcher
```

`tilectl dispatch launcher` opens it too. It works with tiling off or paused. Pressing the bind
again closes it.

This is not the `tile.exe --launcher` helper that starts programs with your normal rights; the
launcher uses that helper too, so apps it starts never run as administrator, unless you ask for it
(see [running as administrator](#running-as-administrator)).

## What it lists

Every app in Start's All apps list (`shell:AppsFolder`): desktop programs and Store apps alike,
with their icons. The list is read again in the background each time the launcher opens, so new
installs show up. Hide entries you never want (uninstallers, help files) with `launcher.hide`,
regexes matched against the app's name or id.

Each row shows the app name and, unless `compact` is on, its id: the AppUserModelID of a Store
app or the path of a desktop program.

## Searching

Type to filter. Matching is fuzzy: the letters must appear in order, not next to each other, and
case doesn't matter. Matches are ranked as in Noctalia:

- A match on the app name always beats a match on its id.
- A name that starts with what you typed gets a bonus.
- Letters that start words (`vs` in **V**isual **S**tudio) and runs of consecutive letters score
  higher than scattered ones.
- With `sort_by_usage` on, apps you launch often win close matches, without beating a much
  better match.

Before anything is typed the list shows `pinned` apps first, in their order, then (with
`sort_by_usage`) the 20 most recently launched, newest first, then everything else by name.

### Running as administrator

**Right click** an app for a menu with **Open** and **Run as administrator**, or press
**Shift+Enter** on the selected app. Windows asks for consent unless Tile already runs elevated.
Store apps can't be elevated, so for those it behaves like Open.

## Calculator

Type a sum and its value is the first row: `12 * 3` shows `= 36`. Enter (or a click) copies the
value to the clipboard and closes the launcher. Apps that match the same text are listed under it.

A search counts as a calculation when it has a digit and is more than a bare number, so `7zip`
or `42` stay app searches. It understands:

- `+`, `-`, `*` (or `x` or `×`), `/` (or `÷`), `%` (remainder) and `^` (or `**`), with
  brackets; spaces don't matter, so `5x5` and `5 * 5` are the same
- numbers like `1.5`, `1_000`, `2.5e3` and hex `0x1f` (`0x` is always hex, never times zero)
- `pi`, `tau` and `e`
- `sqrt`, `cbrt`, `abs`, `ln`, `log` (base 10), `log2`, `exp`, `sin`, `cos`, `tan`, `asin`,
  `acos`, `atan` (radians), `floor`, `ceil` and `round`, as `sqrt(2)`

A sign binds looser than `^`, so `-2^2` is -4. Multiplication is never implied: write `2 * pi`,
not `2pi`. Set `calculator: false` to switch it off.

## Commands

There is no prefix: commands are results like any other, after the apps and system items. What
you type can match:

- **A Tile dispatcher**, as in a bind: `workspace 3`, `layout scrolling`, `reload`. Typing part
  of a name (`work`) lists the dispatchers that start with it; picking one that takes an
  argument puts its name in the search box for you to finish (`workspace `), and picking one
  that doesn't runs it. Dispatchers that act on a window act on the one that had focus before
  the launcher opened.
- **A command line**, like the Run dialog: `notepad notes.txt`, `\\server\share`, a path or a
  URL. A **Run command** row appears last when the text has a space in it (and is not a sum), or
  when nothing else matched. It runs as `exec` does, through `start` and with your normal
  rights.

Set `commands: false` to leave both out.

## System items and tray icons

The launcher also finds what Explorer's taskbar tray would show. They appear after the apps when
what you type matches them (`vol`, `bluetooth`, `shut`). The bar's buttons list them all. Set
`system_items: false` to leave them out of the results.

| Row | Enter does |
| --- | --- |
| **Volume: 45%** | mutes or unmutes the default output |
| **Volume up** / **Volume down** | steps the volume by 10% |
| **Set volume to 30%** (type `volume 30`) | sets the volume |
| **Output: Headphones (RME Babyface Pro)** | makes that device the output, for every role (default, media and communications); the one in use says **In use** |
| **Microphone: 70%** | mutes or unmutes the default input |
| **Microphone up** / **Microphone down** | steps the input level by 10% |
| **Set microphone volume to 30%** (type `mic 30`) | sets the input level |
| **Input: SM7b (RME Babyface Pro)** | makes that device the input |
| **Network: Home** | opens the network settings |
| **Wi-Fi: Home** | for each saved network in range, connects to it; the connected one disconnects (new networks need a password, so use **Wi-Fi settings**) |
| **Wi-Fi** and **Bluetooth** | turn the radio on or off (only on a machine that has one) |
| **Wi-Fi settings**, **Bluetooth devices** | open the Settings page, to join a network or to pair and connect a device |
| **Battery: 80%** | opens the battery settings (only on a machine with a battery) |
| **Clock** | opens the date and time settings |
| **Lock**, **Sleep** | do it now |
| **Sign out**, **Restart**, **Shut down** | ask for Enter again first, as they lose unsaved work |

The rows are read in the background when the launcher opens, so the current values show up a
moment after it does. The device lists are what is plugged in and enabled right now; disabled
devices are left out. Bluetooth devices can't be connected from here, only through Settings.

### Tray icons

When Tile hosts the notification area ([shell mode](shell.md)), the apps' tray icons sit in a
row of their own under the bar at the top of the launcher, centred, for as long as it is open.
The search box and the results are under that. The window
widens to hold them when there are more than its width fits. With Explorer's taskbar running,
Explorer owns the tray and there are none to show.

- **Hover** an icon to see its name.
- **Click** it as you would on a taskbar. The launcher closes and the app opens or focuses.
- **Right click** it for the app's own menu. The launcher stays open behind the menu and comes
  back to the front when the menu closes, unless your choice brought another window forward.

Tray icons are tiny, usually 16 or 32 pixels. One at least as big as the size shown is shrunk
with a smoothing filter. A smaller one gives way to the icon of the app's program, which Windows
has at larger sizes (up to 64 pixels is asked for, then shrunk to fit), and failing that it is
grown with a smoothing filter instead of being stretched into blocks. Only the size shown is kept
in memory.

Some apps (G HUB is one) place the popup of their tray icon by finding the taskbar, which isn't
there, and open it in the corner of the monitor. For two seconds after a click Tile watches the
app for that: a small borderless window, not showing before the click, whose top left corner is
exactly a monitor's, is moved next to the pointer (above it, or below if there is no room),
kept on that monitor. Nothing else is touched. `fix_popups: false` switches it off.

They are also found by typing, like the other items: Enter clicks the icon, and Shift+Enter
right clicks it, which also leaves the launcher open.


## The bar

At the top of the launcher is a bar (`bar: false` hides it). On the left it shows the time in
bold with seconds, ticking while the launcher is open, and the date; they never change to show
anything else. On the right are icon buttons, drawn with Windows' own icons (Segoe Fluent Icons,
or Segoe MDL2 Assets on an older Windows): four that open controls, then five for the session.

### Controls

The first four buttons, a speaker, a microphone, Wi-Fi and Bluetooth, are switches that show
their state: lit while sound is on, the microphone is live, or the radio is on, and dim while it
is muted or off (also dim when the machine has none). A **left click** flips the switch. A
**right click** replaces the results with a list you can pick from, like the taskbar's quick
settings, with a line under the button while it is open. Move with the arrow keys or the mouse
and press Enter or click; Esc, or a right click on the same button again, goes back to the
results. A search in the box narrows the list.

| Button | Left click | List (right click) |
| --- | --- | --- |
| **Speaker** | mute or unmute | the volume and its mute, up and down, and every output device, the one in use marked |
| **Microphone** | mute or unmute | the input's level and its mute, up and down, and every input device |
| **Wi-Fi** | radio on or off | the radio's on and off, the saved networks in range (the connected one disconnects, the others connect), and a link to Wi-Fi settings |
| **Bluetooth** | radio on or off | the radio's on and off, the paired devices with whether each is connected, and a link to Bluetooth settings |

Picking an output or input device makes it the default for every role. The launcher stays open
after a change so you can make more, and the list catches up a moment later; opening Settings
closes it. Windows has no way to connect a Bluetooth device from outside Settings, so a paired
device's row opens Settings.

### Session buttons

**Switch user**, **Log out**, **Lock**, **Restart** and **Shut down**. Switch user leaves your
session running and shows the sign-in screen (`tsdisconnect`). Log out, Restart and Shut down
lose unsaved work, so the first click lights the button and a second click does it; moving off
the button takes the question back. The window widens to hold the bar when it is wider than the
launcher.

## Keys

| Key | Action |
| --- | --- |
| `Enter` | start the selected row (run a command, copy a sum, change a system item, click a tray icon found by typing) |
| `Shift+Enter` | run the selected app as administrator, or right click the selected tray icon found by typing |
| `Esc` | close |
| `Up` / `Down`, `Ctrl+K` / `Ctrl+J`, `Ctrl+P` / `Ctrl+N` | select the previous / next row |
| `Tab` / `Shift+Tab` | next / previous row |
| `Page Up` / `Page Down` | move a page |
| `Left` / `Right`, `Ctrl+H` / `Ctrl+L` | move the caret a character |
| `Ctrl+Left` / `Ctrl+Right` | move the caret a word |
| `Home` / `End` | move the caret to the start / end |
| `Backspace` / `Delete` | delete the character before / after the caret |
| `Ctrl+Backspace` / `Ctrl+W` | delete the word before the caret |
| `Ctrl+U` | delete everything before the caret |

Keys you remap to the arrows (a `remap` layer) work the same as the arrows.

With the mouse, hovering selects, clicking starts, the wheel scrolls, and the tray row takes left and right clicks (unless `ignore_mouse` is
on). Clicking anywhere else, or switching to another window, closes it.

## Launch history

With `track_usage` on, each launch is counted in `%LOCALAPPDATA%\Tile\launcher.json`, which
holds the launch count per app and the 20 most recent. Delete the file to forget them.

## Appearance

The window is exactly as tall as what it shows: the bar, the tray icons, the search box and as
many result rows as there are, up to `max_visible` (6 by default), so it shrinks as the matches
narrow and grows again as you delete. It keeps its top where it is, and it opens centred for the
full number of rows. More results than rows scroll.

Width, the position of the search box, font, colours, corners and opacity are all in the
[`launcher` section](configuration.md#launcher) of the config. The font, corners and colours
follow the shared [`theme`](configuration.md#theme) unless set there. Sizes are in pixels at 100%
scaling and grow with the monitor's scaling. Changes apply the next time it opens.
