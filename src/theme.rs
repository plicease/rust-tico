//! Syntax-highlighting themes in Helix's theme format.
//!
//! tico deliberately does not implement nano's `color`/`icolor` directives
//! (see `CLAUDE.md`); instead it reads the same TOML theme files Helix
//! does, so any Helix theme (its own `runtime/themes/*.toml`, or one written
//! for it) can be dropped in unchanged. The format, in brief:
//!
//! ```toml
//! inherits = "some_other_theme"        # optional
//!
//! "comment" = "gray"                    # a bare string sets just `fg`
//! "keyword" = { fg = "blue", bg = "#202020", modifiers = ["bold"] }
//! "keyword.control.import" = { fg = "purple", underline = { color = "red", style = "curl" } }
//!
//! [palette]                             # optional named colors
//! purple = "#b48ead"
//! ```
//!
//! Keys are dotted scope names (`keyword.control.import`); a capture is
//! styled by the longest defined prefix, so a theme that only defines
//! `keyword` still covers every `keyword.*` capture. Colors are palette
//! names, the sixteen terminal color names (`default`, `black`, `red`,
//! `green`, `yellow`, `blue`, `magenta`, `cyan`, `gray`, `light-red`,
//! `light-green`, `light-yellow`, `light-blue`, `light-magenta`,
//! `light-cyan`, `light-gray`, `white`), `#rrggbb`/`#rgb` hex, or a bare
//! 0-255 index. The color-name -> escape-code mapping follows Helix's own
//! (`gray` is bright black, `light-gray` is normal white, `white` is bright
//! white), so a theme looks the same in both editors.
//!
//! Only syntax scopes matter to tico: a theme's `ui.*` entries, `rainbow`
//! array and diagnostic scopes are parsed (so a real Helix theme loads
//! without complaint) but have no effect, since tico's title/status bars,
//! line numbers and selection follow nano's own `set titlecolor` & co.

use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
    SetUnderlineColor,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::syntax::{HighlightSpan, Scope};

/// The name of the theme used when nothing is configured: tico's own
/// 16-color palette (the pre-theme hardcoded colors, plus a few scopes
/// that palette couldn't express), which takes its actual colors from the
/// terminal's palette the way nano's highlighting does.
pub const DEFAULT_THEME_NAME: &str = "tico-builtin-default";

/// The name prefix reserved for themes compiled into the binary; a name
/// starting with it is never looked for on disk.
pub const BUILTIN_PREFIX: &str = "tico-builtin-";

/// The themes compiled into the binary, so they need no runtime directory
/// and `theme = "tico-builtin-gruvbox"` works on any install. All but the
/// default are vendored unmodified from Helix (MPL-2.0; see
/// `themes/README.md`), chosen as the most widely used ones while keeping
/// each visibly distinct from the others: warm retro, pastel, purple,
/// arctic, night-blue, classic Atom, high-contrast, and one light theme.
/// Keys are the full `tico-builtin-*` name, identical to the file's stem
/// under `themes/` (a test checks the two sets match).
const BUILTIN_THEMES: &[(&str, &str)] = &[
    (
        DEFAULT_THEME_NAME,
        include_str!("../themes/tico-builtin-default.toml"),
    ),
    (
        "tico-builtin-catppuccin_mocha",
        include_str!("../themes/tico-builtin-catppuccin_mocha.toml"),
    ),
    (
        "tico-builtin-dracula",
        include_str!("../themes/tico-builtin-dracula.toml"),
    ),
    (
        "tico-builtin-gruvbox",
        include_str!("../themes/tico-builtin-gruvbox.toml"),
    ),
    (
        "tico-builtin-monokai",
        include_str!("../themes/tico-builtin-monokai.toml"),
    ),
    (
        "tico-builtin-nord",
        include_str!("../themes/tico-builtin-nord.toml"),
    ),
    (
        "tico-builtin-onedark",
        include_str!("../themes/tico-builtin-onedark.toml"),
    ),
    (
        "tico-builtin-solarized_light",
        include_str!("../themes/tico-builtin-solarized_light.toml"),
    ),
    (
        "tico-builtin-tokyonight",
        include_str!("../themes/tico-builtin-tokyonight.toml"),
    ),
];

/// Text attributes a style can carry (Helix's `modifiers` array plus its
/// `underline.style` variants), as a bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers(u16);

impl Modifiers {
    pub const BOLD: Modifiers = Modifiers(1 << 0);
    pub const DIM: Modifiers = Modifiers(1 << 1);
    pub const ITALIC: Modifiers = Modifiers(1 << 2);
    pub const SLOW_BLINK: Modifiers = Modifiers(1 << 3);
    pub const RAPID_BLINK: Modifiers = Modifiers(1 << 4);
    pub const REVERSED: Modifiers = Modifiers(1 << 5);
    pub const HIDDEN: Modifiers = Modifiers(1 << 6);
    pub const CROSSED_OUT: Modifiers = Modifiers(1 << 7);

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }

    pub fn any(bold: bool, italic: bool, reversed: bool) -> Modifiers {
        let mut m = Modifiers::default();
        if bold {
            m.insert(Modifiers::BOLD);
        }
        if italic {
            m.insert(Modifiers::ITALIC);
        }
        if reversed {
            m.insert(Modifiers::REVERSED);
        }
        m
    }

    fn parse(name: &str) -> Option<Modifiers> {
        Some(match name {
            "bold" => Self::BOLD,
            "dim" => Self::DIM,
            "italic" => Self::ITALIC,
            "slow_blink" => Self::SLOW_BLINK,
            "rapid_blink" => Self::RAPID_BLINK,
            "reversed" => Self::REVERSED,
            "hidden" => Self::HIDDEN,
            "crossed_out" => Self::CROSSED_OUT,
            _ => return None,
        })
    }
}

