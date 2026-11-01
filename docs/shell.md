# Shell mode

Tile can be your Windows shell: no Explorer taskbar, desktop or Start menu, just your tiled
windows. File Explorer still works for browsing folders. It is optional and off by default.

What Explorer would otherwise provide is covered like this:

| Explorer's | In shell mode |
| --- | --- |
| Tray icons (Discord, Steam, ...) | Tile hosts the notification area; the [launcher](launcher.md#system-items-and-tray-icons) lists the icons and clicks them |
| Volume, network, Wi-Fi, Bluetooth, battery, clock | Launcher rows that show and change them |
| Start menu, search | The launcher |
| Sign out, restart, shut down | Launcher rows (restart, shut down and sign out ask for Enter twice) |
| Programs started at sign-in | Tile runs the `Run` and `RunOnce` keys and the Startup folders |
| File Explorer windows | Unchanged |
| Opening Settings, the Store and other Store apps | The shell process answers for them, [below](#settings-and-store-apps) |
| The flashing taskbar button of a UAC prompt | Tile brings the prompt up itself, [below](#uac-prompts) |

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

## What you give up

- **No desktop or wallpaper.** Explorer paints both. While Tile hosts the notification area it
  covers each monitor with a plain black window at the very bottom of the stack (Windows keeps
  showing Explorer's last wallpaper otherwise), and removes them when it stops. Icons on the desktop are gone.
- **No Start menu, search or notification centre.** The launcher covers starting apps and the
  volume, network, Bluetooth, battery and clock. Windows toast notifications and tray balloons
  are not shown. Win+A still opens quick settings.
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
finds each new one, asks `ApplicationFrameHost` for a frame (the window with the title bar and
close button), puts the app in it, lifts the hiding, keeps the app the size of its frame and keeps
Windows from freezing the app while it has no visible window. Lifting the hiding needs the Immersive App
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

## UAC prompts

When a program asks for admin rights while its window isn't in front (an installer that restarts
itself elevated, a background updater, a program Tile starts), Windows doesn't show the prompt
at once. It puts a flashing "User Account Control" button on the taskbar and waits for you to
click it. With no taskbar there is nothing to click, so the prompt never appears and the program
waits. While Tile hosts the notification area it presses that button for you: the prompt comes
up on the secure desktop as soon as it is asked for, the same prompt you would get by clicking.
It still needs your consent; nothing is approved for you.

If a prompt still doesn't come up, `tile.log` says whether Tile saw one waiting ("showing a UAC
prompt that was waiting for a taskbar click").

The SmartScreen warning for a downloaded program ("Windows protected your PC", the blue window)
is a Store app that Windows starts for the check, so it needs the same support as
[Settings and Store apps](#settings-and-store-apps). When Tile frames it, `launcher.log` lists
`framed Microsoft.Windows.Apprep.ChxApp_cw5n1h2txyewy!App`.

## Getting out of trouble

If Tile crashes or you quit it, you are left on a black screen. Press `Ctrl+Alt+Del`, open Task
Manager, choose Run new task and run `explorer.exe`. To go back for good, untick Shell in the
tray (or set `tray.shell: false` in the config), then sign out and in.

The shell is only as good as the config it loads. A config with errors keeps the last good one
(or the built-in defaults), and the default binds still work, so keep `general.default_binds` on
until you are happy with your own.
