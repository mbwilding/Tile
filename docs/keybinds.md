# Keybinds

Keys come from the config's `binds`, which add to the default keys below (see
[configuration](configuration.md#binds)). Each runs a dispatcher. Most only work while Tile is on
(the tray's Tile item) and pause with it; the ones marked *always* below also work with tiling off
or paused.

Bound keys take priority over Windows' own `Win+` shortcuts; unbound ones still reach Windows.
`Win+Ctrl+Left/Right`, `Win+Ctrl+D` and `Win+Tab` are left to Windows' virtual desktops. Tile
suppresses the Start menu when a `Win` combo is handled. The key remapper sits in front of the
binds, so they see keys as remapped: a home-row `Win` works for `Win+` binds.

## Default keys

| Keys | Action |
| --- | --- |
| `Win+B` | open a new Chrome window |
| `Win+T` | open a new Windows Terminal window |
| `Win+Q` | close the window (like Alt+F4) |
| `Win+Enter` | make the window the main one (swap with master) |
| `Win+F` / `Win+O` | toggle floating / fullscreen |
| `Win+Space` | next layout |
| `Win+N` | focus the next window in layout order |
| `Win+Z` | lock the computer |
| `Win+Arrows` / `Win+H/J/K/L` | move focus (to the nearest window on the next monitor at the edge) |
| `Win+Shift+Arrows` / `Win+Shift+H/J/K/L` | move window (to the next monitor at the edge) |
| `Win+Alt+Arrows` / `Win+Alt+H/J/K/L` | resize |
| `Win+1`…`Win+9` / add `Shift` | switch to / move window to workspace 1 to 9 |
| `Win+,` / `Win+.` | focus monitor left / right |
| `Win+S` | flip the dwindle split |
| `Win+R` | open the [app launcher](launcher.md) |
| `Win+Ctrl+R` / `Win+C` | scrolling: next column width / centre column |
| `Win+[` / `Win+]` / `Win+\` | scrolling: merge into left / right column, split out |
| `Win+Ctrl+Shift+Left/Right` | move window to previous / next virtual desktop |
| `Win+Ctrl+P` | show window on all desktops (it floats) |
| `Win+Shift+R` / `Win+Shift+P` / `Win+Shift+E` | reload config / pause (or resume) tiling and remapping / exit |
| `Win` + left-drag | move the window with the mouse; drop it on a tile to swap, or on another monitor to move it there |

Mouse: dragging a tile onto another swaps them, and dragging a tile's edge resizes the split.
Releasing `Win` after a `Win` drag doesn't open the Start menu. A held key keeps the modifiers
it was pressed with, so letting go of `Ctrl` before the letter doesn't turn its auto-repeat into
a different bind.

## Writing a key

A key is `"<modifiers>+<key>"`. Modifiers are `super` (or `win`), `shift`, `ctrl` and `alt`; a
bind fires only when exactly those modifiers are held. Keys are letters, digits, `f1`..`f24`,
`left`/`right`/`up`/`down`, `space`, `tab`, `enter`, `esc`, `backspace`, `delete`, `insert`,
`home`, `end`, `pageup`, `pagedown`, `print`, `comma`, `period`, `minus`, `equal`, `semicolon`,
`slash`, `grave`, `bracketleft`, `bracketright`, `backslash`, `apostrophe`, or any Windows
virtual-key name such as `OEM_102`.

Win+L needs `override_win_l` (see [Windows integration](windows-integration.md)); without it,
binds on it are skipped.

## Dispatchers

A `binds` value is `<dispatcher> [arguments]`. `tile dispatch` runs the same ones.

| Dispatcher | Action |
| --- | --- |
| `exec <command line>` | *always*: start a program, a new instance every time |
| `focusorexec <command line>` | *always*: focus a window of the command's program (pressing again moves to the next one), or start it if there is none |
| `killactive` | *always*: close the focused window, like Alt+F4 |
| `togglefloating` / `fullscreen` | toggle floating / fullscreen |
| `movefocus <l\|r\|u\|d>` | focus the nearest window that way, or the nearest one on the next monitor |
| `movewindow <l\|r\|u\|d>` | move the window that way, to the next monitor at the edge |
| `resizeactive <dx> <dy>` | grow (or shrink, with negative values) by pixels |
| `cyclenext` / `cycleprev` | focus the next / previous window in layout order |
| `workspace <n>` | switch to workspace `n` (1-255) on the focused monitor, or on every monitor when `independent_workspaces` is off; the [workspace indicator](configuration.md#workspace_indicator) flashes its number |
| `movetoworkspace <n>` | move the window to workspace `n` and follow it |
| `movetoworkspacesilent <n>` | move the window to workspace `n` and stay |
| `focusmonitor <l\|r\|u\|d>` | focus the next monitor that way |
| `layout <name>` / `cyclelayout` | set / step through the workspace's layout |
| `swapwithmaster` | master / dwindle: swap the window with the main one (or the main one with the next) |
| `togglesplit` | dwindle: flip the direction of the window's split |
| `scroller_proportion <0.05-1.0>` | scrolling: set the column's width |
| `cycleproportion` | scrolling: next width from `proportion_presets` |
| `consumewindow <l\|r>` | scrolling: move the window into the column that way |
| `expelwindow` | scrolling: move the window into a column of its own |
| `centercolumn` | scrolling: centre the focused column |
| `desktop <n\|next\|prev>` | switch virtual desktop (past the last one creates one) |
| `movetodesktop <n\|next\|prev>` | send the window to another virtual desktop |
| `pin` | show the window on all virtual desktops (it floats) |
| `lock` | lock the computer (works while Win+L is taken over) |
| `launcher` | *always*: open or close the [app launcher](launcher.md) |
| `reload` / `togglepause` / `exit` | reload the config / pause both tiling and remapping (or resume both) / quit |
| `toggletile` / `toggleremap` | pause or resume the window manager / the key remapper on its own, like the tray's Tile and Remap items |
| `toggleshell` / `togglelog` | switch [shell mode](shell.md) / logging on or off, like the tray's Shell and Log items |

Directions can also be written `left`, `right`, `up` and `down`.