/// Helix's `underline.style` values; `"underlined"` in `modifiers` is
/// shorthand for `Line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnderlineStyle {
    Line,
    Curl,
    Dotted,
    Dashed,
    DoubleLine,
}

impl UnderlineStyle {
    fn parse(name: &str) -> Option<UnderlineStyle> {
        Some(match name {
            "line" => Self::Line,
            "curl" => Self::Curl,
            "dotted" => Self::Dotted,
            "dashed" => Self::Dashed,
            "double_line" => Self::DoubleLine,
            _ => return None,
        })
    }

    fn attribute(self) -> Attribute {
        match self {
            Self::Line => Attribute::Underlined,
            Self::Curl => Attribute::Undercurled,
            Self::Dotted => Attribute::Underdotted,
            Self::Dashed => Attribute::Underdashed,
            Self::DoubleLine => Attribute::DoubleUnderlined,
        }
    }
}

/// One resolved theme entry. `Color::Reset` is Helix's `default` (the
/// terminal's own foreground/background); `None` means "not specified",
/// which the renderer treats the same way, since it always resets after
/// every styled segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub underline_color: Option<Color>,
    pub underline: Option<UnderlineStyle>,
    pub modifiers: Modifiers,
}

impl Style {
    /// The crossterm attributes to set before printing text in this style
    /// (colors are handled separately by the caller, since they're
    /// `SetForegroundColor`/`SetBackgroundColor` commands, not attributes).
    pub fn attributes(&self) -> impl Iterator<Item = Attribute> + '_ {
        const FLAGS: [(Modifiers, Attribute); 8] = [
            (Modifiers::BOLD, Attribute::Bold),
            (Modifiers::DIM, Attribute::Dim),
            (Modifiers::ITALIC, Attribute::Italic),
            (Modifiers::SLOW_BLINK, Attribute::SlowBlink),
            (Modifiers::RAPID_BLINK, Attribute::RapidBlink),
            (Modifiers::REVERSED, Attribute::Reverse),
            (Modifiers::HIDDEN, Attribute::Hidden),
            (Modifiers::CROSSED_OUT, Attribute::CrossedOut),
        ];
        FLAGS
            .into_iter()
            .filter(move |(m, _)| self.modifiers.contains(*m))
            .map(|(_, a)| a)
            .chain(self.underline.map(UnderlineStyle::attribute))
    }
}

/// Print one run of text in a theme style (or plain, for `None`), resetting
/// all attributes afterwards so nothing leaks into the next segment. Colors
/// and underline color are commands; everything else is an attribute.
/// Shared by tico's own buffer rendering and `tcat`'s standalone highlighter.
pub fn print_styled(out: &mut impl Write, segment: &str, style: Option<Style>) -> io::Result<()> {
    let Some(style) = style else {
        return queue!(out, Print(segment));
    };
    if let Some(fg) = style.fg {
        queue!(out, SetForegroundColor(fg))?;
    }
    if let Some(bg) = style.bg {
        queue!(out, SetBackgroundColor(bg))?;
    }
    if let Some(uc) = style.underline_color {
        queue!(out, SetUnderlineColor(uc))?;
    }
    for attr in style.attributes() {
        queue!(out, SetAttribute(attr))?;
    }
    queue!(out, Print(segment), SetAttribute(Attribute::Reset))
}

/// A loaded theme: its scope table plus a memo of which style each
/// interned capture `Scope` resolves to. The memo has interior mutability
/// because resolution happens lazily from the (read-only) render pass —
/// scopes get interned as languages are first used, so the full set isn't
/// known when the theme is loaded.
#[derive(Debug, Clone)]
pub struct Theme {
    styles: HashMap<String, Style>,
    resolved: RefCell<HashMap<Scope, Option<Style>>>,
}

impl Theme {
    /// The style for a capture scope, falling back through its dotted
    /// prefixes (`keyword.control.import` -> `keyword.control` ->
    /// `keyword`), exactly as Helix's `Theme::try_get`. `None` means the
    /// theme says nothing about this scope at all, in which case the
    /// renderer leaves the enclosing highlight (if any) showing through,
    /// rather than resetting to plain text.
    pub fn style(&self, scope: Scope) -> Option<Style> {
        if let Some(s) = self.resolved.borrow().get(&scope) {
            return *s;
        }
        let style = self.style_for_name(scope.name());
        self.resolved.borrow_mut().insert(scope, style);
        style
    }

    /// `style()` for a scope name that hasn't (necessarily) been interned.
    pub fn style_for_name(&self, name: &str) -> Option<Style> {
        std::iter::successors(Some(name), |s| Some(s.rsplit_once('.')?.0))
            .find_map(|s| self.styles.get(s).copied())
    }

    /// The built-in default theme. Its TOML is part of the binary, so this
    /// can't fail short of a bug in that file, which the tests catch.
    pub fn builtin_default() -> Theme {
        let mut warnings = Vec::new();
        let theme = Loader::new(Vec::new())
            .load(DEFAULT_THEME_NAME, &mut warnings)
            .expect("built-in default theme must parse");
        debug_assert!(warnings.is_empty(), "{warnings:?}");
        theme
    }

