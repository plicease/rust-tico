//! `tcat`: behaves like POSIX `cat`, except that when stdout is a terminal
//! it colorizes each operand with tico's own syntax highlighting -- same
//! config file (`~/.ticorc`), same theme resolution, same language
//! detection (filename extension -> shebang -> modeline) as the `tico`
//! editor itself. Piped/redirected output is byte-identical to plain
//! `cat`, so `tcat` is safe to use anywhere `cat` is.
//!
//! Per operand, not per run: `tcat foo.pl foo.c` highlights each in its own
//! language. `-Y`/`--syntax NAME` overrides detection for every operand at
//! once (matching tico's own, not-per-file, `-Y` semantics); `-Y none`
//! disables highlighting outright. Standard input has no filename, so it's
//! only highlighted when `-Y` names a real language -- which means fully
//! buffering it first (giving up `cat`'s usual streaming for that case).
//!
//! GNU `cat` extensions (`-n`, `-s`, ...) are intentionally out of scope.

use clap::Parser;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use tico::syntax::HighlightSpan;
use tico::theme::{Style, Theme};

/// The theme to paint one span's scope with: its own language's override,
/// or the global theme -- exactly `Editor::theme_for`'s logic, without
/// needing an `Editor` to hang it off of.
fn theme_for<'a>(
    theme: &'a Theme,
    language_themes: &'a HashMap<String, Theme>,
    language: &str,
) -> &'a Theme {
    language_themes.get(language).unwrap_or(theme)
}

#[derive(Parser, Debug)]
#[command(
    name = "tcat",
    version,
    about = "cat, with tico's syntax highlighting when stdout is a terminal"
)]
struct Cli {
    #[arg(
        short = 'u',
        help = "Accepted for POSIX cat compatibility; output is never extra-buffered here"
    )]
    _unbuffered: bool,
    #[arg(
        short = 'Y',
        long = "syntax",
        value_name = "name",
        help = "Force this language for every operand (\"none\" disables highlighting)"
    )]
    syntax: Option<String>,
    #[arg(
        long = "tico-theme",
        value_name = "[lang.]name",
        action = clap::ArgAction::Append,
        help = "Syntax-highlighting theme, overriding [syntax]'s `theme` (or, as `lang.NAME`, \
                only that language's); a built-in (tico-builtin-*), a Helix theme name, or \
                a .toml path. May be repeated."
    )]
    tico_theme: Vec<String>,
    #[arg(
        short = 'f',
        long = "rcfile",
        value_name = "file",
        help = "Use only this file for configuring tico"
    )]
    rcfile: Option<String>,
    #[arg(
        short = 'I',
        long = "ignorercfiles",
        help = "Don't look at nanorc/ticorc files"
    )]
    ignorercfiles: bool,
    /// Files to print. `-`, or no operands at all, means standard input.
    #[arg(trailing_var_arg = true)]
    files: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    let loaded = tico::config::load(cli.rcfile.as_deref(), cli.ignorercfiles, false);
    let mut options = loaded.options;
    if let Some(v) = &cli.syntax {
        options.syntax_name = Some(v.clone());
    }
    for v in &cli.tico_theme {
        match tico::theme::split_language_theme(v) {
            Some((lang, name)) => options
                .language_themes
                .push((lang.to_ascii_lowercase(), name.to_string())),
            None => options.theme = Some(v.clone()),
        }
    }

    let mut warnings = loaded.warnings;
    let (theme, language_themes) = tico::theme::resolve_themes(&options, &mut warnings);
    for w in &warnings {
        eprintln!("tcat: {w}");
    }

    let stdout_is_tty = io::stdout().is_terminal();
    let syntax_override = options.syntax_name.as_deref();
    let operands: &[String] = if cli.files.is_empty() {
        &["-".to_string()]
    } else {
        &cli.files
    };

    let mut out = io::stdout().lock();
    let mut exit_code = 0i32;
    for operand in operands {
        if let Err(e) = cat_one(
            operand,
            &options,
            stdout_is_tty,
            syntax_override,
            &theme,
            &language_themes,
            &mut out,
        ) {
            // Rust ignores SIGPIPE by default and hands back a normal
            // error instead of dying outright the way a C program (real
            // `cat`) would -- so a downstream reader closing early (`tcat
            // bigfile | head`) needs to be handled explicitly the same
            // way: exit quietly, not with a "Broken pipe" error message.
            if e.kind() == io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            eprintln!("tcat: {operand}: {e}");
            exit_code = 1;
        }
    }
    // `process::exit` below skips normal drop glue, so anything crossterm
    // queued onto `out` but hasn't written yet needs an explicit flush.
    let _ = out.flush();
    std::process::exit(exit_code);
}

/// Print one operand (`-` for standard input, else a filename), matching
/// `cat`'s own byte-for-byte passthrough -- no line-ending or whitespace
/// transformation -- plus tico's own syntax coloring, layered on top only
/// when it's safe and meaningful: stdout is a terminal, `syntax_highlighting`
/// is on, the content is valid UTF-8, it's under the configured size limit,
/// and a language was actually detected (or forced).
fn cat_one(
    operand: &str,
    options: &tico::options::Options,
    stdout_is_tty: bool,
    syntax_override: Option<&str>,
    theme: &Theme,
    language_themes: &HashMap<String, Theme>,
    out: &mut impl Write,
) -> io::Result<()> {
    let (bytes, path): (Vec<u8>, Option<PathBuf>) = if operand == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        (buf, None)
    } else {
        let path = PathBuf::from(operand);
        let bytes = std::fs::read(&path)?;
        (bytes, Some(path))
    };

    let plain = !stdout_is_tty
        || !options.syntax_highlighting
        || bytes.len() as u64 > options.max_syntax_highlight_bytes;
    if !plain && let Ok(text) = std::str::from_utf8(&bytes) {
        let lang = tico::syntax::detect_with_override(path.as_deref(), text, syntax_override);
        if let Some(lang) = lang {
            let spans = tico::syntax::highlight(text, lang);
            print_highlighted(out, text, &spans, theme, language_themes)?;
            return Ok(());
        }
    }
    out.write_all(&bytes)
}

/// Colorize `text` (a whole operand's contents) line by line -- bounding
/// memory to one line's worth of per-byte style resolution at a time,
/// rather than the whole file -- and write the original bytes verbatim in
/// between (line terminators are never touched, matching `cat`).
fn print_highlighted(
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
/// char-indexed, unlike `ui.rs`'s line-grid renderer), since `tcat` just
/// streams contiguous byte ranges rather than laying out a fixed-width
/// screen row. Later spans overwrite earlier ones where they overlap
/// (innermost/most-specific wins), and each span's *own* language picks
/// its theme -- not the operand's detected language -- so an injected
/// heredoc body (Perl `<<SQL`, VCL `inline C`, ...) follows its own
/// language's `[syntax]` override, exactly like the `tico` editor.
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
        tico::theme::print_styled(out, &raw[i..j], style)?;
        i = j;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tico::syntax::Scope;

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
}
