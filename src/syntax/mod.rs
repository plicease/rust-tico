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

    spans
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