    /// Parse one theme file's TOML (already merged with anything it
    /// inherits from) into a `Theme`. Unparseable individual entries are
    /// reported through `warnings` and skipped rather than failing the whole
    /// theme, matching Helix.
    fn from_toml(name: &str, mut table: toml::Table, warnings: &mut Vec<String>) -> Theme {
        let palette = match table.remove("palette") {
            Some(toml::Value::Table(t)) => Palette::from_table(t, name, warnings),
            Some(_) => {
                warnings.push(format!("theme {name}: [palette] must be a table"));
                Palette::default()
            }
            None => Palette::default(),
        };
        table.remove("inherits");
        // Helix's per-bracket-depth `rainbow` array: not a scope table
        // entry, and tico has no rainbow brackets. Drop it silently.
        table.remove("rainbow");

        let mut styles = HashMap::with_capacity(table.len());
        for (key, value) in table {
            match palette.parse_style(&value) {
                Ok(style) => {
                    styles.insert(key, style);
                }
                Err(e) => warnings.push(format!("theme {name}: {key:?}: {e}")),
            }
        }
        Theme {
            styles,
            resolved: RefCell::new(HashMap::new()),
        }
    }
}

/// Where theme files are looked for, in priority order, and the machinery
/// for loading one by name (following `inherits` chains, with cycle
/// detection).
pub struct Loader {
    dirs: Vec<PathBuf>,
}

impl Loader {
    pub fn new(dirs: Vec<PathBuf>) -> Loader {
        Loader { dirs }
    }

    /// The default search path: tico's own user theme directory first
    /// (`$XDG_CONFIG_HOME/tico/themes`, i.e. normally `~/.config/tico/themes`),
    /// then anywhere an installed Helix keeps its themes, so a Helix user's
    /// existing themes — and the ~70 that ship with Helix — are available
    /// by name without copying.
    pub fn with_default_dirs() -> Loader {
        let mut dirs = Vec::new();
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
        if let Some(ch) = &config_home {
            dirs.push(ch.join("tico/themes"));
            dirs.push(ch.join("helix/themes"));
        }
        if let Some(rt) = std::env::var_os("HELIX_RUNTIME") {
            dirs.push(PathBuf::from(rt).join("themes"));
        }
        for sys in [
            "/usr/share/helix/runtime/themes",
            "/usr/local/share/helix/runtime/themes",
            "/usr/lib/helix/runtime/themes",
            "/opt/homebrew/share/helix/runtime/themes",
        ] {
            dirs.push(PathBuf::from(sys));
        }
        Loader { dirs }
    }

    /// Load the theme called `name` (a bare name resolved against the
    /// search path, or a path to a `.toml` file). Returns `None`, with the
    /// reason in `warnings`, if it can't be found or read at all; a theme
    /// with merely some bad entries still loads, with those entries
    /// reported.
    pub fn load(&self, name: &str, warnings: &mut Vec<String>) -> Option<Theme> {
        let mut visited = HashSet::new();
        match self.load_toml(name, &mut visited) {
            Ok(table) => Some(Theme::from_toml(name, table, warnings)),
            Err(e) => {
                warnings.push(format!("theme {name}: {e}"));
                None
            }
        }
    }

    /// The theme's fully-merged TOML: its own file with its `inherits`
    /// ancestors folded in underneath. Top-level scope entries replace the
    /// parent's whole entry for that scope (no per-field merging), while
    /// `[palette]` tables are unioned with the child winning — both as in
    /// Helix's `merge_themes`.
    fn load_toml(&self, name: &str, visited: &mut HashSet<PathBuf>) -> Result<toml::Table, String> {
        let text = self.read(name, visited)?;
        let mut table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| format!("parse error: {}", e.message()))?;

        let Some(inherits) = table.get("inherits") else {
            return Ok(table);
        };
        let parent_name = inherits
            .as_str()
            .ok_or_else(|| format!("`inherits` must be a string, not {inherits}"))?
            .to_string();
        let mut merged = self.load_toml(&parent_name, visited)?;

        let child_palette = table.remove("palette");
        for (k, v) in table {
            merged.insert(k, v);
        }
        if let Some(toml::Value::Table(child)) = child_palette {
            match merged.get_mut("palette") {
                Some(toml::Value::Table(parent)) => {
                    for (k, v) in child {
                        parent.insert(k, v);
                    }
                }
                _ => {
                    merged.insert("palette".into(), toml::Value::Table(child));
                }
            }
        }
        Ok(merged)
    }

    fn read(&self, name: &str, visited: &mut HashSet<PathBuf>) -> Result<String, String> {
        // Built-ins are namespaced by prefix, so they can never be shadowed
        // by (or confused with) a same-named file in a Helix directory.
        if name.starts_with(BUILTIN_PREFIX) {
            let Some((_, text)) = BUILTIN_THEMES.iter().find(|(n, _)| *n == name) else {
                return Err(format!(
                    "no built-in theme named {name:?} (built-ins: {})",
                    BUILTIN_THEMES
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            };
            let marker = PathBuf::from(format!("<builtin>/{name}"));
            if !visited.insert(marker) {
                return Err(format!("cycle in `inherits` chain at built-in {name}"));
            }
            return Ok((*text).to_string());
        }

        // An explicit path (`theme = ~/x.toml`, `./x.toml`, `/x.toml`) is
        // used as-is; that's how a one-off theme file outside the search
        // path gets used. Anything else is a bare name.
        let looks_like_path =
            name.contains('/') || name.ends_with(".toml") || name.starts_with('~');
        if looks_like_path {
            let path = expand_tilde(name);
            if !visited.insert(path.clone()) {
                return Err(format!("cycle in `inherits` chain at {}", path.display()));
            }
            return std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()));
        }

        let filename = format!("{name}.toml");
        let mut cycle = false;
        for dir in &self.dirs {
            let path = dir.join(&filename);
            if !path.is_file() {
                continue;
            }
            if visited.contains(&path) {
                // Same escape hatch as Helix: a theme that inherits from a
                // same-named theme in a lower-priority directory (a user
                // "gruvbox" tweaking the system one) is allowed; only a
                // true cycle is an error.
                cycle = true;
                continue;
            }
            visited.insert(path.clone());
            return std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()));
        }
        Err(if cycle {
            format!("cycle in `inherits` chain at {name}")
        } else {
            format!(
                "no theme named {name:?} found (searched {})",
                self.searched()
            )
        })
    }

    fn searched(&self) -> String {
        let mut parts: Vec<String> = self.dirs.iter().map(|d| d.display().to_string()).collect();
        parts.push("built-in themes".into());
        parts.join(", ")
    }
}

