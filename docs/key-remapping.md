# Key remapping

Tile has a key remapper built in: layers, home-row mods and tap/hold keys. Switch it on and off
with `general.remap` or the tray's Remap item (it starts off). The remapper has its own keyboard
hook in front of the keybinds, so binds see keys as remapped.

`remap.layers` holds the layers: each entry is a layer, named by its key (quoted when it has
`:`), that maps keys to actions. Quote keys and actions that start with punctuation, like `";"`
or `"!"`. There are no layers by default, so the remapper changes nothing until you add some.

```yaml
remap:
  layers:
    main:
      capslock: overload(nav, esc)    # tap for Escape, hold for the nav layer
      a: overloadt(control, a, 200)   # Control once held for 200ms
      rightalt: layer(nav)
    "nav:C":                          # keys nav doesn't bind get Control while it's held
      h: left
      l: right
    nav+shift:                        # while nav and shift are both active
      h: C-left
```

## Layers

| Layer name | Meaning |
| --- | --- |
| `main` | always active; its keys apply whenever no other active layer binds them |
| `<name>` | a layer of your own, active while a `layer`, `overload` or `overloadt` key holds it |
| `<name>:<mods>` | a layer whose modifiers (e.g. `C` or `C-A`) apply to the keys it doesn't bind |
| `<a>+<b>` | a composite layer, whose keys apply while both `a` and `b` are active |

Built-in layers: `control`, `shift`, `alt`, `meta` (Win) and `altgr`, each holding its modifier.
Modifier letters, for layer names and key sequences: `C`, `S`, `A`, `M` (Win) and `G` (AltGr).

Layers stack: the most recently activated layer is searched first, then the others, then `main`.
A key no active layer binds is sent as itself with the modifiers of every active layer; a bound
key is sent with its own modifiers plus those of the other active layers. Keys no layer touches
are let through untouched.

## Actions

| Action | Effect |
| --- | --- |
| `<key>` / `C-left` / `!` | send a key, with modifiers or as a shifted character |
| `layer(<name>)` | the layer is active while the key is held |
| `overload(<name>, <key>)` | the layer while held, the key if tapped alone |
| `overloadt(<name>, <key>, <ms>)` | the layer once held for `ms`, the key if released before |
| `noop` | nothing |

Keys typed while an `overloadt` key is deciding are replayed in order once it's decided, so fast
rolls type normally.

`modifier_guard` (on by default) stops a layer's lone Alt or Win press opening a menu or Start.

## Key names

Keys are physical positions named after the US QWERTY layout, not characters, and Windows'
keyboard layout applies to what Tile sends: with the Windows Dvorak layout, the key named `j`
types `h`.

The remapper and [keybinds](keybinds.md#key-names) share one list of key names, so any name works
in both: `a`, `1`, `;` or `semicolon`, `[` or `leftbrace` or `bracketleft`, `.` or `dot` or
`period`, `esc`, `capslock`, `leftshift`, `pageup` or `pgup`, `f1`… Names longer than one
character ignore case. A single uppercase letter is the shifted key (`A` sends `S-a`), and so are
shifted characters such as `!`, `{` or `?`.

## Limits

Not supported yet: `oneshot`, `toggle`, `swap`, `macro`, `overloadt2`, `timeout` and other
actions, aliases and including other files. Per-keyboard layers can't be: a low-level keyboard
hook can't tell keyboards apart, so the layers apply to every keyboard.

## Emergency exit

Holding Backspace + Escape + Enter (physical keys) turns remapping off until the config is
reloaded.
