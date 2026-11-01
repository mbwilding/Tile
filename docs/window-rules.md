# Window rules

Each `rules` entry matches windows by `exe`, `class` and/or `title`, all case-insensitive
regular expressions; every matcher given must match. Rules apply in order, and a later match
overrides an earlier one. Writing any `rules` replaces the default rules. Single-quote the
patterns, so a backslash stays a backslash.

```yaml
rules:
  - exe: '^Spotify\.exe$'
    workspace: 9
  - class: '^#32770$'
    float: true
```

| Action | Effect |
| --- | --- |
| `float: true` | float instead of tiling (or `false` to tile a window that would float) |
| `workspace: n` | open on workspace `n` of the monitor it opens on (or the one a `monitor` rule names) |
| `monitor: n` | open on monitor `n` (from 0, left to right) |
| `decorations: true` | force the title bar on or off, overriding `general.decorations` |
| `ignore: true` | leave the window alone entirely |

`tile query windows` shows the `exe`, `class` and `title` of every managed window. The default
rules float standard dialogs and ignore shell windows, PowerToys and ShareX. Dialogs and
fixed-size windows float even without a rule.

Without a `monitor` rule, windows already open at startup go to the monitor they are on, and new
ones to the monitor under the mouse.
