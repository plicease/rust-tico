//! Tree-sitter-based syntax highlighting: language detection (filename ->
//! shebang -> modeline, in that order, all on by default) and highlight-span
//! computation.
//!
//! Highlight queries are vendored locally under `src/syntax/queries/`
//! rather than pulled from each crate at run time: most `tree-sitter-*`
//! crates on crates.io do bundle their grammar's real `queries/highlights.scm`,
//! but not consistently — the constant that exposes it is named differently
//! from crate to crate (`HIGHLIGHTS_QUERY`, `HIGHLIGHT_QUERY`,
//! `XML_HIGHLIGHT_QUERY`, ...), sometimes commented out, and a few crates
//! (perl, graphql, nim) don't publish one at all. Vendoring the same files
//! locally (see `queries/README.md`) sidesteps all of that and gives every
//! language a uniform `include_str!`. Perl's is hand-written, since no
//! upstream one exists; see `queries/perl.scm`.

mod languages;

pub use languages::{detect, LanguageDef};
pub(crate) use languages::{find_by_name, names};

/// Resolve a buffer's language, honoring an optional `-Y`/`--syntax` CLI
/// override (matching nano's `find_and_prime_applicable_syntax`): `"none"`
/// disables highlighting outright, a recognized name forces that language,
/// and an unrecognized name falls back to normal detection (nano shows an
/// "Unknown syntax name" alert in that last case; tico just falls back
/// silently, since there's no persistent status line to put it on this
/// early in startup).
pub fn detect_with_override(
    path: Option<&std::path::Path>,
    text: &str,
    syntax_override: Option<&str>,
) -> Option<&'static LanguageDef> {
    match syntax_override {
        Some("none") => None,
        Some(name) => find_by_name(name).or_else(|| detect(path, text)),
        None => detect(path, text),
    }
}

use tree_sitter::StreamingIterator;

/// A simplified highlight category that every language's much richer set of
/// tree-sitter capture names (`@function.method`, `@string.special`, ...)
/// gets bucketed into, each mapped to one terminal color in the UI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Comment,
    String,
    Number,
    Keyword,
    Function,
    Type,
    Constant,
    Variable,
    Module,
    Attribute,
    Tag,
}

/// One highlighted span of the buffer, as byte offsets into its text.
#[derive(Debug, Clone, Copy)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: HighlightKind,
}

/// Parse `text` with `lang`'s grammar and run its highlight query, bucketing
/// each capture into a `HighlightKind`. Spans are returned in the query's
/// natural (mostly outer-to-inner) order, so painting them in that order —
/// later spans overwriting earlier ones where they overlap — gives the
/// expected "innermost/most-specific wins" result (e.g. an interpolated
/// variable inside a string shows as a variable, the rest of the string
/// still shows as a string). Returns an empty vec if parsing or compiling
/// the query fails (should not normally happen for a vendored, tested
/// query, but a corrupt/huge buffer shouldn't be able to crash the editor).
pub fn highlight(text: &str, lang: &LanguageDef) -> Vec<HighlightSpan> {
    let language = (lang.language)();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else { return Vec::new() };
    let Ok(query) = tree_sitter::Query::new(&language, lang.highlights_query) else { return Vec::new() };

    let mut spans = Vec::new();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), text.as_bytes());
    while let Some((m, capture_ix)) = captures.next() {
        let capture = m.captures[*capture_ix];
        let name = query.capture_names()[capture.index as usize];
        if let Some(kind) = bucket_capture(name) {
            spans.push(HighlightSpan { start: capture.node.start_byte(), end: capture.node.end_byte(), kind });
        }
    }

    // Fallback pass: some grammars (Perl among them) don't give numeric
    // literals their own named node, so a query can't capture them at all.
    // Catch any leaf token that looks like a number and isn't already
    // covered by a real capture.
    add_numeric_fallback(&tree, text, &mut spans);

    // Heredoc language injection: when a heredoc's terminator names a known
    // language (e.g. `<<SQL`, `<<'HTML'`), re-highlight its body with that
    // language's own grammar instead of leaving it as one flat string.
    inject_heredocs(&tree, text, &mut spans);

    spans
}

