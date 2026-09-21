# tico

A nano-compatible TUI text editor written in Rust. tico matches GNU nano's
keybindings, configuration, and on-screen behavior as closely as practical,
verified against the installed `nano` binary and the nano source (checked
out separately, if available, for reference) rather than assumptions.

## Never commit or push without explicit instructions

Do not run `git commit` or `git push` (or anything else that lands work in
git history or a shared remote) unless the user explicitly asks for it in
that message — e.g. "commit and push", "commit this". Being asked to
implement or fix something is not permission to commit or push it, even
when the change is small and obviously correct; leave the working tree as
it is and say what's ready. This applies equally when the trigger for the
work was a bug report, a pasted error, or a failing CI link — investigate
and fix, but don't commit/push on your own judgment. Offering to commit
("want me to commit this?") is fine; proceeding without a yes is not.

## Never merge a pull request

Do not merge a pull request (`gh pr merge`, the GitHub web UI, or any
other route) — not even if explicitly asked to. If asked to merge a PR,
say that this is intentionally outside what you'll do here and that the
user needs to merge it themselves. This one holds even with a direct,
explicit instruction to merge; it isn't a "confirm once" situation like
commit/push above.

## Syntax highlighting: an intentional exception to nano-compatibility

Everywhere else, the goal is to match nano's real behavior. Syntax
highlighting is a deliberate exception:

- tico implements its own syntax highlighting using **tree-sitter**
  grammars (`src/syntax/`), not nano's regex-based `color`/`icolor` engine.
- nanorc's syntax-highlighting directives — `syntax`, `color`, `icolor`,
  `header`, `magic`, `include`, `extendsyntax`, and the per-syntax
  `formatter`/`linter`/`comment`/`tabgives` lines inside a `syntax` block —
  are **parsed but ignored** (see `SYNTAX_BODY_COMMANDS` in
  `src/config/nanorc.rs`). They're recognized only so the rest of a
  real-world `~/.nanorc`/`/etc/nanorc` still parses correctly; a user's
  custom syntax/color definitions have no effect in tico.
- Do not implement nanorc `color`/`icolor` parsing to actually drive
  highlighting, and do not treat "nanorc syntax directives don't work" as a
  bug to fix. If asked to make nanorc-defined highlighting work, point back
  to this file rather than implementing it.
- The built-in language list, including any per-language `linter`/
  `formatter` defaults, lives in `src/syntax/languages.rs` as a
  `LanguageDef` table (tree-sitter grammar + highlight query + optional
  linter/formatter command), sourced from nano's own shipped nanorc files
  where applicable (see git history for `justify`/`Speller-Formatter-
  Linter-Execute` work). Adding support for a new language means adding an
  entry there and a corresponding `.scm` highlight query under
  `src/syntax/queries/`, not adding nanorc color-directive support.
- Syntax colors themselves are currently a single hardcoded palette
  (`syntax_color()` in `src/ui.rs`, keyed by `HighlightKind`) — there is no
  theme system and no nanorc-driven override. If a theming mechanism is
  ever wanted, it should be a tico-specific mechanism (e.g. a small set of
  named built-in palettes, or a tico-specific config key), not a
  reimplementation of nano's `color`/`icolor` directives.
