# Windows integration

How Tile fits in with Windows, and what it changes.

## Files

| Path | Contents |
| --- | --- |
| `%APPDATA%\Tile\config.yml` | [Config](configuration.md), created on first run with only the `features` switches |
| `%LOCALAPPDATA%\Tile\logs` | Daily log files, while logging is on |
| `%LOCALAPPDATA%\Tile\restore.json` | What to undo if Tile dies, while it runs |
| `%LOCALAPPDATA%\Tile\launcher.json` | The [launcher](launcher.md)'s launch history, while `launcher.track_usage` is on |
| `%LOCALAPPDATA%\Tile\grant-win-l.ps1` | The script that asks once for permission to manage the [Win+L](#winl) policy, when it is admin-only |
| `%LOCALAPPDATA%\Tile\grant-smartscreen.ps1` | The same, for the [SmartScreen](shell.md#smartscreen) policy |
| `C:\Program Files\Tile` | The installed programs |

## The tray

The menu holds Tile, Remap, Shell and Log, then Open logs and Open config, then Quit.

Tile and Remap switch the window manager and the key remapper on or off on their own, and are
remembered across restarts in the config's `features` section. Tile starts on and Remap off. Shell
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

## Empty monitors

When the focused monitor has no window to focus (you closed its last one, or closed the launcher over
it), Windows would hand focus to a window on another monitor, and Tile would follow it. Tile puts
focus on the desktop instead and ignores the jump for half a second, so the focused monitor stays
where it was. Without Explorer there is no desktop window, so Tile keeps a one pixel, invisible window
off screen to take the focus.

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

## Window borders

Windows 11 draws a one pixel border that Tile colours with `DWMWA_BORDER_COLOR`; it can't be
made thicker. With `general.border_width` above 1, Tile also draws a ring outside each tiled
window, on a layered popup of its own (made on a thread of its own, like the indicator) that
can't be activated and lets clicks through. The ring is per-pixel alpha, so its outer and inner
edges are smooth, and its inner corners follow Windows 11's window corner radius. The ring thread reads each
window's position straight from Windows, once per compositor frame while anything is moving, so
a ring moves with its window while it animates or you drag it, whatever the daemon is doing.
Once nothing has moved for about half a second it only looks 25 times a second, and it redraws
or moves a ring only when its window actually changed, so a still desktop costs next to nothing. The ring sits
directly above its window in the stacking order, not topmost, so anything over the window is over
its ring too. It still needs a gap at least as wide beside the window, or it covers a neighbour's
edge.

## Scrolling and monitor edges

Windows has one desktop across all monitors, so a window moved part way off its monitor shows on
the next one. The scrolling layout does that all the time, and the first version showed columns
sliding in on the neighbouring monitor. Tile now cuts each window of a scrolling workspace to
its monitor's work area with `SetWindowRgn`, which works on windows of other processes. The
region is the part of the window's outer rect (its invisible resize borders and shadow
included) inside the work area, and is set after every placement and every animation frame, so
the cut follows the window as it moves. A window wholly outside gets an empty region and shows
nothing. Windows draws a window's own border and shadow outside that region, so on a cut window Tile
takes the border away (`DWMWA_COLOR_NONE`) and draws its own ring at the full `border_width`
instead. Border rings are cut the same way, to their monitor, so the ring of a fullscreen window doesn't
spill onto the next one. The cut is lifted when a window leaves the scrolling
layout, is released or when Tile exits, and the file Tile keeps for its watchdog lists cut
windows so they are freed after a crash too.

Some apps (File Explorer, Windows Terminal) reset or ignore a window region when they resize,
so the cut alone leaks. When Tile paints the desktop itself (shell mode), it also hides the
overflow behind the other monitors' backdrops. Stacking order is one list for every monitor, so
Tile puts the scrolling monitor's backdrop at the very bottom and each of its scrolling windows
directly beneath the lowest of the other backdrops: over its own backdrop and under theirs, so
whatever reaches the next monitor is covered by that monitor's wallpaper. Windows raises a
window when it takes focus, so Tile puts it back on every focus change, and a window that stops
scrolling is raised again so it isn't left under a backdrop. This works while one monitor
scrolls. With two, each would have to be under the other's backdrop, which can't be done, so
Tile stacks nothing and only the cut is left. Without Tile's backdrop (Explorer paints the
desktop) there is nothing to hide behind either.

A column only shows while any of it is on the monitor. One that scrolls into view starts just
past the monitor's edge and slides in under the cut, and one that scrolls out slides off the edge
and is hidden there, as a window on another workspace is. With animations off they just appear
and disappear. On monitors with different scaling, an app may rescale for the moment it is
partly over the other monitor.

## Notifications