/// Perl-specific for now: `tree-sitter-perl` is the only vendored grammar
/// whose node kinds this matches (`heredoc_start_identifier` /
/// `heredoc_body_statement`); other languages' trees simply won't contain
/// nodes with these kind names, so this is a no-op for them.
fn inject_heredocs(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut starts = Vec::new();
    collect_by_kind(tree.root_node(), "heredoc_start_identifier", &mut starts);
    if starts.is_empty() {
        return;
    }
    let mut bodies = Vec::new();
    collect_by_kind(tree.root_node(), "heredoc_body_statement", &mut bodies);
    starts.sort_by_key(|n| n.start_byte());
    bodies.sort_by_key(|n| n.start_byte());

    // Heredoc bodies appear in the source in the same order as their `<<TAG`
    // starts (Perl processes them in that order), so pairing by position is
    // reliable for the common case of one heredoc per statement. There's no
    // structural link in the tree between a start identifier and its body.
    for (start_id, body) in starts.iter().zip(bodies.iter()) {
        let raw = &text[start_id.start_byte()..start_id.end_byte()];
        let Some(lang) = languages::find_by_name(heredoc_language_name(raw)) else { continue };

        // The body node includes its own closing terminator line (as a
        // `heredoc_end_identifier` child); only the text before that should
        // be handed to the injected grammar.
        let mut body_end = body.end_byte();
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "heredoc_end_identifier" {
                body_end = child.start_byte();
                break;
            }
        }
        let body_start = body.start_byte();
        if body_end <= body_start || body_end > text.len() {
            continue;
        }

        let inner_spans = highlight(&text[body_start..body_end], lang);
        if inner_spans.is_empty() {
            // Leave the outer (Perl) query's own @string coloring in place
            // rather than blanking the body out.
            continue;
        }
        spans.retain(|s| !(s.start < body_end && s.end > body_start));
        spans.extend(inner_spans.into_iter().map(|s| HighlightSpan {
            start: s.start + body_start,
            end: s.end + body_start,
            kind: s.kind,
        }));
    }
}

/// Strip a heredoc terminator down to the bare language name: an optional
/// leading `~` (indented heredoc, `<<~SQL`) or `\` (no-interpolation
/// bareword, `<<\SQL`), then matching surrounding `'` or `"` quotes.
fn heredoc_language_name(raw: &str) -> &str {
    let s = raw.strip_prefix('~').unwrap_or(raw);
    let s = s.strip_prefix('\\').unwrap_or(s);
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn collect_by_kind<'a>(node: tree_sitter::Node<'a>, kind: &str, out: &mut Vec<tree_sitter::Node<'a>>) {
    if node.kind() == kind {
        out.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_by_kind(child, kind, out);
    }
}

