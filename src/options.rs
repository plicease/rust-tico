//! The full set of nano-compatible settings ("set" options in nanorc terms),
//! with their defaults, as documented in `nanorc(5)`.

/// A named color, with the "light" (bright) intensity nano's `light`-prefix
/// convention selects (e.g. `lightyellow`) tracked explicitly — `light` has
/// no effect for `Normal` or `Rgb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Normal,
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedColor {
    pub color: Color,
    pub light: bool,
}

impl NamedColor {
    const fn new(color: Color) -> NamedColor {
        NamedColor {
            color,
            light: false,
        }
    }

    const fn light(color: Color) -> NamedColor {
        NamedColor { color, light: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorPair {
    pub bold: bool,
    pub italic: bool,
    pub fg: Option<NamedColor>,
    pub bg: Option<NamedColor>,
}

/// All boolean "set"/"unset" toggles from nanorc(5), off by default unless
/// noted. Field names match the nanorc option names.
#[derive(Debug, Clone)]
pub struct Options {
    // Boolean toggles (all default false unless noted).
    pub afterends: bool,
    pub allow_insecure_backup: bool,
    pub atblanks: bool,
    pub autoindent: bool,
    pub backup: bool,
    pub boldtext: bool,
    pub bookstyle: bool,
    pub breaklonglines: bool,
    pub casesensitive: bool,
    pub colonparsing: bool,
    pub constantshow: bool,
    pub cutfromcursor: bool,
    pub emptyline: bool,
    pub historylog: bool,
    pub indicator: bool,
    pub jumpyscrolling: bool,
    pub linenumbers: bool,
    pub locking: bool,
    pub magic: bool,
    pub minibar: bool,
    pub mouse: bool,
    pub multibuffer: bool,
    pub noconvert: bool,
    pub nohelp: bool,
    pub nonewlines: bool,
    pub positionlog: bool,
    pub preserve: bool,
    pub quickblank: bool,
    pub rawsequences: bool,
    pub rebinddelete: bool,
    pub regexp: bool,
    pub saveonexit: bool,
    pub showcursor: bool,
    pub smarthome: bool,
    pub softwrap: bool,
    pub stateflags: bool,
    pub tabstospaces: bool,
    pub trimblanks: bool,
    pub unix: bool,
    pub wordbounds: bool,
    pub zap: bool,
    pub zero: bool,
    /// syntax highlighting on by default (per tico's own requirements, unlike
    /// nano which enables it whenever a syntax matches regardless of a toggle).
    pub syntax_highlighting: bool,

    // Valued options.
    pub backupdir: Option<String>,
    pub brackets: String,
    pub fill: i32,
    pub guidestripe: Option<u32>,
    pub matchbrackets: String,
    pub operatingdir: Option<String>,
    pub punct: String,
    pub quotestr: String,
    pub speller: Option<String>,
    pub tabsize: u32,
    pub whitespace: (char, char),
    pub wordchars: Option<String>,

    // Colors.
    pub errorcolor: ColorPair,
    pub functioncolor: ColorPair,
    pub keycolor: ColorPair,
    pub minicolor: ColorPair,
    pub numbercolor: ColorPair,
    pub promptcolor: ColorPair,
    pub scrollercolor: ColorPair,
    pub selectedcolor: ColorPair,
    pub spotlightcolor: ColorPair,
    pub statuscolor: ColorPair,
    pub stripecolor: ColorPair,
    pub titlecolor: ColorPair,

    // Other CLI-only / mixed options not exposed as plain "set" booleans
    // above (nano exposes these as both CLI flags and nanorc `set` names).
    pub restricted: bool,
    pub view: bool,
    pub nowrap: bool,
    pub ignorercfiles: bool,
    pub modernbindings: bool,
    pub syntax_name: Option<String>,
    pub rcfile: Option<String>,

    /// tico-only (no nano equivalent): buffers larger than this are never
    /// syntax-highlighted, regardless of `syntax_highlighting` -- a full
    /// tree-sitter parse plus query run isn't free, and without a cap a
    /// large enough file (a big generated source file, a log file opened
    /// by mistake, ...) would make every load/edit visibly stall.
    /// Configured via `~/.ticorc`'s `[tico]` section
    /// (`max_syntax_highlight_size = 4MB`); see `parse_byte_size`.
    pub max_syntax_highlight_bytes: u64,
    /// tico-only: the syntax-highlighting theme to load (a Helix-format
    /// theme; see `crate::theme`), by name or file path. `None` means the
    /// built-in default. Configured via `~/.ticorc`'s `[syntax]` section
    /// (`theme = tico-builtin-gruvbox`) or `--tico-theme NAME`.
    pub theme: Option<String>,
    /// tico-only: per-language theme overrides, `(language name, theme
    /// name)`, from `[syntax]` lines like `perl.theme = tico-builtin-nord`.
    /// A language not listed here uses `theme`. Kept in file order; a
    /// later entry for the same language wins.
    pub language_themes: Vec<(String, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            afterends: false,
            allow_insecure_backup: false,
            atblanks: false,
            autoindent: false,
            backup: false,
            boldtext: false,
            bookstyle: false,
            breaklonglines: false,
            casesensitive: false,
            colonparsing: false,
            constantshow: false,
            cutfromcursor: false,
            emptyline: false,
            historylog: false,
            indicator: false,
            jumpyscrolling: false,
            linenumbers: false,
            locking: false,
            magic: false,
            minibar: false,
            mouse: false,
            multibuffer: false,
            noconvert: false,
            nohelp: false,
            nonewlines: false,
            positionlog: false,
            preserve: false,
            quickblank: false,
            rawsequences: false,
            rebinddelete: false,
            regexp: false,
            saveonexit: false,
            showcursor: false,
            smarthome: false,
            softwrap: false,
            stateflags: false,
            tabstospaces: false,
            trimblanks: false,
            unix: false,
            wordbounds: false,
            zap: false,
            zero: false,
            syntax_highlighting: true,

            backupdir: None,
            brackets: "\"')>]}".to_string(),
            fill: -8,
            guidestripe: None,
            matchbrackets: "(<[{)>]}".to_string(),
            operatingdir: None,
            punct: "!.?".to_string(),
            quotestr: "^([ \t]*([!#%:;>|}]|//))+".to_string(),
            speller: None,
            tabsize: 8,
            whitespace: ('\u{bb}', '\u{22c5}'),
            wordchars: None,

            errorcolor: ColorPair {
                bold: true,
                italic: false,
                fg: Some(NamedColor::new(Color::White)),
                bg: Some(NamedColor::new(Color::Red)),
            },
            functioncolor: ColorPair::default(),
            keycolor: ColorPair::default(),
            minicolor: ColorPair::default(),
            numbercolor: ColorPair::default(),
            promptcolor: ColorPair::default(),
            scrollercolor: ColorPair::default(),
            selectedcolor: ColorPair::default(),
            spotlightcolor: ColorPair {
                bold: false,
                italic: false,
                fg: Some(NamedColor::new(Color::Black)),
                bg: Some(NamedColor::light(Color::Yellow)),
            },
            statuscolor: ColorPair::default(),
            stripecolor: ColorPair::default(),
            titlecolor: ColorPair::default(),

            restricted: false,
            view: false,
            nowrap: true,
            ignorercfiles: false,
            modernbindings: false,
            syntax_name: None,
            rcfile: None,
            // 4 MiB: generous enough that real-world source files
            // essentially never hit it, while keeping worst-case
            // highlight latency well under a second (see the perf work
            // that made this cap meaningful to set at all).
            max_syntax_highlight_bytes: 4 * 1024 * 1024,
            theme: None,
            language_themes: Vec::new(),
        }
    }
}

/// Parse a byte-size setting: a bare integer (bytes), or one suffixed with
/// `KB`/`MB`/`GB` (case-insensitive; 1024-based, so `1MB` == 1048576).
/// `None` on anything that isn't a plain integer optionally followed by
/// one of those three suffixes, or that overflows `u64` once multiplied.
pub fn parse_byte_size(spec: &str) -> Option<u64> {
    let lower = spec.trim().to_ascii_lowercase();
    let (digits, multiplier): (&str, u64) = if let Some(n) = lower.strip_suffix("gb") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("kb") {
        (n, 1024)
    } else {
        (lower.as_str(), 1)
    };
    digits.trim().parse::<u64>().ok()?.checked_mul(multiplier)
}

/// Parse a nanorc-style color spec: `[bold,][italic,]fgcolor[,bgcolor]`.
pub fn parse_color_pair(spec: &str) -> Option<ColorPair> {
    let mut cp = ColorPair::default();
    let mut parts: Vec<&str> = spec.split(',').map(|s| s.trim()).collect();
    parts.retain(|p| !p.is_empty());
    let mut colors = Vec::new();
    for p in parts {
        match p {
            "bold" => cp.bold = true,
            "italic" => cp.italic = true,
            other => colors.push(other),
        }
    }
    if let Some(fg) = colors.first() {
        cp.fg = parse_color(fg);
    }
    if let Some(bg) = colors.get(1) {
        cp.bg = parse_color(bg);
    }
    Some(cp)
}

fn parse_color(name: &str) -> Option<NamedColor> {
    // "grey"/"gray" is documented as a synonym for lightblack, so it
    // carries its own light-ness rather than going through the general
    // light-prefix stripping below.
    if name == "grey" || name == "gray" {
        return Some(NamedColor::light(Color::Black));
    }
    let (name, light) = if let Some(rest) = name.strip_prefix("light") {
        (rest, true)
    } else {
        (name, false)
    };
    if let Some(hex) = name.strip_prefix('#') {
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            return Some(NamedColor::new(Color::Rgb(r, g, b)));
        }
        return None;
    }
    let color = match name {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "normal" => Color::Normal,
        _ => return None,
    };
    Some(NamedColor { color, light })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_byte_size_plain_bytes() {
        assert_eq!(parse_byte_size("4096"), Some(4096));
        assert_eq!(parse_byte_size("0"), Some(0));
    }

    #[test]
    fn parse_byte_size_kb_mb_gb_suffixes_are_1024_based_and_case_insensitive() {
        assert_eq!(parse_byte_size("4KB"), Some(4 * 1024));
        assert_eq!(parse_byte_size("4kb"), Some(4 * 1024));
        assert_eq!(parse_byte_size("4Kb"), Some(4 * 1024));
        assert_eq!(parse_byte_size("4MB"), Some(4 * 1024 * 1024));
        assert_eq!(parse_byte_size("4mb"), Some(4 * 1024 * 1024));
        assert_eq!(parse_byte_size("2GB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_byte_size("2gb"), Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn parse_byte_size_tolerates_surrounding_and_inner_whitespace() {
        assert_eq!(parse_byte_size("  4 MB  "), Some(4 * 1024 * 1024));
    }

    #[test]
    fn parse_byte_size_rejects_garbage() {
        assert_eq!(parse_byte_size(""), None);
        assert_eq!(parse_byte_size("MB"), None);
        assert_eq!(parse_byte_size("4TB"), None); // not a supported suffix
        assert_eq!(parse_byte_size("four MB"), None);
        assert_eq!(parse_byte_size("-4MB"), None);
    }

    #[test]
    fn parse_byte_size_overflow_returns_none_instead_of_panicking() {
        assert_eq!(parse_byte_size("99999999999999999999GB"), None);
    }
}