/// Resolve the global theme and any per-language overrides from already-
/// loaded config + CLI options, exactly as `tico`'s own startup does: a
/// theme that fails to load falls back to the built-in default (for the
/// global one) or is just dropped (for a per-language override), with a
/// warning either way -- never a refusal to start. Shared by the `tico`
/// and `tcat` binaries, so both pick a theme the same way.
pub fn resolve_themes(
    options: &crate::options::Options,
    warnings: &mut Vec<String>,
) -> (Theme, HashMap<String, Theme>) {
    let loader = Loader::with_default_dirs();
    let theme = match options.theme.as_deref() {
        Some(name) => loader
            .load(name, warnings)
            .unwrap_or_else(Theme::builtin_default),
        None => Theme::builtin_default(),
    };
    let mut language_themes = HashMap::new();
    let mut loaded_themes: HashMap<&str, Option<Theme>> = HashMap::new();
    for (lang, name) in &options.language_themes {
        // Two languages sharing one theme parse it once; a theme that
        // failed to load is only reported once, too.
        let t = loaded_themes
            .entry(name.as_str())
            .or_insert_with(|| loader.load(name, warnings));
        match t {
            Some(t) => {
                language_themes.insert(lang.clone(), t.clone());
            }
            None => {
                language_themes.remove(lang);
            }
        }
    }
    (theme, language_themes)
}

/// Split a `--tico-theme` (or ticorc `[syntax]`) value of the form
/// `LANG.NAME` into its parts, only when `LANG` is a known language name --
/// so a theme path like `~/x.toml` or `./perl.toml`, whose first dot isn't
/// a language, still reads as one whole name. (A bare `perl.toml` *is*
/// taken as language `perl` + theme `toml`; write `./perl.toml` for the
/// file.) Shared by tico's own CLI parsing and tcat's.
pub fn split_language_theme(value: &str) -> Option<(&str, &str)> {
    let (lang, name) = value.split_once('.')?;
    if name.is_empty() || crate::syntax::find_by_name(&lang.to_ascii_lowercase()).is_none() {
        return None;
    }
    Some((lang, name))
}

/// Decide whether `bytes` should be syntax-highlighted at all (valid UTF-8,
/// under the configured size limit, `syntax_highlighting` on, and a
/// language actually detected or forced from `path` -- `None` for input
/// with no real filename, e.g. standard input), and either print it
/// highlighted or fall back to a raw, byte-for-byte passthrough. Shared by
/// `tcat` and `ttee`, whose only real difference is where `bytes` came
/// from and whether stdout is a terminal at all (the caller is expected to
/// only reach this once it already knows that -- there's nothing tty-
/// specific here).
pub fn highlight_or_plain(
    bytes: &[u8],
    path: Option<&std::path::Path>,
    options: &crate::options::Options,
    syntax_override: Option<&str>,
    theme: &Theme,
    language_themes: &HashMap<String, Theme>,
    out: &mut impl Write,
) -> io::Result<()> {
    let plain =
        !options.syntax_highlighting || bytes.len() as u64 > options.max_syntax_highlight_bytes;
    if !plain && let Ok(text) = std::str::from_utf8(bytes) {
        let lang = crate::syntax::detect_with_override(path, text, syntax_override);
        if let Some(lang) = lang {
            let spans = crate::syntax::highlight(text, lang);
            return print_highlighted(out, text, &spans, theme, language_themes);
        }
    }
    out.write_all(bytes)
}

/// `--color[=WHEN]` for `tcat`/`ttee`, with GNU `ls`/`grep` semantics:
/// `auto` (the default) colorizes only when stdout is a terminal, `always`
/// colorizes even into a pipe (`tcat --color=always foo.c | less -R`), and
/// `never` turns it off. A bare `--color` means `always`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorWhen {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorWhen {
    /// Whether output should be highlighted, given whether stdout is a
    /// terminal.
    pub fn colorize(self, stdout_is_tty: bool) -> bool {
        match self {
            ColorWhen::Auto => stdout_is_tty,
            ColorWhen::Always => true,
            ColorWhen::Never => false,
        }
    }
}

/// The theme to paint one span's scope with: its own language's override,
/// or the global theme -- exactly `Editor::theme_for`'s logic, without
/// needing an `Editor` to hang it off of. Shared by `tcat` and `ttee`.
pub fn theme_for<'a>(
    theme: &'a Theme,
    language_themes: &'a HashMap<String, Theme>,
    language: &str,
) -> &'a Theme {
    language_themes.get(language).unwrap_or(theme)
}

/// Colorize `text` (a whole operand's/stream's contents) line by line --
/// bounding memory to one line's worth of per-byte style resolution at a
/// time, rather than the whole text -- and write the original bytes
/// verbatim in between (line terminators are never touched, matching
/// `cat`/`tee`). Shared by `tcat` and `ttee`.
pub fn print_highlighted(
    out: &mut impl Write,
    text: &str,
    spans: &[HighlightSpan],
    theme: &Theme,
    language_themes: &HashMap<String, Theme>,
) -> io::Result<()> {
    let bytes = text.as_bytes();
    let mut line_start = 0usize;
    while line_start < bytes.len() {
        let nl = bytes[line_start..].iter().position(|&b| b == b'\n');
        let line_end = nl.map_or(bytes.len(), |off| line_start + off);
        print_line_highlighted(
            out,
            &text[line_start..line_end],
            line_start,
            spans,
            theme,
            language_themes,
        )?;
        match nl {
            Some(_) => {
                out.write_all(b"\n")?;
                line_start = line_end + 1;
            }
            None => line_start = bytes.len(),
        }
    }
    Ok(())
}