fn add_numeric_fallback(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let bytes = text.as_bytes();
    let mut cursor = tree.walk();
    let mut nodes = Vec::new();
    collect_leaves(&mut cursor, &mut nodes);
    for node in nodes {
        let (start, end) = (node.start_byte(), node.end_byte());
        if end <= start || end > bytes.len() {
            continue;
        }
        if spans.iter().any(|s| s.start <= start && end <= s.end) {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(&bytes[start..end]) {
            if looks_numeric(text) {
                spans.push(HighlightSpan { start, end, kind: HighlightKind::Number });
            }
        }
    }
}

fn collect_leaves<'a>(cursor: &mut tree_sitter::TreeCursor<'a>, out: &mut Vec<tree_sitter::Node<'a>>) {
    loop {
        if cursor.node().child_count() == 0 {
            out.push(cursor.node());
        } else if cursor.goto_first_child() {
            collect_leaves(cursor, out);
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn looks_numeric(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let t = t.strip_prefix('-').or_else(|| t.strip_prefix('+')).unwrap_or(t);
    if t.is_empty() {
        return false;
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit() || c == '_');
    }
    let mut seen_digit = false;
    let mut seen_dot = false;
    for c in t.chars() {
        match c {
            '0'..='9' => seen_digit = true,
            '_' => {}
            '.' if !seen_dot => seen_dot = true,
            'e' | 'E' if seen_digit => {}
            _ => return false,
        }
    }
    seen_digit
}

/// Bucket a tree-sitter capture name (its first dot-separated segment, e.g.
/// "function" from "function.method") into a `HighlightKind`. Rare/
/// grammar-specific captures with no good general bucket (punctuation,
/// plain "text", spell-check hints, ...) are left uncolored (`None`) rather
/// than guessed at.
fn bucket_capture(name: &str) -> Option<HighlightKind> {
    // A couple of grammars (markdown's block query among them) use
    // "text.*"-prefixed names for things that don't fit the generic
    // buckets below; special-case the ones worth coloring before falling
    // back to the first-segment match.
    match name {
        "text.title" => return Some(HighlightKind::Keyword),
        "text.uri" | "text.reference" => return Some(HighlightKind::String),
        _ => {}
    }
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "comment" => HighlightKind::Comment,
        "string" | "character" | "char" | "escape" => HighlightKind::String,
        "number" | "float" => HighlightKind::Number,
        "boolean" | "constant" => HighlightKind::Constant,
        "keyword" | "conditional" | "repeat" | "storageclass" | "storage" | "include" | "exception"
        | "label" | "preproc" => HighlightKind::Keyword,
        "function" | "_function" | "method" | "constructor" => HighlightKind::Function,
        "type" | "_type" | "interface" => HighlightKind::Type,
        "variable" | "parameter" | "property" | "field" | "_name" => HighlightKind::Variable,
        "module" | "namespace" => HighlightKind::Module,
        "attribute" => HighlightKind::Attribute,
        "tag" => HighlightKind::Tag,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use languages::detect;

    /// Confirm every registered language's query actually compiles against
    /// its grammar (Query::new can fail at runtime even when the Rust code
    /// compiles fine, e.g. from a typo'd node name), and produces at least
    /// one non-empty span on a real snippet where that's a reasonable
    /// expectation.
    fn check(name: &str, path: &str, source: &str, want_at_least_one: bool) {
        let lang = languages::detect(Some(std::path::Path::new(path)), source)
            .unwrap_or_else(|| panic!("{name}: detection failed for {path}"));
        assert_eq!(lang.name, name, "detected wrong language for {path}");
        let spans = highlight(source, lang);
        if want_at_least_one {
            assert!(!spans.is_empty(), "{name}: expected at least one highlight span, got none");
        }
    }

    #[test]
    fn perl_highlights() {
        check(
            "perl",
            "test.pl",
            "#!/usr/bin/env perl\nuse strict;\nmy $x = 42; # comment\nsub foo { return \"hi\"; }\n",
            true,
        );
    }

    #[test]
    fn heredoc_injects_named_language() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "print <<SQL;\n   SELECT * FROM foo WHERE bar = 1\nSQL\n";
        let spans = highlight(src, lang);
        // "SELECT" and "FROM" should be captured as SQL keywords, at their
        // exact position within the outer Perl buffer -- not just colored
        // as one flat Perl string covering the whole heredoc body.
        let select_start = src.find("SELECT").unwrap();
        let from_start = src.find("FROM").unwrap();
        assert!(
            spans
                .iter()
                .any(|s| s.kind == HighlightKind::Keyword && s.start == select_start && s.end == select_start + 6),
            "expected a Keyword span for SELECT at {select_start}, got {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.kind == HighlightKind::Keyword && s.start == from_start && s.end == from_start + 4),
            "expected a Keyword span for FROM at {from_start}, got {spans:?}"
        );
    }

    #[test]
    fn heredoc_unknown_terminator_stays_plain_string() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "print <<EOF;\nsome text\nEOF\n";
        let spans = highlight(src, lang);
        // "EOF" isn't a recognized language name, so the body should still
        // be covered by the outer Perl query's plain @string capture.
        let body_start = src.find("some text").unwrap();
        assert!(
            spans.iter().any(|s| s.kind == HighlightKind::String && s.start <= body_start && s.end >= body_start + 9),
            "expected the heredoc body to remain a String span, got {spans:?}"
        );
    }

    #[test]
    fn override_forces_language_regardless_of_filename() {
        let lang = detect_with_override(Some(std::path::Path::new("foo.txt")), "", Some("perl")).unwrap();
        assert_eq!(lang.name, "perl");
    }

    #[test]
    fn override_none_disables_detection() {
        assert!(detect_with_override(Some(std::path::Path::new("foo.pl")), "", Some("none")).is_none());
    }

    #[test]
    fn override_unknown_name_falls_back_to_detection() {
        let lang = detect_with_override(Some(std::path::Path::new("foo.pl")), "", Some("boguslang")).unwrap();
        assert_eq!(lang.name, "perl");
    }

    #[test]
    fn listed_names_are_sorted_and_nonempty() {
        let names = names();
        assert!(names.contains(&"perl"));
        assert!(names.contains(&"sql"));
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn perl_extensions() {
        for ext in ["pl", "pm", "t", "xs"] {
            let path = format!("foo.{ext}");
            let lang = detect(Some(std::path::Path::new(&path)), "").expect("perl file should be detected");
            assert_eq!(lang.name, "perl");
        }
    }

    #[test]
    fn all_languages_query_compiles_and_highlights() {
        let cases: &[(&str, &str, &str)] = &[
            ("c", "a.c", "#include <stdio.h>\nint main() { return 0; } // hi\n"),
            ("cpp", "a.cpp", "#include <iostream>\nclass Foo { public: int x; }; // hi\n"),
            ("rust", "a.rs", "fn main() { let x = 1; } // hi\n"),
            ("go", "a.go", "package main\nfunc main() { x := 1 } // hi\n"),
            ("python", "a.py", "def foo():\n    x = 1  # hi\n    return x\n"),
            ("bash", "a.sh", "#!/bin/bash\nfoo() { echo hi; } # comment\n"),
            ("json", "a.json", "{\"a\": 1, \"b\": \"c\"}\n"),
            ("yaml", "a.yaml", "a: 1\nb: \"c\" # hi\n"),
            ("toml", "a.toml", "a = 1\nb = \"c\" # hi\n"),
            ("html", "a.html", "<html><!-- hi --><body>x</body></html>\n"),
            ("css", "a.css", "/* hi */ .a { color: red; }\n"),
            ("sql", "a.sql", "SELECT * FROM foo WHERE x = 1; -- hi\n"),
            ("javascript", "a.js", "function foo() { return 1; } // hi\n"),
            ("typescript", "a.ts", "function foo(): number { return 1; } // hi\n"),
            ("java", "a.java", "class Foo { void bar() {} } // hi\n"),
            ("ruby", "a.rb", "def foo\n  1 # hi\nend\n"),
            ("php", "a.php", "<?php\nfunction foo() { return 1; } // hi\n"),
            ("csharp", "a.cs", "class Foo { void Bar() {} } // hi\n"),
            ("make", "Makefile", "all:\n\techo hi # comment\n"),
            ("fortran", "a.f90", "program hi\n  integer :: x = 1\nend program hi\n"),
            ("markdown", "a.md", "# Title\n\nSome *text*.\n"),
            ("toml", "Cargo.toml", "[package]\nname = \"x\"\n"),
            ("haskell", "a.hs", "main = putStrLn \"hi\" -- comment\n"),
            ("scala", "a.scala", "object Foo { def bar() = 1 } // hi\n"),
            ("objc", "a.m", "@interface Foo : NSObject @end // hi\n"),
            ("r", "a.r", "foo <- function(x) { x + 1 } # hi\n"),
            ("julia", "a.jl", "function foo(x)\n    x + 1 # hi\nend\n"),
            ("xml", "a.xml", "<!-- hi --><root><a>1</a></root>\n"),
            ("diff", "a.diff", "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n"),
            ("ini", "a.ini", "[section]\n; comment\nkey = value\n"),
            ("elixir", "a.ex", "defmodule Foo do\n  def bar, do: 1 # hi\nend\n"),
            ("elm", "a.elm", "foo x = x + 1 -- hi\n"),
            ("zig", "a.zig", "pub fn main() void { } // hi\n"),
            ("dart", "a.dart", "void main() { print('hi'); } // hi\n"),
            ("scss", "a.scss", "// hi\n.a { color: red; }\n"),
            ("proto", "a.proto", "// hi\nmessage Foo { string bar = 1; }\n"),
            ("cmake", "CMakeLists.txt", "# hi\nadd_executable(foo bar.c)\n"),
            ("nix", "a.nix", "# hi\n{ foo = 1; }\n"),
            ("vim", "a.vim", "\" hi\nlet g:foo = 1\n"),
            ("lua", "a.lua", "-- hi\nfunction foo() return 1 end\n"),
            ("swift", "a.swift", "func foo() -> Int { return 1 } // hi\n"),
            ("go", "a.go", "package main\n// hi\nfunc main() {}\n"),
        ];
        for (name, path, source) in cases {
            check(name, path, source, true);
        }
    }
}
