# Windows integration

How Tile fits in with Windows, and what it changes.

## Files

| Path | Contents |
| --- | --- |
| `%APPDATA%\Tile\config.yml` | [Config](configuration.md), created on first run with only the tray switches |
| `%LOCALAPPDATA%\Tile\logs` | Daily log files, while logging is on |
| `%LOCALAPPDATA%\Tile\restore.json` | What to undo if Tile dies, while it runs |
| `%LOCALAPPDATA%\Tile\launcher.json` | The [launcher](launcher.md)'s launch history, while `launcher.track_usage` is on |
| `C:\Program Files\Tile` | The installed programs |

## The tray

The menu holds Tile, Remap, Shell and Log, then Open logs and Open config, then Quit.

Tile and Remap switch the window manager and the key remapper on or off on their own, and are
remembered across restarts in the config's `tray` section. Tile starts on and Remap off. Shell
makes Tile the [shell](shell.md) (it starts off). Launch
and close binds work either way. Log switches logging on or off (nothing is logged until then, or
until `--log` is passed: `Tile.exe --log tile=debug` logs every keybind it handles).

There is no reload item: Tile reloads the config whenever the file is saved. When the saved file
has errors, Tile keeps running on the last config that loaded, the tray icon's top left tile
turns red, its tooltip shows the error, and a **Config error** item at the top of the menu opens
the config. Saving a fixed file clears it. A broken config at startup does the same, running on
the built-in defaults. `tile check` prints the full error.

Pausing Tile hands every window back as it was: shown, with its title bar and border. Resuming
starts again from the windows that are open.

## Win + drag

Holding Win and dragging with the left button moves the window under the pointer, from anywhere
on it. The click is swallowed and never reaches the app. A low-level mouse hook moves the window
directly, a few milliseconds apart, so it follows the pointer closely. When you drop a managed
window, Tile tiles it where it landed, as for a title bar drag. It doesn't work on maximised
windows, on windows of a higher privilege than Tile, or on the taskbar or desktop. Turn it off
with `cursor.drag: false`.

## Virtual desktops

Windows 11 24H2 and later. Every native virtual desktop has its own workspaces and layouts. Task
View, `Win+Ctrl+Left/Right` and "show this window on all desktops" keep working.

## Win+L

Windows handles Win+L before any keyboard hook sees it. With `override_win_l: true`, the default,
Tile sets the per-user `DisableLockWorkstation` policy while it runs, so Win+L can be bound (to
`movefocus r` by default) and the `lock` dispatcher locks instead. The policy is removed when Tile
stops, even after a crash or kill. If that policy is admin-only on your machine and Tile doesn't
run as administrator, it asks once through UAC to let your account manage that single registry
key. With `override_win_l: false`, Win+L locks as usual and binds to it are skipped.

## Start menu

Windows opens Start when Win is released with no other key pressed in between. With
`disable_start_menu: true`, Tile presses a key that does nothing just before the release, so Start
stays closed. Win combos, binds and the `launcher` dispatcher are unaffected. It is off by default.

## Quick settings

Win+A isn't bound by Tile by default, so it still reaches Windows and opens quick settings (Wi-Fi, audio and
the rest), with or without Explorer running. Use it if you need that menu. The
[launcher](launcher.md) has its own rows for the same things.

## Click to Do

On Copilot+ PCs, holding Win shows Click to Do's glow around the pointer. Tile switches it off
once and leaves it off, because Windows shows a notice every time it is switched. To get it back,
set `disable_click_to_do: false` and turn it on in Settings, Privacy & security, Click to Do.

## Admin windows

A program running without admin rights can't move admin windows or see keys pressed in them. The
installed Tile runs as administrator, so it manages them like any other window. A Tile started by
hand without admin rights (e.g. `cargo run`) leaves them alone, and its keybinds and remapping
don't work while one has focus. Programs Tile starts (`exec` and `focusorexec` binds) go through a
launcher running with your normal rights, so they don't run as administrator.

## The launcher

The [launcher](launcher.md) lists the shell's `shell:AppsFolder` (Start's All apps), takes each
icon from the shell (`IShellItemImageFactory`) and starts apps as `shell:AppsFolder\<id>`
through the normal-rights helper above. Its window is created hidden at startup, is never tiled
(Tile leaves its own windows alone) and takes focus the way `focusorexec` does. It closes when it
loses focus.

## Shell mode

With [shell mode](shell.md), Tile registers a window of class `Shell_TrayWnd` (the one apps send
`Shell_NotifyIcon` to) while no other taskbar exists. It allows `WM_COPYDATA` through UIPI, so
an elevated Tile still hears from normal apps, and broadcasts `TaskbarCreated` so apps add their
icons again. It keeps the icons and posts a click to the app's callback window as Explorer
would, first reporting the pointer over it, and allowing the app to take the foreground.

Many apps (and Tile's own tray icon) ask `Shell_NotifyIconGetRect` where an icon is before they
show a menu or ignore the click. The shell answers that with two `WM_COPYDATA` questions (the
corner, then the size, each packed into the reply), and Tile answers them with a small square
under the pointer, where the click that opens the menu was made. Without an answer such an app
shows its menu in the screen's corner, or not at all.

Apps also ask the taskbar's position with `SHAppBarMessage` (`ABM_GETTASKBARPOS`, and register
app bars, as the touch keyboard does). Tile answers those too, so the call doesn't fail: the
taskbar is said to be a small square under the pointer, and the quiet questions (is it hiding,
register, remove) get the answer of a taskbar with nothing to hide. Where the answer goes is an
address in the asking program, which Tile writes only into programs without administrator
rights, since the sender can't be told apart from another program. An elevated app (G HUB is
one) is told the call worked but gets no position. The window goes away when another taskbar
appears. Turning the Shell switch on writes the Winlogon `Shell` value under `HKCU` and sets
`SeparateProcess`; turning it off removes the value, if it is Tile's, and puts `SeparateProcess`
back. Tile checks both against the switch each time it starts and when the config changes.

How the shell process answers for Store apps (Settings, the Microsoft Store) is in
[shell mode](shell.md#settings-and-store-apps).

## The workspace indicator

The [workspace indicator](configuration.md#workspace_indicator) is a topmost layered popup made
hidden at startup on its own thread, like the launcher. It is a tool window that can't be
activated and lets clicks through (`WS_EX_NOACTIVATE`, `WS_EX_TRANSPARENT`), so it never steals
focus, and Tile never tiles it. It fades by stepping the window's layered alpha on a timer.

## Secure desktops

No program sees keys on the lock screen, UAC prompts or Ctrl+Alt+Del. The remapper resets on lock
and unlock; after a UAC prompt, a layer key released on the prompt may need pressing again. If
Tile is killed (not quit), a modifier the remapper was holding stays down until you press and
release it.

## Minimum sizes

Apps that refuse a tile smaller than their minimum size are placed with `SWP_NOSENDCHANGING`, so
they fit their tile anyway.

## Undoing changes

Tile hides windows on inactive workspaces, removes title bars, sets the Win+L policy and blanks
the system cursors to hide the pointer. It records these in `restore.json` and undoes them on
exit, pause, turning tiling off and on panic. A watchdog process (`Tile.exe --watchdog <pid>`)
undoes them if Tile is killed, and the next start cleans up anything left over. Closing the
terminal Tile was started from can kill the watchdog too, so Tile cleans up itself when that
console closes.

Held keys are released on exit, pause, reload, and when the session locks or unlocks.

Only one Tile runs per session: a second one finds the `Local\Tile` mutex taken and exits.