/// Resolve and print one line's worth of `spans` -- byte-indexed (not
/// char-indexed, unlike `ui.rs`'s line-grid renderer), since a standalone
/// highlighter just streams contiguous byte ranges rather than laying out
/// a fixed-width screen row. Later spans overwrite earlier ones where they
/// overlap (innermost/most-specific wins), and each span's *own* language
/// picks its theme -- not the operand's detected language -- so an
/// injected heredoc body (Perl `<<SQL`, VCL `inline C`, ...) follows its
/// own language's `[syntax]` override, exactly like the `tico` editor.
fn print_line_highlighted(
    out: &mut impl Write,
    raw: &str,
    line_start: usize,
    spans: &[HighlightSpan],
    theme: &Theme,
    language_themes: &HashMap<String, Theme>,
) -> io::Result<()> {
    let line_end = line_start + raw.len();
    let mut byte_styles: Vec<Option<Style>> = vec![None; raw.len()];
    for span in spans {
        if span.end <= line_start || span.start >= line_end {
            continue;
        }
        let Some(style) = theme_for(theme, language_themes, span.language).style(span.scope) else {
            continue;
        };
        let rel_start = span.start.max(line_start) - line_start;
        let rel_end = span.end.min(line_end) - line_start;
        for s in &mut byte_styles[rel_start..rel_end] {
            *s = Some(style);
        }
    }

    let len = raw.len();
    let mut i = 0;
    while i < len {
        let style = byte_styles[i];
        let mut j = i + 1;
        while j < len && byte_styles[j] == style {
            j += 1;
        }
        // Span boundaries are always at UTF-8 char boundaries (tree-sitter
        // guarantees this), so every point where the style changes is one
        // too -- this slice is safe.
        print_styled(out, &raw[i..j], style)?;
        i = j;
    }
    Ok(())
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(p)
}

/// The color names a theme can use: the sixteen terminal colors plus
/// whatever its `[palette]` adds (palette names may shadow the terminal
/// ones, as in Helix).
#[derive(Debug, Clone, Default)]
struct Palette {
    named: HashMap<String, Color>,
}

impl Palette {
    fn from_table(table: toml::Table, theme: &str, warnings: &mut Vec<String>) -> Palette {
        let mut named = HashMap::with_capacity(table.len());
        for (key, value) in table {
            match value
                .as_str()
                .ok_or_else(|| "not a string".to_string())
                .and_then(parse_color_literal)
            {
                Ok(c) => {
                    named.insert(key, c);
                }
                Err(e) => warnings.push(format!("theme {theme}: palette entry {key:?}: {e}")),
            }
        }
        Palette { named }
    }

    fn parse_color(&self, value: &toml::Value) -> Result<Color, String> {
        let s = value
            .as_str()
            .ok_or_else(|| format!("expected a color string, got {value}"))?;
        if let Some(c) = self.named.get(s) {
            return Ok(*c);
        }
        parse_color_literal(s)
    }

    /// A theme entry's value: either a bare color string (sets `fg`) or a
    /// `{ fg, bg, underline, modifiers }` table.
    fn parse_style(&self, value: &toml::Value) -> Result<Style, String> {
        let mut style = Style::default();
        let toml::Value::Table(entries) = value else {
            style.fg = Some(self.parse_color(value)?);
            return Ok(style);
        };
        for (key, v) in entries {
            match key.as_str() {
                "fg" => style.fg = Some(self.parse_color(v)?),
                "bg" => style.bg = Some(self.parse_color(v)?),
                "underline" => {
                    let t = v
                        .as_table()
                        .ok_or_else(|| "`underline` must be a table".to_string())?;
                    for (uk, uv) in t {
                        match uk.as_str() {
                            "color" => style.underline_color = Some(self.parse_color(uv)?),
                            "style" => {
                                let s = uv
                                    .as_str()
                                    .ok_or_else(|| format!("invalid underline style {uv}"))?;
                                style.underline = Some(
                                    UnderlineStyle::parse(s)
                                        .ok_or_else(|| format!("invalid underline style {s:?}"))?,
                                );
                            }
                            other => return Err(format!("invalid underline attribute {other:?}")),
                        }
                    }
                }
                "modifiers" => {
                    let list = v
                        .as_array()
                        .ok_or_else(|| "`modifiers` must be an array".to_string())?;
                    for m in list {
                        let s = m.as_str().ok_or_else(|| format!("invalid modifier {m}"))?;
                        if s == "underlined" {
                            style.underline = Some(UnderlineStyle::Line);
                        } else {
                            style.modifiers.insert(
                                Modifiers::parse(s)
                                    .ok_or_else(|| format!("invalid modifier {s:?}"))?,
                            );
                        }
                    }
                }
                other => return Err(format!("invalid style attribute {other:?}")),
            }
        }
        Ok(style)
    }
}

