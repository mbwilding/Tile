# Layouts

Each workspace has its own layout. Set the default for new workspaces with `general.layout`,
change the current one with the `layout <name>` and `cyclelayout` dispatchers (`Win+Space`), and
the windows keep their order when you switch. Options are in
[configuration](configuration.md#layout-sections).

| Layout | Description |
| --- | --- |
| `dwindle` | A binary tree. Each new window splits the focused one along its longer side. `togglesplit` flips a split, and dragging a tile's edge changes its ratio. |
| `master` | `master.count` windows in a column on the left (`master.ratio` of the width), the rest stacked on the right. `swapwithmaster` promotes a window. `resizeactive` changes the column's width and, with a `dy`, how tall the window is against the others in its column. |
| `monocle` | Every window fills the workspace. `movefocus` steps through them in order, and past the first or last it moves on to the next monitor, as in the other layouts. |
| `even` | Every window gets an equal share: side by side on a landscape monitor, stacked as equal rows on a portrait one. Resizing does nothing, since every share is equal. |

## Portrait monitors

A new workspace on a portrait monitor starts with `even`, whatever `general.layout` says.
Rotating a monitor switches its workspace to `even` (portrait) or back to the default layout
(landscape). Only workspaces that were on `even` or are now on a portrait monitor change; a
workspace you set to another layout stays on it until the monitor is rotated.

## Workspaces and monitors

By default every monitor has workspaces of its own (`general.independent_workspaces: true`).
`workspace 2` shows workspace 2 *of the monitor you are on*, so it never jumps to another monitor,
and each monitor has its own workspace 2 with its own windows and layout. `movetoworkspace 2`
likewise sends the window to workspace 2 of its monitor. Moving a window to another monitor puts
it on the workspace that monitor is showing.

Set `independent_workspaces: false` to sync the monitors instead: switching to workspace 2
switches every monitor to its own workspace 2 at once.

Switching to an empty workspace keeps focus on that monitor: with no window to focus, Tile
focuses the desktop, so Windows doesn't hand focus to a window on another monitor and flick you
over to it.

If a monitor is unplugged, its workspaces are merged into the first monitor's workspaces with the
same numbers, so no window is lost.

## Several monitors

Windows already open when Tile starts are tiled on the monitor they are on, taken left to right then top to bottom so they
land in tiles near where they were, and new windows open on the monitor under the
mouse. `movefocus` and `movewindow` carry on to the next monitor at the edge of the current one,
landing on the window nearest the one you left.

## Mouse resizing

Drag the edge of a tiled window to resize its split. The other windows fit around it while you
drag, and everything settles into its tile when you let go. Dragging an edge on past a
neighbour keeps resizing, however far the pointer goes; it never swaps the windows. Swapping is
a plain drag of the title bar (or `Win` and drag) at the window's own size, dropped on another
tile.
