# Layouts

Each workspace has its own layout. Set the default for new workspaces with `general.layout.default`,
change the current one with the `layout <name>` and `cyclelayout` dispatchers (`Win+Space`), and
the windows keep their order when you switch. Options are in
[configuration](configuration.md#layout-sections).

| Layout | Description |
| --- | --- |
| `dwindle` | A binary tree. Each new window splits the focused one along its longer side. `togglesplit` flips a split, and dragging a tile's edge changes its ratio. |
| `master` | `master.count` windows in a column on the left (`master.proportion` of the width), the rest stacked on the right. `swapwithmaster` promotes a window. `resizeactive` changes the column's width and, with a `dy`, how tall the window is against the others in its column. |
| `monocle` | Every window fills the workspace. `movefocus` steps through them in order, and past the first or last it moves on to the next monitor, as in the other layouts. |
| `scrolling` | An endless horizontal strip of columns, scrolled to keep the focus in view, as in niri or hyprscroller. Columns can hold several windows (`consumewindow`, `expelwindow`), and columns scrolled off screen are hidden. A column at the edge shows cut off at the monitor's edge, never on the next monitor. See [Scrolling](#scrolling). |
| `even` | Every window gets an equal share: side by side on a landscape monitor, stacked as equal rows on a portrait one. Resizing does nothing, since every share is equal. |

## Scrolling

A lone window takes the whole area. Each new window opens in a column of its own to the right of the focused one, `scrolling.proportion`
of the monitor wide (half by default, so two windows split it evenly). Moving focus scrolls the strip, with the usual animation, only as far as it takes to bring the
focused column fully into view, as in niri: nothing moves while the column you focus is already
in view. `scrolling.center_focused` can centre it instead. `cycleproportion`
steps the column through `scrolling.proportion_presets`, `scroller_proportion` sets it, and
`resizeactive` changes it by pixels. `movefocus` left and right step between columns, up and
down between the windows in one, and past the first or last column it moves on to the next
monitor. `movewindow` swaps columns, or moves a window within its column.

A column partly off the monitor is cut at the monitor's edge, so on a multi-monitor desktop the
part that would reach the next monitor isn't drawn there. See
[how the cut is made](windows-integration.md#scrolling-and-monitor-edges).

## Layout per monitor

A new workspace starts with `general.layout.default`, or with the layout last used on its monitor
if [`general.layout.monitors`](configuration.md#generallayoutmonitors) has one. Tile keeps that section up to date as you
change layouts, so a portrait monitor you put on `even` stays on it.

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