/// A color that isn't a palette name: one of the terminal color names,
/// `#rrggbb`/`#rgb`, or a 0-255 palette index. Names map to the same
/// escape codes Helix emits for them (crossterm's `Dark*` variants are the
/// normal-intensity colors; its unprefixed ones are the bright/"light"
/// set).
fn parse_color_literal(s: &str) -> Result<Color, String> {
    Ok(match s {
        "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::DarkRed,
        "green" => Color::DarkGreen,
        "yellow" => Color::DarkYellow,
        "blue" => Color::DarkBlue,
        "magenta" => Color::DarkMagenta,
        "cyan" => Color::DarkCyan,
        "gray" => Color::DarkGrey,
        "light-red" => Color::Red,
        "light-green" => Color::Green,
        "light-yellow" => Color::Yellow,
        "light-blue" => Color::Blue,
        "light-magenta" => Color::Magenta,
        "light-cyan" => Color::Cyan,
        "light-gray" => Color::Grey,
        "white" => Color::White,
        hex if hex.starts_with('#') => {
            parse_hex(hex).ok_or_else(|| format!("malformed hex color {s:?}"))?
        }
        idx => match idx.parse::<u8>() {
            Ok(n) => Color::AnsiValue(n),
            Err(_) => return Err(format!("unknown color {s:?}")),
        },
    })
}

fn parse_hex(h: &str) -> Option<Color> {
    let digits = h.strip_prefix('#')?;
    let byte = |s: &str| u8::from_str_radix(s, 16).ok();
    match digits.len() {
        6 => Some(Color::Rgb {
            r: byte(&digits[0..2])?,
            g: byte(&digits[2..4])?,
            b: byte(&digits[4..6])?,
        }),
        3 => {
            let nibble = |s: &str| byte(s).map(|n| n * 17);
            Some(Color::Rgb {
                r: nibble(&digits[0..1])?,
                g: nibble(&digits[1..2])?,
                b: nibble(&digits[2..3])?,
            })
        }
        _ => None,
    }
}

/// The names of the themes compiled into the binary, in table order (the
/// default first).
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_THEMES.iter().map(|(n, _)| *n)
}