A popup, top-right by default, shows for each new Windows toast notification while Tile hosts the
shell, since that is when Explorer's own `shellexperiencehost.exe` popups don't show (only the
registered shell can drive them, and Tile doesn't reimplement that; see
[shell mode](shell.md#what-you-give-up)). [`notifications.enabled`](configuration.md#notifications)
overrides the default: `true` always shows popups, `false` never does, even while hosting. It is
styled like the [launcher](launcher.md)'s own cards (rounded, bordered panels) rather than the
plain [workspace indicator](#the-workspace-indicator), and up to
[`notifications.max_visible`](configuration.md#notifications) show stacked at once, the rest
queued. `togglednd` mutes popups without touching Windows' own notification or Focus Assist
settings; [`notifications.filter`](configuration.md#notifications) does the same per app, or
overrides one app's duration, by name.

The popup shows the posting app's icon, the same one Windows shows for it elsewhere, with the app
name centred underneath it, and the title and body to the right. Each notification carries the
app's AUMID (`UserNotification.AppInfo().AppUserModelId()`), and the popup resolves its icon from
`shell:AppsFolder\<aumid>` through `tile_shell::shell_icon`, the same `IShellItemImageFactory`
lookup the [launcher](launcher.md) uses for its own rows, so both surfaces draw the identical icon
for a given app rather than Tile having two icon-resolution paths. A notification with no AUMID,
or an AUMID `shell_icon` can't resolve, shows with no icon: the app name goes back to being a line
above the title instead of a caption under the icon, so its identity is never dropped, and the
text keeps the width the icon column would have used. Checked by hand with
`scripts/send-test-toast.ps1`: Windows Terminal, a Win32 console app with a path-shaped AUMID, and
most Store apps all resolved; a couple of system apps' own AUMIDs
(`Windows.Defender.SecurityCenter`) did not, most likely because their real AUMID needs a suffix
this one lacks.

Clicking a popup dismisses it and, with an AUMID, brings that app to the front (`launch::start_app`,
the same call a launcher row's click makes). It can only do that much: `UserNotificationListener`
carries the app, title and body, not the launch arguments the original toast would have activated
the app with, and no other app can legitimately reach those either, so a click can never jump to
the exact thing the notification was about.

Tile reads notifications with
`Windows.UI.Notifications.Management.UserNotificationListener`, the API behind Focus Assist's
priority list and notification-reading apps, rather than Explorer's path. This was checked by
hand before writing any of it: from an unpackaged, unsigned build with no app manifest or package
identity, `GetAccessStatus` reports `Allowed` outright and `GetNotificationsAsync` returns full
notification content (app, title, body). Microsoft's docs describe a `userNotificationListener`
restricted capability, normally a Store packaging concern, but an unpackaged desktop process
already runs with the signed-in user's full rights, which this read-only API accepts in place of
it; no sparse MSIX package or Store listing was needed.

The live `NotificationChanged` event is a different story: subscribing to it fails outright with
`0x80070490` ("Element not found"). Two further pushes were tried, rather than settling for
polling straight away:

- A throwaway test binary was given a sparse MSIX package (a manifest with app identity but no
  real packaging payload, the usual way an unpackaged Win32 process gets WinRT identity), signed
  with a self-signed test certificate, built entirely outside the Tile source tree. Packaging and
  signing worked, but installing the package failed with `0x800B0109` ("the root certificate ...
  must be trusted"): the self-signed certificate's issuer isn't a trusted root. Trusting it as a
  root CA is a real, if small, weakening of the machine's trust store, so that step wasn't taken,
  and the test was left there rather than pushed through. Package identity, and so whether
  `NotificationChanged` would subscribe, was never confirmed either way.
- Windows' own shell wakes for new notifications via WNF (Windows Notification Facility), an
  undocumented publish/subscribe mechanism with some state names catalogued by community
  reverse-engineering tools (SharpWnfSuite, the WNFUN talk, Quarkslab's WNF write-up). None of
  those sources tie a specific, named WNF state to "a toast was posted or changed": the
  `WNF_SHEL_*` names found in public dumps cover other shell activity (app launches and the like),
  not notifications. Tile already uses undocumented NT APIs elsewhere (`tile-shell`'s frame code,
  for `NtUserAcquireIAMKey`) with a documented fallback, so there is precedent for this if a
  genuine candidate turned up — but subscribing to a guessed state name with no evidence it fires
  for notifications isn't that, so none was tried.

So Tile polls `GetNotificationsAsync` every two seconds instead and works out what is new by id;
new notifications land as a popup within that couple of seconds, not instantly. Two seconds is a
deliberate trade-off, not a technical floor: it is one cheap, asynchronous WinRT call, so a
shorter interval is possible if the delay matters more than the (small) extra overhead. It stays a
private constant (`POLL_INTERVAL` in `tile-sys/src/notifications.rs`) rather than a setting, so
it's easy to tune later without it being something most users would ever need to touch.

The popup window is a topmost layered popup on a thread of its own, the same shape as the
[launcher](launcher.md) and the [workspace indicator](#the-workspace-indicator): it never steals
focus, and clicking a card dismisses it early. Hovering a card pauses its countdown (shown as a
thin progress bar) until the pointer leaves.

As [shell mode](shell.md#what-you-give-up) already documents, Action Center (Win+N) is one of the
things Tile gives up in shell mode: it is `ShellExperienceHost.exe`-driven, the same host behind
the Store app frames and Explorer's own toast popups, none of which run without Explorer. With no
Action Center to fall back on, a dismissed or missed popup currently has nowhere to be reviewed in
shell mode; a small in-memory notification history shown from the launcher would close that gap,
but wasn't built in this change.

## Secure desktops

No program sees keys on the lock screen, UAC prompts or Ctrl+Alt+Del. The remapper resets on lock
and unlock; after a UAC prompt, a layer key released on the prompt may need pressing again. If
Tile is killed (not quit), a modifier the remapper was holding stays down until you press and
release it.

## Minimum sizes

Apps that refuse a tile smaller than their minimum size are placed with `SWP_NOSENDCHANGING`, so
they fit their tile anyway.

## Undoing changes

Tile hides windows on inactive workspaces, removes title bars, sets the Win+L policy, turns off
the [SmartScreen](shell.md#smartscreen) policy and blanks the system cursors to hide the pointer.
It records these in `restore.json` and undoes them on exit, pause, turning tiling off and on
panic. A watchdog process (`Tile.exe --watchdog <pid>`)
undoes them if Tile is killed, and the next start cleans up anything left over. Closing the
terminal Tile was started from can kill the watchdog too, so Tile cleans up itself when that
console closes.

Held keys are released on exit, pause, reload, and when the session locks or unlocks.

Only one Tile runs per session: a second one finds the `Local\Tile` mutex taken and exits.
