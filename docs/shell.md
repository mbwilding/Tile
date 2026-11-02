# Shell mode

Tile can be your Windows shell: no Explorer taskbar, desktop or Start menu, just your tiled
windows. File Explorer still works for browsing folders. It is optional and off by default.

What Explorer would otherwise provide is covered like this:

| Explorer's | In shell mode |
| --- | --- |
| Tray icons (Discord, Steam, ...) | Tile hosts the notification area; the [launcher](launcher.md#system-items-and-tray-icons) lists the icons and clicks them |
| Volume, network, Wi-Fi, Bluetooth, battery, clock | Launcher rows that show and change them |
| Start menu, search | The launcher |
| Sign out, restart, shut down | Launcher rows |
| Programs started at sign-in | Tile runs the `Run` and `RunOnce` keys and the Startup folders |
| File Explorer windows | Unchanged |
| Opening Settings, the Store and other Store apps | The shell process answers for them, [below](#settings-and-store-apps) |
| SmartScreen's warning for an unrecognised download | Can't be shown; Tile turns the check off instead, [below](#smartscreen) |

## Turning it on

```powershell
tile install          # the shell has to live somewhere that stays
```

Then tick **Shell** in the tray menu, run the `toggleshell` dispatcher, or set `shell: true`
under `tray` in your config. It takes effect at your next sign-in. Ticking it sets your user's
Winlogon `Shell` value to `Tile.exe --shell`, which needs no admin rights and affects only your
account, and turns on "Launch folder windows in a separate process" so a folder window doesn't
bring Explorer's taskbar back. Unticking it puts Explorer back from the next sign-in.

The switch is the config's `tray.shell`, and Tile keeps the sign-in shell in step with it every
time it starts and whenever the file changes. So a config with `shell: true` is enough on a
first run, and a config set back to `false` restores Explorer.

The launcher is how you start everything in shell mode, so in shell mode it is always bound. If
you haven't bound the `launcher` dispatcher yourself, Tile binds it to `Win+R`, or to `Win+D` if
`Win+R` is taken. The tray icons are a row of their own at the top of the launcher, under its bar.

### Without signing out

The tray's **Shell** item (next to Tile and Remap, remembered in `tray.shell`) makes a running
Tile act as the shell whenever no taskbar is running. With it on, end
Explorer (`taskkill /f /im explorer.exe`) and Tile starts hosting the notification area within a
couple of seconds. Apps that were running add their tray icons again when Tile announces the new
taskbar. If Explorer's taskbar comes back, Tile hands the notification area back to it. This does not
run the sign-in programs, since Explorer already did.

## What Tile does as the shell

**Notification area.** Apps add their tray icons by sending a message to the window of class
`Shell_TrayWnd`. Tile creates that window when no taskbar exists and keeps the icons it is sent.
Opening one in the launcher passes your click on to the app the way Explorer does: Enter is a
left click and Shift+Enter (or the right mouse button) is a right click, so the app shows its
window or menu. The menu appears where the pointer is.

**Sign-in programs.** When Tile starts and no taskbar is running, it runs what Explorer would:
`RunOnce` (removing each entry as it runs), then `Run`, from `HKLM` and `HKCU`, then the shared
and your own Startup folders. Entries switched off in Task Manager are skipped, and so is Tile's
own `--launcher` entry, because the shell instance already does that job. It happens once per
sign-in session, so restarting Tile doesn't start everything twice. `shell.run_startup: false`
turns it off.

**Admin rights.** `tile install` runs Tile as administrator through a sign-in task. As the
shell, Windows also starts `Tile.exe --shell` with your normal rights. That copy serves as the
helper that starts programs without admin rights and waits for the elevated Tile. If the sign-in
task never starts Tile within 30 seconds, the helper runs Tile itself, without admin rights, and
logs it to `launcher.log`.

## The desktop

While Tile hosts the notification area it covers each monitor with a window at the very bottom of
the stack (Windows keeps showing Explorer's last wallpaper otherwise), and removes them when it
stops. They are black by default. The [`wallpaper`](configuration.md#wallpaper) section sets a
plain colour, or an image from a path or a folder, fitted to each monitor, and can change it on a
timer:

```yaml
wallpaper:
  color: "#101018"
  directory: C:\Users\you\Pictures\Wallpapers
  fill_mode: crop
  cycle:
    enabled: true
    interval_seconds: 600
    order: random
```

It is a setting in the config only, with nothing for it in the tray or the launcher. It doesn't
touch the wallpaper Windows has saved, so that is back as it was with Explorer. With Explorer
running Tile paints nothing.

## What you give up

- **No Explorer desktop.** Explorer paints the wallpaper and the desktop icons. While Tile hosts
  the notification area it paints its own [desktop](#the-desktop), black unless you say otherwise.
  Icons on the desktop are gone.
- **No Start menu, search or notification centre.** The launcher covers starting apps and the
  volume, network, Bluetooth, battery and clock. Windows' own toast popups and tray balloons,
  drawn by `shellexperiencehost.exe`, are still not shown: that is Explorer-only, like the Store
  app frames [below](#settings-and-store-apps). Tile can instead read every posted notification
  through `UserNotificationListener` and show its own popups; see
  [Notifications](windows-integration.md#notifications). Win+A still opens quick settings.
- **Alt+Tab, Task View and snapping** are Explorer's; they may be missing or odd. Tile's
  workspaces replace most of that.
- **Win+E** and the Win key shortcuts that Explorer handles stop working. Bind your own, for
  example `super+e: exec explorer.exe`.

Opening a folder with `explorer.exe C:\path` gives a window on its own when folder windows run
in a separate process, which the Shell switch sets up.

## Settings and Store apps

Settings, the Microsoft Store, `ms-settings:` links and other Store apps need the shell's help to
open: Windows starts them through `sihost.exe`, which then waits for the shell to say the app's
window is ready. Explorer answers; with no Explorer a launch hangs for 45 seconds and fails with
error 0x80040900 (or "class not registered" or a "File system error").

When Windows starts Tile as the shell (`Tile.exe --shell`, the copy that runs with your normal
rights), that process answers instead. It creates the same component Explorer does, the shell's
`Windows.Internal.Shell.UwpWindowLifecycleManagement.UwpWindowLifecycleManager`, on a
CoreMessaging session of its own: it makes the session with `CoreUICreateEx`, calls the
manager's static `Create` from the session's thread (which opens it with `CoreUIOpenExisting`),
then runs the session. Nothing else of Explorer runs.
This is on whenever Tile is your sign-in shell, and it stands aside if
Explorer owns the taskbar.

A Store app's window starts hidden by the shell and has no title bar, so the same process also
finds each new one (window events report it as it is created, shown or hidden, and a look over
every window once a second catches any missed), asks `ApplicationFrameHost` for a frame (the window with the title bar and
close button), puts the app in it, lifts the hiding, keeps the app the size of its frame and keeps
Windows from freezing the app while it has no visible window. Apps that Windows keeps running in the background (Settings is one) already
have a hidden window when Tile starts; those are left hidden until they take focus, so a restart of Tile
doesn't open them. Lifting the hiding needs the Immersive App
Management key Windows only gives the shell window's owner, so the process registers a hidden
window as the shell window. It also registers a stand-in for the `ImmersiveShell` class, which Store apps ask for, that
gives them no services.

The frame host reports the close button, `Alt+F4` and the system menu's Close to this process,
which asks the app to close (and ends it if it hasn't after three seconds); the frame goes with it.

Without Explorer nothing tells a Store app how big its window is, so one would lay itself out for
the whole monitor and never follow its frame. The same process therefore sends each app its
frame's size whenever it changes, with `UniversalAppModelInProc`'s `SetBounds` and
`SetVisibleBounds`, which only answer the shell process. The frame also
gets its app's title, which is what lets Tile manage and tile it like any other window, and
frames with no app in them (left by an earlier helper, or the frame host's spares) are destroyed so
they don't show up as empty blue windows.

Known limit: some images that come from the shell are missing, such as the device picture on
Settings' System page.

It rests on undocumented parts of Windows 11 (found from build 26100) and a Windows update may
change them. If it can't start, `launcher.log` says why and Store apps behave as they would
without it: they hang and fail. Everything else keeps working. Plain programs never need it
(`snippingtool.exe /clip`, `control bthprops.cpl`, `ncpa.cpl` and `mmsys.cpl` always work).

The launcher starts a Store app by asking Windows' activation manager for it by id, from the
process with your normal rights. It doesn't use `shell:AppsFolder` for these (the way Start would),
because that route asks for a COM class only Explorer registers and fails with "class not
registered". Plain programs still start the usual way. A launch that arrives before that process
is up waits a few seconds for it instead of running as administrator.

It runs in the helper process that starts programs with your normal rights: the copy Windows
starts at sign-in, and the one Tile starts again when it restarts, as long as Tile is your
sign-in shell. So updating Tile with `cargo deploy` needs no sign-out.

## SmartScreen

SmartScreen's "Windows protected your PC" warning for an unrecognised downloaded program is shown
through a shell service only Explorer provides. Without it the check still runs but has nowhere
to ask, so the launch is just cancelled rather than offered. Rather than leave every download
silently blocked, Tile turns the `EnableSmartScreen` policy off (machine-wide) while it hosts the
notification area, and puts back whatever value was there before as soon as it doesn't, including
when Tile quits. Windows Defender itself is unaffected: this is only the download reputation
check.

The policy is admin-only on some machines. If so, Tile asks once for permission (one UAC prompt,
via `%LOCALAPPDATA%\Tile\grant-smartscreen.ps1`) the first time it starts hosting the shell, the
same way it does for [Win+L](windows-integration.md#winl). Until that is accepted, downloaded programs are still blocked
with no warning, as they were before.

## Getting out of trouble

If Tile crashes or you quit it, you are left on a black screen. Press `Ctrl+Alt+Del`, open Task
Manager, choose Run new task and run `explorer.exe`. To go back for good, untick Shell in the
tray (or set `tray.shell: false` in the config), then sign out and in.

The shell is only as good as the config it loads. A config with errors keeps the last good one
(or the built-in defaults), and the default binds still work, so keep `general.default_binds` on
until you are happy with your own.