impl Loader {
    /// Every theme file on the search path, as `(name, path)`, sorted by
    /// name. A name found in more than one directory is listed once, at the
    /// path `load()` would actually use (the first directory wins).
    pub fn disk_themes(&self) -> Vec<(String, PathBuf)> {
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        for dir in &self.dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    && !found.iter().any(|(n, _)| n == stem)
                {
                    found.push((stem.to_string(), path));
                }
            }
        }
        found.sort();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize, scope_name: &str, language: &'static str) -> HighlightSpan {
        HighlightSpan {
            start,
            end,
            scope: Scope::intern(scope_name),
            language,
        }
    }

    #[test]
    fn theme_for_falls_back_to_the_global_theme() {
        let theme = Theme::builtin_default();
        let language_themes = HashMap::new();
        // Same underlying data either way (no override configured), but
        // this confirms the lookup itself doesn't panic/misbehave when
        // the map is empty.
        assert!(std::ptr::eq(
            theme_for(&theme, &language_themes, "perl"),
            &theme
        ));
    }

    #[test]
    fn theme_for_prefers_a_configured_per_language_override() {
        let theme = Theme::builtin_default();
        let mut language_themes = HashMap::new();
        language_themes.insert("perl".to_string(), Theme::builtin_default());
        assert!(std::ptr::eq(
            theme_for(&theme, &language_themes, "perl"),
            language_themes.get("perl").unwrap()
        ));
        // An unconfigured language still falls back to the global theme.
        assert!(std::ptr::eq(
            theme_for(&theme, &language_themes, "c"),
            &theme
        ));
    }

    #[test]
    fn print_highlighted_preserves_line_endings_and_untouched_gaps() {
        let theme = Theme::builtin_default();
        let language_themes = HashMap::new();
        // "keyword" is styled by the built-in theme; the space and
        // semicolon in between aren't covered by any span, so they must
        // come through with no color codes at all.
        let text = "fn x\nfn y\n";
        let spans = vec![span(0, 2, "keyword", "rust"), span(5, 7, "keyword", "rust")];
        let mut out = Vec::new();
        print_highlighted(&mut out, text, &spans, &theme, &language_themes).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Two identical lines, each with only "fn" colored -- confirms both
        // per-line slicing and that line terminators pass through verbatim.
        assert_eq!(s.matches('\n').count(), 2);
        assert!(s.contains("fn"), "{s:?}");
        assert!(
            s.contains(" x\n"),
            "the untouched tail must be plain: {s:?}"
        );
        assert!(
            s.contains(" y\n"),
            "the untouched tail must be plain: {s:?}"
        );
    }

    #[test]
    fn print_highlighted_lets_a_later_overlapping_span_win() {
        let theme = Theme::builtin_default();
        let language_themes = HashMap::new();
        // "string" and "keyword" resolve to different styles in the
        // built-in theme; the later, narrower span should win for the
        // bytes it covers (innermost/most-specific-wins, matching the
        // tico editor's own layering).
        let text = "abc";
        let spans = vec![span(0, 3, "string", "rust"), span(1, 2, "keyword", "rust")];
        let mut out = Vec::new();
        print_highlighted(&mut out, text, &spans, &theme, &language_themes).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Three distinct styled/plain segments: "a" (string), "b"
        // (keyword, overwrites string), "c" (string again).
        let reset = "\x1b[0m";
        assert_eq!(s.matches(reset).count(), 3, "{s:?}");
    }

    #[test]
    fn print_highlighted_skips_a_scope_the_theme_says_nothing_about() {
        let theme = Theme::builtin_default();
        let language_themes = HashMap::new();
        let text = "abc";
        // Not a real Helix scope name the built-in theme resolves, so it
        // must fall through to plain, uncolored output.
        let spans = vec![span(0, 3, "totally.made.up.scope", "rust")];
        let mut out = Vec::new();
        print_highlighted(&mut out, text, &spans, &theme, &language_themes).unwrap();
        assert_eq!(out, b"abc");
    }

    fn parse(name: &str, text: &str) -> (Theme, Vec<String>) {
        let mut warnings = Vec::new();
        let table: toml::Table = text.parse().expect("test TOML must parse");
        let theme = Theme::from_toml(name, table, &mut warnings);
        (theme, warnings)
    }

    #[test]
    fn bare_string_sets_only_fg() {
        let (t, w) = parse("t", r##""keyword" = "blue""##);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            t.style_for_name("keyword"),
            Some(Style {
                fg: Some(Color::DarkBlue),
                ..Style::default()
            })
        );
    }

    #[test]
    fn table_form_parses_every_field() {
        let (t, w) = parse(
            "t",
            r##""string" = { fg = "#ff8800", bg = "black", modifiers = ["bold", "italic", "underlined"], underline = { color = "light-red", style = "curl" } }"##,
        );
        assert!(w.is_empty(), "{w:?}");
        let s = t.style_for_name("string").unwrap();
        assert_eq!(
            s.fg,
            Some(Color::Rgb {
                r: 255,
                g: 136,
                b: 0
            })
        );
        assert_eq!(s.bg, Some(Color::Black));
        assert!(s.modifiers.contains(Modifiers::BOLD));
        assert!(s.modifiers.contains(Modifiers::ITALIC));
        assert!(!s.modifiers.contains(Modifiers::DIM));
        // `underline.style` came after `modifiers = ["underlined"]` and
        // wins, as the last setting of the same field.
        assert_eq!(s.underline, Some(UnderlineStyle::Curl));
        assert_eq!(s.underline_color, Some(Color::Red));
        let attrs: Vec<Attribute> = s.attributes().collect();
        assert_eq!(
            attrs,
            vec![Attribute::Bold, Attribute::Italic, Attribute::Undercurled]
        );
    }

    #[test]
    fn dotted_scopes_fall_back_to_their_longest_defined_prefix() {
        let (t, _) = parse(
            "t",
            r##"
"keyword" = "blue"
"keyword.control" = "red"
"##,
        );
        assert_eq!(
            t.style_for_name("keyword.control.import").unwrap().fg,
            Some(Color::DarkRed)
        );
        assert_eq!(
            t.style_for_name("keyword.operator").unwrap().fg,
            Some(Color::DarkBlue)
        );
        assert_eq!(
            t.style_for_name("keyword").unwrap().fg,
            Some(Color::DarkBlue)
        );
        assert_eq!(t.style_for_name("function"), None);
    }

    #[test]
    fn palette_names_resolve_and_may_shadow_terminal_colors() {
        let (t, w) = parse(
            "t",
            r##"
"comment" = "mygray"
"string" = "red"

[palette]
mygray = "#808080"
red = "#f00"
"##,
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            t.style_for_name("comment").unwrap().fg,
            Some(Color::Rgb {
                r: 128,
                g: 128,
                b: 128
            })
        );
        assert_eq!(
            t.style_for_name("string").unwrap().fg,
            Some(Color::Rgb { r: 255, g: 0, b: 0 })
        );
    }

    #[test]
    fn terminal_color_names_match_helix_escape_codes() {
        // The three grey-ish names are the ones that are easy to get wrong;
        // these are what Helix's crossterm backend emits for them.
        assert_eq!(parse_color_literal("gray"), Ok(Color::DarkGrey));
        assert_eq!(parse_color_literal("light-gray"), Ok(Color::Grey));
        assert_eq!(parse_color_literal("white"), Ok(Color::White));
        assert_eq!(parse_color_literal("default"), Ok(Color::Reset));
        assert_eq!(parse_color_literal("yellow"), Ok(Color::DarkYellow));
        assert_eq!(parse_color_literal("light-yellow"), Ok(Color::Yellow));
        assert_eq!(parse_color_literal("208"), Ok(Color::AnsiValue(208)));
        assert!(parse_color_literal("chartreuse").is_err());
        assert!(parse_color_literal("#12345").is_err());
    }

    #[test]
    fn bad_entries_warn_but_do_not_sink_the_theme() {
        let (t, w) = parse(
            "t",
            r##"
"keyword" = "blue"
"string" = "nosuchcolor"
"type" = { fg = "red", bogus = 1 }
"##,
        );
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("string") || w[1].contains("string"));
        assert!(t.style_for_name("keyword").is_some());
        assert!(t.style_for_name("string").is_none());
        assert!(t.style_for_name("type").is_none());
    }

    #[test]
    fn ui_and_rainbow_entries_are_accepted_and_ignored() {
        let (t, w) = parse(
            "t",
            r##"
"ui.background" = { bg = "black" }
"ui.selection" = { modifiers = ["reversed"] }
rainbow = ["red", "green", { fg = "blue" }]
"comment" = "gray"
"##,
        );
        assert!(w.is_empty(), "{w:?}");
        assert!(t.style_for_name("comment").is_some());
        assert!(t.style_for_name("rainbow").is_none());
    }

    #[test]
    fn builtin_default_loads_cleanly() {
        let t = Theme::builtin_default();
        assert!(t.style_for_name("keyword").is_some());
        assert!(t.style_for_name("comment").is_some());
        assert!(t.style_for_name("diff.plus").is_some());
    }

    #[test]
    fn inherits_overlays_child_scopes_and_unions_palettes() {
        let dir = tempdir("inherits");
        std::fs::write(
            dir.join("parent.toml"),
            r##"
"keyword" = "kw"
"string" = { fg = "str", modifiers = ["bold"] }
"comment" = "gray"

[palette]
kw = "#000001"
str = "#000002"
"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("child.toml"),
            r##"
inherits = "parent"
"string" = "str"
"type" = "ty"

[palette]
kw = "#000003"
ty = "#000004"
"##,
        )
        .unwrap();
        let loader = Loader::new(vec![dir.clone()]);
        let mut w = Vec::new();
        let t = loader.load("child", &mut w).expect("child theme loads");
        assert!(w.is_empty(), "{w:?}");
        // Inherited entry, re-resolved against the child's palette override.
        assert_eq!(
            t.style_for_name("keyword").unwrap().fg,
            Some(Color::Rgb { r: 0, g: 0, b: 3 })
        );
        // Child entry replaces the parent's whole entry: no bold carried over.
        let s = t.style_for_name("string").unwrap();
        assert_eq!(s.fg, Some(Color::Rgb { r: 0, g: 0, b: 2 }));
        assert!(!s.modifiers.contains(Modifiers::BOLD));
        // Parent-only entry survives; child-only palette name resolves.
        assert_eq!(
            t.style_for_name("comment").unwrap().fg,
            Some(Color::DarkGrey)
        );
        assert_eq!(
            t.style_for_name("type").unwrap().fg,
            Some(Color::Rgb { r: 0, g: 0, b: 4 })
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inherits_can_name_a_builtin_theme() {
        let dir = tempdir("inherits_builtin");
        std::fs::write(
            dir.join("mine.toml"),
            "inherits = \"tico-builtin-default\"\n\"comment\" = \"light-red\"\n",
        )
        .unwrap();
        let loader = Loader::new(vec![dir.clone()]);
        let mut w = Vec::new();
        let t = loader.load("mine", &mut w).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(t.style_for_name("comment").unwrap().fg, Some(Color::Red));
        assert!(
            t.style_for_name("keyword").is_some(),
            "inherited from default"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inherits_cycle_is_an_error_not_a_hang() {
        let dir = tempdir("cycle");
        std::fs::write(dir.join("a.toml"), "inherits = \"b\"\n").unwrap();
        std::fs::write(dir.join("b.toml"), "inherits = \"a\"\n").unwrap();
        let loader = Loader::new(vec![dir.clone()]);
        let mut w = Vec::new();
        assert!(loader.load("a", &mut w).is_none());
        assert!(w.iter().any(|m| m.contains("cycle")), "{w:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_theme_reports_where_it_looked() {
        let loader = Loader::new(vec![PathBuf::from("/nonexistent/themes")]);
        let mut w = Vec::new();
        assert!(loader.load("nope", &mut w).is_none());
        assert_eq!(w.len(), 1);
        assert!(
            w[0].contains("nope") && w[0].contains("/nonexistent/themes"),
            "{w:?}"
        );
    }

    #[test]
    fn explicit_path_loads_directly() {
        let dir = tempdir("path");
        let file = dir.join("custom.toml");
        std::fs::write(&file, "\"keyword\" = \"cyan\"\n").unwrap();
        let loader = Loader::new(Vec::new());
        let mut w = Vec::new();
        let t = loader.load(file.to_str().unwrap(), &mut w).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            t.style_for_name("keyword").unwrap().fg,
            Some(Color::DarkCyan)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn disk_themes_lists_toml_files_first_directory_winning() {
        let hi = tempdir("names-hi");
        let lo = tempdir("names-lo");
        std::fs::write(hi.join("zeta.toml"), "").unwrap();
        std::fs::write(hi.join("notes.txt"), "").unwrap();
        std::fs::write(lo.join("zeta.toml"), "").unwrap();
        std::fs::write(lo.join("alpha.toml"), "").unwrap();
        let found = Loader::new(vec![hi.clone(), lo.clone()]).disk_themes();
        assert_eq!(
            found,
            vec![
                ("alpha".to_string(), lo.join("alpha.toml")),
                ("zeta".to_string(), hi.join("zeta.toml")),
            ]
        );
        assert_eq!(builtin_names().next(), Some(DEFAULT_THEME_NAME));
        assert_eq!(builtin_names().count(), BUILTIN_THEMES.len());
        std::fs::remove_dir_all(&hi).ok();
        std::fs::remove_dir_all(&lo).ok();
    }

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tico-theme-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_builtin_theme_loads_without_warnings() {
        let loader = Loader::new(Vec::new());
        for (name, _) in BUILTIN_THEMES {
            assert!(name.starts_with(BUILTIN_PREFIX), "{name}");
            let mut w = Vec::new();
            let t = loader
                .load(name, &mut w)
                .unwrap_or_else(|| panic!("{name}: {w:?}"));
            assert!(w.is_empty(), "{name}: {w:?}");
            for scope in ["comment", "keyword", "string", "function", "type"] {
                assert!(t.style_for_name(scope).is_some(), "{name} lacks {scope}");
            }
        }
    }

    #[test]
    fn builtin_names_match_their_files_on_disk() {
        // `include_str!` keeps the *contents* in sync; this keeps the table's
        // keys honest against the file names, and catches a file added to
        // themes/ without a table entry.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(String::from))?
            })
            .collect();
        on_disk.sort();
        let mut in_table: Vec<String> = BUILTIN_THEMES.iter().map(|(n, _)| n.to_string()).collect();
        in_table.sort();
        assert_eq!(on_disk, in_table);
    }

    #[test]
    fn unknown_builtin_name_is_reported_without_touching_disk() {
        let loader = Loader::new(vec![PathBuf::from("/nonexistent")]);
        let mut w = Vec::new();
        assert!(loader.load("tico-builtin-nope", &mut w).is_none());
        assert!(w[0].contains("no built-in theme"), "{w:?}");
    }

    #[test]
    fn color_when_gates_on_the_terminal_only_for_auto() {
        assert!(ColorWhen::Auto.colorize(true));
        assert!(!ColorWhen::Auto.colorize(false));
        assert!(ColorWhen::Always.colorize(false));
        assert!(!ColorWhen::Never.colorize(true));
        assert_eq!(ColorWhen::default(), ColorWhen::Auto);
    }
}
