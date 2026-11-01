# The command line

`Tile.exe` is both the window manager and its command-line tool. With no command it runs Tile;
with one it controls a Tile that is already running.

```powershell
tile dispatch workspace 3      # run any dispatcher (see keybinds.md)
tile query windows             # monitors | workspaces | windows | activewindow; --json for JSON
tile subscribe                 # JSON lines on workspace, active window and layout changes
tile reload                    # reload the config
tile check [file]              # check a config file without applying it
tile config                    # print the config file's path
tile install / uninstall       # see the README; `cargo deploy` installs a checkout's build
```

Becoming the Windows shell is the tray's Shell item (or `tray.shell` in the config, or the
`toggleshell` dispatcher), not a `tile` command; see [shell mode](shell.md).

`tile query windows` is the way to find the `exe`, `class` and `title` to write
[window rules](window-rules.md) against.

## Console and windows

`Tile.exe` is a console program, so its output works in any terminal and the prompt waits for
it. Run in a terminal, `tile` stays in the foreground with that console (Ctrl-C quits it,
undoing its changes). Started any other way (sign-in, the task, Explorer) it lets go of its
console at once. What `install` registers starts it through `conhost.exe --headless`, so no
console window appears at sign-in either.

## The pipe

The commands talk to Tile over the named pipe `\.\pipe\tile.sock`: one JSON request per line,
JSON responses back. Scripts can use it directly. Only your account (and administrators) can
connect, including from a normal terminal when Tile runs as administrator. The protocol is
defined in the `tile-ipc` crate.
