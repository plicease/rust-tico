# Built-in themes

Every `*.toml` here is compiled into the tico binary (`BUILTIN_THEMES` in
`src/theme.rs`, via `include_str!`) and selectable by its file name, e.g.
`theme = tico-builtin-gruvbox` in `~/.ticorc`'s `[syntax]` section or
`--tico-theme tico-builtin-gruvbox`. The `tico-builtin-` prefix is reserved:
a name starting with it is never looked for on disk, so a built-in can't
be shadowed by a same-named file in a Helix theme directory. Themes
without the prefix are looked up on the filesystem instead
(`~/.config/tico/themes/`, then an installed Helix's theme directories).

The format is Helix's; see `src/theme.rs` for a summary and
<https://docs.helix-editor.com/themes.html> for the full reference. Only
syntax scopes are used — a theme's `ui.*` entries have no effect in tico.

## Which themes, and why these

`tico-builtin-default` is tico's own: a 16-color theme with no backgrounds,
taking its colors from the terminal's palette the way nano's highlighting
does. It reproduces the palette tico had hardcoded before themes existed.

The other eight are the most widely used Helix themes, picked so that each
looks clearly different from the rest rather than eight variations on the
same idea:

| Name | Character |
|---|---|
| `tico-builtin-gruvbox` | warm, retro, earthy dark |
| `tico-builtin-catppuccin_mocha` | soft pastel dark |
| `tico-builtin-dracula` | purple/pink high-saturation dark |
| `tico-builtin-nord` | cool, muted "arctic" blue dark |
| `tico-builtin-tokyonight` | deep night-blue with violet accents |
| `tico-builtin-onedark` | the classic Atom One Dark |
| `tico-builtin-monokai` | high-contrast green/yellow/pink (Sublime's classic) |
| `tico-builtin-solarized_light` | the one light-background theme |

All eight use truecolor (`#rrggbb`), so they need a terminal that supports
it; `tico-builtin-default` is the choice for a 16-color terminal.

## Provenance and licensing

| File | Source | License |
|---|---|---|
| `tico-builtin-default.toml` | Written for tico | MIT (tico's own) |
| all `tico-builtin-*.toml` others | [Helix](https://github.com/helix-editor/helix) `runtime/themes/<name>.toml`, at the commit named in each file's header; unmodified apart from that header | **MPL-2.0** — see `LICENSE-MPL-2.0` in the repository root |

MPL-2.0 is a file-scoped copyleft: each vendored file must stay under
MPL-2.0 and carry its notice (each does, in its header comment), but that
has no effect on the license of the rest of tico. To refresh a vendored
theme, replace everything below the tico header with the new upstream
contents and update the commit in the header.
