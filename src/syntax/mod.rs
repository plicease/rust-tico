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

pub use languages::{LanguageDef, detect};
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

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tree_sitter::StreamingIterator;

/// An interned highlight scope name in Helix's vocabulary
/// (`keyword.control.import`, `constant.numeric`, `markup.heading`, ...).
/// The theme (`crate::theme`) maps scopes to styles by longest dotted
/// prefix, so the full capture name is kept — a theme may well style
/// `keyword.control.import` differently from plain `keyword`.
///
/// Interning keeps `HighlightSpan` small and `Copy` (a `u16` rather than a
/// heap string per span, of which a big buffer has tens of thousands) and
/// gives the theme a cheap key to memoize resolution on. The set of
/// distinct names is bounded by what the vendored queries contain, a few
/// hundred at most, so leaking them is fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scope(u16);

struct Interner {
    names: Vec<&'static str>,
    ids: HashMap<&'static str, u16>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(|| {
        Mutex::new(Interner {
            names: Vec::new(),
            ids: HashMap::new(),
        })
    })
}

impl Scope {
    /// The scope for a (Helix-vocabulary) name, interning it if new.
    pub fn intern(name: &str) -> Scope {
        let mut it = interner().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = it.ids.get(name) {
            return Scope(id);
        }
        let id = u16::try_from(it.names.len()).expect("more than 65535 distinct highlight scopes");
        let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
        it.names.push(leaked);
        it.ids.insert(leaked, id);
        Scope(id)
    }

    pub fn name(self) -> &'static str {
        interner().lock().unwrap_or_else(|e| e.into_inner()).names[self.0 as usize]
    }
}

/// One highlighted span of the buffer, as byte offsets into its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub scope: Scope,
    /// `LanguageDef::name` of the grammar whose query produced this span.
    /// Usually the buffer's own language, but a heredoc body injected with
    /// another language (`<<SQL` in Perl) carries that language, so the
    /// renderer can paint it with *its* theme rather than the buffer's.
    pub language: &'static str,
}

/// Parse `text` with `lang`'s grammar and run its highlight query, tagging
/// each capture with its (normalized) scope. Spans are returned in the
/// query's natural (mostly outer-to-inner) order, so painting them in that
/// order — later spans overwriting earlier ones where they overlap — gives
/// the expected "innermost/most-specific wins" result (e.g. an interpolated
/// variable inside a string shows as a variable, the rest of the string
/// still shows as a string). Returns an empty vec if parsing or compiling
/// the query fails (should not normally happen for a vendored, tested
/// query, but a corrupt/huge buffer shouldn't be able to crash the editor).
pub fn highlight(text: &str, lang: &'static LanguageDef) -> Vec<HighlightSpan> {
    let language = (lang.language)();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let Ok(query) = tree_sitter::Query::new(&language, lang.highlights_query) else {
        return Vec::new();
    };

    // Capture index -> scope, resolved once per query rather than once per
    // captured node (there are as many of those as tokens in the file).
    let scopes: Vec<Option<Scope>> = query
        .capture_names()
        .iter()
        .map(|name| normalize_capture(name).map(|n| Scope::intern(&n)))
        .collect();

    let mut spans = Vec::new();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), text.as_bytes());
    while let Some((m, capture_ix)) = captures.next() {
        let capture = m.captures[*capture_ix];
        if let Some(scope) = scopes[capture.index as usize] {
            spans.push(HighlightSpan {
                start: capture.node.start_byte(),
                end: capture.node.end_byte(),
                scope,
                language: lang.name,
            });
        }
    }

    // Fallback pass: some grammars (Perl among them) don't give numeric
    // literals their own named node, so a query can't capture them at all.
    // Catch any leaf token that looks like a number and isn't already
    // covered by a real capture.
    add_numeric_fallback(&tree, text, lang.name, &mut spans);

    // Heredoc language injection: when a heredoc's terminator names a known
    // language (e.g. `<<SQL`, `<<'HTML'`), re-highlight its body with that
    // language's own grammar instead of leaving it as one flat string.
    inject_heredocs(&tree, text, &mut spans);

    // Varnish inline C: highlight the body of a `C{ ... }C` block with the C
    // grammar instead of leaving it one flat string.
    inject_inline_c(&tree, text, &mut spans);

    spans
}

/// Heredoc language injection: when a heredoc's terminator names a known
/// language (`<<SQL`, `<<~'VCL'`, `cat <<HTML`, PHP's `<<<JSON`),
/// re-highlight its body with that language's own grammar instead of
/// leaving it one flat string. Two routes, chosen by which node kinds the
/// buffer's grammar produces: Perl's heredoc nodes can't be trusted for
/// the body, so its bodies are found by text; Bash, Ruby and PHP give the
/// body a real node with a clean range.
fn inject_heredocs(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    inject_perl_heredocs(tree, text, spans);
    inject_tree_heredocs(tree, text, spans);
}

/// Replace whatever the outer grammar made of `text[start..end]` with the
/// spans `lang`'s own grammar produces for it, offset back into `text`.
/// If that comes to nothing, the outer coloring (typically one flat
/// string) is left in place rather than blanking the range out.
fn inject_range(
    text: &str,
    spans: &mut Vec<HighlightSpan>,
    start: usize,
    end: usize,
    lang: &'static LanguageDef,
) {
    if end <= start || end > text.len() {
        return;
    }
    let inner = highlight(&text[start..end], lang);
    if inner.is_empty() {
        return;
    }
    spans.retain(|s| !(s.start < end && s.end > start));
    spans.extend(inner.into_iter().map(|s| HighlightSpan {
        start: s.start + start,
        end: s.end + start,
        ..s
    }));
}

/// Bash (`heredoc_start` / `heredoc_body`), Ruby (`heredoc_beginning` /
/// `heredoc_body`) and PHP (`heredoc_start` / `heredoc_body`, or
/// `nowdoc_body` for a nowdoc): these grammars give every heredoc body
/// its own node, wherever the heredoc sits, so each start marker is paired
/// with the first unclaimed body after it -- bodies follow their markers
/// in source order, including several on one line. Ruby's body node ends
/// with the `heredoc_end` line, which is trimmed off. No other vendored
/// grammar produces these kinds, so this is a no-op elsewhere.
fn inject_tree_heredocs(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut starts = Vec::new();
    for kind in ["heredoc_start", "heredoc_beginning"] {
        collect_by_kind(tree.root_node(), kind, &mut starts);
    }
    if starts.is_empty() {
        return;
    }
    let mut bodies = Vec::new();
    for kind in ["heredoc_body", "nowdoc_body"] {
        collect_by_kind(tree.root_node(), kind, &mut bodies);
    }
    starts.sort_by_key(|n| n.start_byte());
    bodies.sort_by_key(|n| n.start_byte());

    let mut next_body = 0;
    for start_id in starts {
        let Some(i) = bodies
            .iter()
            .skip(next_body)
            .position(|b| b.start_byte() >= start_id.end_byte())
        else {
            break;
        };
        let body = bodies[next_body + i];
        next_body += i + 1;

        let raw = &text[start_id.byte_range()];
        let Some(lang) = languages::find_by_name(heredoc_language_name(raw)) else {
            continue;
        };
        let mut body_end = body.end_byte();
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "heredoc_end" {
                body_end = child.start_byte();
                break;
            }
        }
        inject_range(text, spans, body.start_byte(), body_end, lang);
    }
}

/// Perl (`heredoc_start_identifier`; no other vendored grammar produces
/// that kind, so this is a no-op elsewhere).
///
/// Bodies are located by text rather than from the tree. The Perl grammar
/// only produces a `heredoc_body_statement` when the statement carrying
/// the `<<TAG` ends on that same line; inside a multi-line construct
/// (`content => <<~'VCL',` in a hash) it emits no body node and parses the
/// body as Perl code, so pairing start identifiers with body nodes would
/// silently skip exactly the heredocs that most need re-highlighting, and
/// leave Perl's coloring of the body in place (where `-A` in a VCL header
/// name reads as a file test operator). Perl's own rule is simple enough
/// to apply directly: the body starts on the line after the `<<TAG` (or
/// after the previous heredoc's terminator, when several share a line) and
/// ends at the first line that is exactly the terminator, with leading
/// whitespace allowed for `<<~`.
fn inject_perl_heredocs(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut starts = Vec::new();
    collect_by_kind(tree.root_node(), "heredoc_start_identifier", &mut starts);
    if starts.is_empty() {
        return;
    }
    starts.sort_by_key(|n| n.start_byte());

    // Where the next body may begin: heredocs are consumed in source order,
    // so a second `<<TAG` on the same line gets the lines after the first
    // one's terminator.
    let mut next_body = 0;
    for start_id in starts {
        let raw = &text[start_id.byte_range()];
        let indented = raw.starts_with('~');
        let name = heredoc_language_name(raw);
        let after_line = text[start_id.end_byte()..]
            .find('\n')
            .map_or(text.len(), |i| start_id.end_byte() + i + 1);
        let body_start = after_line.max(next_body);
        let Some((body_end, after_terminator)) =
            find_heredoc_terminator(text, body_start, name, indented)
        else {
            // Unterminated: the body runs to end of file and Perl itself
            // would reject it. Nothing sensible to inject, for this heredoc
            // or any that follow it.
            break;
        };
        next_body = after_terminator;

        let Some(lang) = languages::find_by_name(name) else {
            continue;
        };
        inject_range(text, spans, body_start, body_end, lang);
    }
}

/// Find the first line at or after byte offset `from` that is exactly
/// `name` (after an optional `\r`, and after leading whitespace when
/// `indented`, i.e. for `<<~`). Returns the byte offset where that line
/// starts, which ends the heredoc body, and the offset just past it.
fn find_heredoc_terminator(
    text: &str,
    from: usize,
    name: &str,
    indented: bool,
) -> Option<(usize, usize)> {
    let mut pos = from;
    while pos < text.len() {
        let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
        let line = text[pos..line_end]
            .strip_suffix('\r')
            .unwrap_or(&text[pos..line_end]);
        let candidate = if indented { line.trim_start() } else { line };
        if candidate == name {
            return Some((pos, (line_end + 1).min(text.len())));
        }
        pos = line_end + 1;
    }
    None
}

/// VCL-specific: the vendored VCL grammar lexes a Varnish `C{ ... }C` block
/// as a single `inline_c` token (its query colors it as a string as a
/// fallback). Re-highlight the body with the C grammar and color the two
/// delimiters, so the block reads as the C it is. No other grammar produces
/// `inline_c` nodes, so this is a no-op elsewhere.
fn inject_inline_c(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut blocks = Vec::new();
    collect_by_kind(tree.root_node(), "inline_c", &mut blocks);
    if blocks.is_empty() {
        return;
    }
    let Some(c) = languages::find_by_name("c") else {
        return;
    };
    let delimiter = Scope::intern("punctuation.special");
    for block in blocks {
        let (start, end) = (block.start_byte(), block.end_byte());
        // `C{` and `}C` are two bytes each; the token can't be shorter.
        if end < start + 4 || end > text.len() {
            continue;
        }
        let (body_start, body_end) = (start + 2, end - 2);
        let inner_spans = highlight(&text[body_start..body_end], c);
        spans.retain(|s| !(s.start < end && s.end > start));
        for (a, b) in [(start, body_start), (body_end, end)] {
            spans.push(HighlightSpan {
                start: a,
                end: b,
                scope: delimiter,
                language: "vcl",
            });
        }
        spans.extend(inner_spans.into_iter().map(|s| HighlightSpan {
            start: s.start + body_start,
            end: s.end + body_start,
            ..s
        }));
    }
}

/// Strip a heredoc marker down to the bare language name, whichever
/// grammar produced it: an optional leading `<<` (Ruby's marker node is the
/// whole `<<~SQL`; Perl's, Bash's and PHP's start after the operator),
/// then `~` (indented, `<<~SQL`) or `-` (Ruby's `<<-SQL`), then `\`
/// (Perl's no-interpolation bareword, `<<\SQL`), then matching surrounding
/// `'` or `"` quotes.
fn heredoc_language_name(raw: &str) -> &str {
    let s = raw.strip_prefix("<<").unwrap_or(raw);
    let s = s.strip_prefix(['~', '-']).unwrap_or(s);
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

fn collect_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if node.kind() == kind {
        out.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_by_kind(child, kind, out);
    }
}

fn add_numeric_fallback(
    tree: &tree_sitter::Tree,
    text: &str,
    language: &'static str,
    spans: &mut Vec<HighlightSpan>,
) {
    let bytes = text.as_bytes();
    let mut cursor = tree.walk();
    let mut nodes = Vec::new();
    collect_leaves(&mut cursor, &mut nodes);

    // "Is this leaf already covered by a real capture?" used to be a
    // `spans.iter().any(...)` linear scan run for *every* leaf -- with
    // both leaf count and span count growing with file size, that's
    // O(leaves * spans), i.e. quadratic, and made opening or editing a
    // large file dramatically slower than it needed to be.
    //
    // Every span here comes from a node of this same parse tree (this
    // runs before heredoc injection adds any that don't), so any two
    // spans either nest or are disjoint -- they can never partially
    // overlap. That laminar property is what makes a sorted-start sweep
    // with a stack of "currently enclosing" spans a correct, linear-time
    // (after an O(n log n) sort) replacement: a leaf is already covered
    // exactly when the stack is non-empty once expired (ended-before-here)
    // entries have been popped off it.
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i].start);
    let mut next = 0usize;
    let mut active: Vec<usize> = Vec::new();

    for node in nodes {
        let (start, end) = (node.start_byte(), node.end_byte());
        if end <= start || end > bytes.len() {
            continue;
        }
        while next < order.len() && spans[order[next]].start <= start {
            let idx = order[next];
            next += 1;
            // Drop anything on the stack that already ended before this
            // span begins -- a disjoint sibling, not really an enclosing
            // span, so it must not linger and cause false "covered" hits
            // for later leaves.
            while let Some(&top) = active.last()
                && spans[top].end <= spans[idx].start
            {
                active.pop();
            }
            active.push(idx);
        }
        while let Some(&top) = active.last()
            && spans[top].end <= start
        {
            active.pop();
        }
        if !active.is_empty() {
            continue;
        }
        if let Ok(leaf_text) = std::str::from_utf8(&bytes[start..end])
            && looks_numeric(leaf_text)
        {
            spans.push(HighlightSpan {
                start,
                end,
                scope: Scope::intern("constant.numeric"),
                language,
            });
        }
    }
}

fn collect_leaves<'a>(
    cursor: &mut tree_sitter::TreeCursor<'a>,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
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
    let t = t
        .strip_prefix('-')
        .or_else(|| t.strip_prefix('+'))
        .unwrap_or(t);
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

/// Translate a raw tree-sitter capture name into Helix's scope vocabulary
/// (the one themes are written against), or `None` for captures that
/// shouldn't produce a highlight at all.
///
/// The vendored queries come from each grammar's own upstream repo, so they
/// are a mix of conventions: the tree-sitter CLI's (`function.method`,
/// `variable.builtin` — already Helix-compatible), nvim-treesitter's
/// (`conditional`, `repeat`, `include`, `number`, `property`, `text.title`
/// ...), and the odd grammar-specific one (elm suffixes everything with
/// `.elm`). Everything here is a *renaming*; the actual choice of color is
/// entirely the theme's, via longest-prefix lookup, so a name that has no
/// alias is passed through unchanged and still falls back sensibly
/// (`constant.macro` -> a theme's `constant`).
///
/// Captures dropped outright: `_`-prefixed helper captures (used only for
/// predicates), `spell`/`nospell`, `none`, `error`, `embedded`, and the
/// nvim `text.{note,warning,danger}` TODO markers, which have no Helix
/// equivalent — leaving them out means the enclosing comment's style shows
/// through, which is what a theme user would expect.
fn normalize_capture(raw: &str) -> Option<String> {
    let name = raw.strip_suffix(".elm").unwrap_or(raw);
    // `_foo`: predicate-only helper captures; `source.*`: injection markers.
    if name.starts_with('_') || name.starts_with("source.") {
        return None;
    }
    // Exact renames first: these change more than the leading segment.
    let exact = match name {
        "spell" | "nospell" | "none" | "error" | "embedded" | "clean" | "text.note"
        | "text.warning" | "text.danger" => return None,
        "number" => "constant.numeric",
        "number.float" | "float" => "constant.numeric.float",
        "boolean" => "constant.builtin.boolean",
        "character" | "char" | "character.special" => "constant.character",
        "escape" | "string.escape" | "character.escape" => "constant.character.escape",
        "string.regex" | "string.special.regex" => "string.regexp",
        "string.special.uri" => "string.special.url",
        "string.documentation" => "string",
        "conditional" | "keyword.conditional" | "keyword.conditional.ternary" => {
            "keyword.control.conditional"
        }
        "repeat" | "keyword.repeat" => "keyword.control.repeat",
        "include" | "import" | "keyword.import" | "meta.import" => "keyword.control.import",
        "exception" | "keyword.exception" => "keyword.control.exception",
        "keyword.return" => "keyword.control.return",
        "keyword.control" => "keyword.control",
        "keyword.coroutine" | "keyword.debug" | "keyword.other" | "keyword.other.port" => "keyword",
        "keyword.type" | "storage.type" => "keyword.storage.type",
        "keyword.modifier" | "storageclass" | "type.qualifier" => "keyword.storage.modifier",
        "preproc" | "define" | "macro" | "custom_directive" => "keyword.directive",
        "method" | "method.call" | "function.method.call" | "function.method.builtin" => {
            "function.method"
        }
        "function.call" | "local.function" => "function",
        "function.macro.builtin" => "function.macro",
        "parameter" | "parameter.builtin" => "variable.parameter",
        "property" | "property.definition" | "field" | "variable.member" => "variable.other.member",
        "module" | "module.builtin" => "namespace",
        "type.definition" | "interface" | "union" => "type",
        "tag.attribute" => "attribute",
        "delimiter" => "punctuation.delimiter",
        "comment.doc" | "comment.doc.__attribute__" | "comment.documentation" => {
            "comment.block.documentation"
        }
        "text.title" => "markup.heading",
        "text.uri" => "markup.link.url",
        "text.reference" => "markup.link.label",
        "text.literal" => "markup.raw",
        "text.emphasis" => "markup.italic",
        "text.strong" => "markup.bold",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact.to_string());
    }
    // Otherwise rename just the leading segment where nvim's differs from
    // Helix's, keeping any more specific tail (`property.foo` ->
    // `variable.other.member.foo`).
    let (head, tail) = match name.split_once('.') {
        Some((h, t)) => (h, Some(t)),
        None => (name, None),
    };
    let head = match head {
        "number" => "constant.numeric",
        "character" => "constant.character",
        "conditional" => "keyword.control.conditional",
        "repeat" => "keyword.control.repeat",
        "include" => "keyword.control.import",
        "exception" => "keyword.control.exception",
        "preproc" => "keyword.directive",
        "method" => "function.method",
        "parameter" => "variable.parameter",
        "property" | "field" => "variable.other.member",
        "module" => "namespace",
        "text" => "markup",
        other => other,
    };
    Some(match tail {
        Some(t) => format!("{head}.{t}"),
        None => head.to_string(),
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
            assert!(
                !spans.is_empty(),
                "{name}: expected at least one highlight span, got none"
            );
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

    /// The vendored VCL grammar is a fork of ntsk/tree-sitter-vcl extended
    /// for Fastly's dialect (see `grammars/tree-sitter-vcl/README.md`).
    /// Every Fastly-only construct here must parse without an ERROR node,
    /// and the query must color the ones with dedicated captures.
    #[test]
    fn vcl_fastly_dialect_parses_and_highlights() {
        let source = concat!(
            "pragma optional_param geoip_opt_in true;\n",
            "backend F_origin {\n",
            "  .host = \"origin.example.com\";\n",
            "  .ssl = true;\n",
            "  .probe = { .request = \"HEAD / HTTP/1.1\" \"Connection: close\"; .timeout = 2s; }\n",
            "}\n",
            "table redirects { \"/old\": \"/new\", }\n",
            "table limits INTEGER { \"max\": 100 }\n",
            "director pool random { .quorum = 20%; { .backend = F_origin; .weight = 1; } }\n",
            "penaltybox pb {}\n",
            "ratecounter rc {}\n",
            "sub is_admin BOOL { return req.http.Cookie:admin == \"1\"; }\n",
            "sub vcl_recv {\n",
            "#FASTLY recv\n",
            "  declare local var.x STRING;\n",
            "  set var.x = std.tolower(req.http.Host) \"/\" req.url;\n",
            "  set req.hash += req.url;\n",
            "  set req.http.X = if(req.url ~ \"^/x\", \"yes\", \"no\");\n",
            "  if ((req.url ~ \"^/y\") && !req.http.Z) { error 601 var.x; }\n",
            "  add req.http.Vary = \"Accept\";\n",
            "  remove req.http.Cookie;\n",
            "  goto done;\n",
            "  done:\n",
            "  return (lookup);\n",
            "}\n",
            "sub vcl_fetch {\n",
            "  if (beresp.status == 503 && req.restarts < 1) { restart; }\n",
            "  esi;\n",
            "  return (deliver);\n",
            "}\n",
            "sub vcl_error {\n",
            "  synthetic {\"<p>\"} obj.status {\"</p>\"};\n",
            "  log \"syslog \" req.service_id \" x :: \" req.url;\n",
            "  include \"snippet\";\n",
            "  return (deliver);\n",
            "}\n",
        );
        let lang = languages::find_by_name("vcl").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "Fastly VCL failed to parse:\n{}",
            tree.root_node().to_sexp()
        );

        // The renderer paints spans in order, so for a node captured more
        // than once the last span wins; mirror that here.
        let spans = highlight(source, lang);
        let scope_of = |needle: &str| {
            let start = source.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(
            scope_of("#FASTLY recv").as_deref(),
            Some("keyword.directive")
        );
        assert_eq!(scope_of("table").as_deref(), Some("keyword"));
        assert_eq!(scope_of("declare").as_deref(), Some("keyword"));
        assert_eq!(scope_of("STRING").as_deref(), Some("type"));
        assert_eq!(scope_of("INTEGER").as_deref(), Some("type"));
        assert_eq!(scope_of("+=").as_deref(), Some("operator"));
        assert_eq!(scope_of("20%").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("tolower").as_deref(), Some("function"));
        assert_eq!(scope_of("std").as_deref(), Some("namespace"));
        assert_eq!(scope_of("vcl_recv").as_deref(), Some("function.builtin"));
        assert_eq!(scope_of("lookup").as_deref(), Some("constant"));
        assert_eq!(scope_of("done").as_deref(), Some("label"));
        assert_eq!(
            scope_of(".quorum").as_deref(),
            Some("variable.other.member")
        );
        assert_eq!(scope_of("random").as_deref(), Some("type"));
    }

    /// Varnish `C{ ... }C` blocks: the body is re-highlighted as C (spans
    /// carrying the C language, so the renderer uses C's theme) and the
    /// delimiters get their own color, at top level and inside a sub.
    #[test]
    fn vcl_inline_c_is_highlighted_as_c() {
        let source = concat!(
            "C{\n",
            "#include <stdio.h>\n",
            "static int counter = 0;\n",
            "}C\n",
            "sub vcl_recv {\n",
            "  C{ if (1) { counter++; } }C\n",
            "  return (pass);\n",
            "}\n",
            "C{}C\n",
        );
        let lang = languages::find_by_name("vcl").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );

        let spans = highlight(source, lang);
        let at = |needle: &str| {
            let start = source.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        // C keywords inside the block come from the C grammar and query.
        assert_eq!(at("static"), Some(("keyword".to_string(), "c")));
        assert_eq!(at("int"), Some(("type".to_string(), "c")));
        assert_eq!(at("if"), Some(("keyword".to_string(), "c")));
        // The delimiters are colored, and as VCL.
        assert_eq!(at("C{"), Some(("punctuation.special".to_string(), "vcl")));
        assert_eq!(at("}C"), Some(("punctuation.special".to_string(), "vcl")));
        // Nothing from the VCL query survives inside a block.
        let first_block_end = source.find("}C").unwrap() + 2;
        assert!(
            spans
                .iter()
                .filter(|s| s.start < first_block_end)
                .all(|s| s.language == "c" || s.scope.name() == "punctuation.special"),
            "VCL spans leaked into the C block"
        );
        // Statements after the block are still VCL.
        assert_eq!(
            at("return"),
            Some(("keyword.control.return".to_string(), "vcl"))
        );
    }

    /// An ordinary rule with no standard target name must still get color:
    /// the crate's make query left `foo: bar` blank (only the `:` was
    /// captured), which read as "highlighting is off" in a two-line Makefile.
    #[test]
    fn make_plain_rule_is_colored() {
        let src = "foo: bar baz.o\n\tcc -o foo bar\n\nall: foo\n";
        let lang = languages::detect(Some(std::path::Path::new("Makefile")), src).unwrap();
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("foo").as_deref(), Some("function"));
        assert_eq!(at("bar").as_deref(), Some("string.special.path"));
        assert_eq!(at("baz.o").as_deref(), Some("string.special.path"));
        // A standard target keeps the crate's more specific capture.
        assert_eq!(at("all").as_deref(), Some("constant.macro"));
    }

    /// A Fastly VCL snippet is a bare run of statements with no `sub` around
    /// it, and that is also what a `<<VCL` heredoc usually holds. It must
    /// parse cleanly: when it went through error recovery instead, the
    /// lexer split hyphenated header names and left odd characters
    /// uncolored (`AWS-Access-Key-Id` lost its second `A`).
    #[test]
    fn vcl_snippet_without_sub_parses_and_keeps_hyphenated_names_whole() {
        let snippet = concat!(
            "set req.http.AWS-Access-Key-Id = \"AKIA\";\n",
            "set req.http.Slack-Bot-Token = \"xoxb\";\n",
            "if (req.url ~ \"^/x\") { error 404; }\n",
            "C{ int x; }C\n",
            "include \"snip\";\n",
        );
        let lang = languages::find_by_name("vcl").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(snippet, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );

        // The same snippet as an indented, quoted heredoc inside a Perl hash.
        let src = format!(
            "my %h = (\n    content => <<~'VCL',\n{}    VCL\n);\n",
            snippet
                .lines()
                .map(|l| format!("        {l}\n"))
                .collect::<String>()
        );
        let perl = languages::detect(Some(std::path::Path::new("a.pl")), &src).unwrap();
        let spans = highlight(&src, perl);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(at("set"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(
            at("AWS-Access-Key-Id"),
            Some(("variable".to_string(), "vcl"))
        );
        assert_eq!(at("Slack-Bot-Token"), Some(("variable".to_string(), "vcl")));
        assert_eq!(at("error"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(at("int"), Some(("type".to_string(), "c")));
        // Every non-blank character of the header name lines is covered by
        // some VCL span, so nothing renders in the terminal's plain color.
        for line in [
            "set req.http.AWS-Access-Key-Id = \"AKIA\";",
            "set req.http.Slack-Bot-Token = \"xoxb\";",
        ] {
            let start = src.find(line).unwrap();
            for (i, ch) in line.char_indices() {
                if ch == ' ' {
                    continue;
                }
                let pos = start + i;
                assert!(
                    spans
                        .iter()
                        .any(|s| s.language == "vcl" && s.start <= pos && pos < s.end),
                    "{ch:?} at column {i} of {line:?} has no VCL span"
                );
            }
        }
    }

    #[test]
    fn numeric_fallback_finds_disjoint_and_nested_numbers() {
        // Perl doesn't give numeric literals their own captured node, so
        // these all rely on `add_numeric_fallback`'s sweep. Several plain
        // disjoint numbers plus one inside a nested capture (an array
        // index expression) exercises the exact shape of bug its
        // stack-based rewrite had to avoid: a sibling span that already
        // ended must not linger on the stack and cause a later, unrelated
        // leaf to be wrongly treated as "already covered".
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "my @a = (1, 22, 333); my $x = $a[444];\n";
        let spans = highlight(src, lang);
        for needle in ["1", "22", "333", "444"] {
            let start = src.find(needle).unwrap();
            let end = start + needle.len();
            assert!(
                spans.iter().any(|s| s.scope.name() == "constant.numeric"
                    && s.start == start
                    && s.end == end),
                "expected a constant.numeric span for {needle:?} at {start}..{end}, got: {spans:?}"
            );
        }
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
            spans.iter().any(|s| s.scope.name().starts_with("keyword")
                && s.start == select_start
                && s.end == select_start + 6),
            "expected a keyword span for SELECT at {select_start}, got {spans:?}"
        );
        assert!(
            spans.iter().any(|s| s.scope.name().starts_with("keyword")
                && s.start == from_start
                && s.end == from_start + 4),
            "expected a keyword span for FROM at {from_start}, got {spans:?}"
        );
    }

    /// `<<VCL` in Perl injects the VCL grammar, and a `C{ ... }C` block inside
    /// that heredoc is injected as C in turn: highlight() recurses, so each
    /// layer's injections run on the body it was handed.
    #[test]
    fn heredoc_vcl_with_inline_c_nests_both_injections() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = concat!(
            "my $vcl = <<VCL;\n",
            "sub vcl_recv {\n",
            "  C{ static int hits = 0; }C\n",
            "  return (pass);\n",
            "}\n",
            "VCL\n",
            "print $vcl;\n",
        );
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(at("sub"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(
            at("return"),
            Some(("keyword.control.return".to_string(), "vcl"))
        );
        assert_eq!(at("C{"), Some(("punctuation.special".to_string(), "vcl")));
        assert_eq!(at("static"), Some(("keyword".to_string(), "c")));
        assert_eq!(at("int"), Some(("type".to_string(), "c")));
        assert_eq!(at("}C"), Some(("punctuation.special".to_string(), "vcl")));
        // The surrounding Perl is untouched.
        assert_eq!(at("print").map(|(_, l)| l), Some("perl"));
    }

    #[test]
    fn heredoc_spans_carry_the_injected_language() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "my $x = 1;\nprint <<SQL;\n   SELECT 1\nSQL\n";
        let spans = highlight(src, lang);
        let select = src.find("SELECT").unwrap();
        let sql_span = spans
            .iter()
            .find(|s| s.start == select)
            .expect("SELECT should be highlighted");
        assert_eq!(sql_span.language, "sql", "{sql_span:?}");
        // Everything outside the heredoc body is still Perl's, including
        // the numeric-fallback span for `1`.
        let one = src.find('1').unwrap();
        assert!(
            spans.iter().any(|s| s.start == one && s.language == "perl"),
            "{spans:?}"
        );
        assert!(
            spans
                .iter()
                .filter(|s| s.language == "sql")
                .all(|s| s.start >= src.find("   SELECT").unwrap()),
            "no sql-tagged span may leak outside the heredoc body: {spans:?}"
        );
    }

    /// Bash, Ruby and PHP heredocs inject the same way Perl's do, through
    /// the grammar's own body nodes: including Bash's `<<-`, Ruby's
    /// indented `<<~` and `<<-`, PHP's nowdoc, and quoted terminators.
    #[test]
    fn heredocs_inject_in_bash_ruby_and_php() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "bash",
                "a.sh",
                "psql <<SQL\nSELECT * FROM foo WHERE bar = 1\nSQL\necho done\n",
            ),
            (
                "bash",
                "a.sh",
                "psql <<-'SQL'\n\tSELECT * FROM foo\n\tSQL\n",
            ),
            (
                "ruby",
                "a.rb",
                "q = <<~SQL\n  SELECT * FROM foo\n  WHERE bar = 1\nSQL\nputs q\n",
            ),
            (
                "ruby",
                "a.rb",
                "q = <<-'SQL'\n  SELECT * FROM foo\n  SQL\nputs q\n",
            ),
            (
                "ruby",
                "a.rb",
                "h = { a: <<~SQL, b: 1 }\n  SELECT * FROM foo\nSQL\n",
            ),
            (
                "php",
                "a.php",
                "<?php\n$q = <<<SQL\nSELECT * FROM foo\nSQL;\necho $q;\n",
            ),
            (
                "php",
                "a.php",
                "<?php\n$q = <<<'SQL'\nSELECT * FROM foo\nSQL;\n",
            ),
        ];
        for (outer, path, src) in cases {
            let lang = languages::detect(Some(std::path::Path::new(path)), src).unwrap();
            assert_eq!(lang.name, *outer, "{path}");
            let spans = highlight(src, lang);
            let select = src.find("SELECT").unwrap();
            assert!(
                spans.iter().any(|s| s.language == "sql"
                    && s.start == select
                    && s.end == select + 6
                    && s.scope.name().starts_with("keyword")),
                "{outer} {src:?}: no SQL keyword span for SELECT, got {spans:?}"
            );
            // Nothing of the outer language survives inside the body.
            let body_end = src.rfind("SQL").unwrap();
            assert!(
                spans
                    .iter()
                    .filter(|s| s.start >= select && s.end <= body_end)
                    .all(|s| s.language == "sql"),
                "{outer} {src:?}: outer spans leaked into the heredoc body"
            );
            // Code after the terminator is still the outer language.
            if let Some(after) = src.find("puts").or_else(|| src.find("echo")) {
                assert!(
                    spans
                        .iter()
                        .any(|s| s.start == after && s.language == *outer),
                    "{outer} {src:?}: code after the heredoc lost its language"
                );
            }
        }
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
            spans.iter().any(|s| s.scope.name().starts_with("string")
                && s.start <= body_start
                && s.end >= body_start + 9),
            "expected the heredoc body to remain a string span, got {spans:?}"
        );
    }

    #[test]
    fn override_forces_language_regardless_of_filename() {
        let lang =
            detect_with_override(Some(std::path::Path::new("foo.txt")), "", Some("perl")).unwrap();
        assert_eq!(lang.name, "perl");
    }

    #[test]
    fn override_none_disables_detection() {
        assert!(
            detect_with_override(Some(std::path::Path::new("foo.pl")), "", Some("none")).is_none()
        );
    }

    #[test]
    fn override_unknown_name_falls_back_to_detection() {
        let lang =
            detect_with_override(Some(std::path::Path::new("foo.pl")), "", Some("boguslang"))
                .unwrap();
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
            let lang = detect(Some(std::path::Path::new(&path)), "")
                .expect("perl file should be detected");
            assert_eq!(lang.name, "perl");
        }
    }

    #[test]
    fn all_languages_query_compiles_and_highlights() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "c",
                "a.c",
                "#include <stdio.h>\nint main() { return 0; } // hi\n",
            ),
            (
                "cpp",
                "a.cpp",
                "#include <iostream>\nclass Foo { public: int x; }; // hi\n",
            ),
            ("rust", "a.rs", "fn main() { let x = 1; } // hi\n"),
            ("go", "a.go", "package main\nfunc main() { x := 1 } // hi\n"),
            (
                "python",
                "a.py",
                "def foo():\n    x = 1  # hi\n    return x\n",
            ),
            (
                "bash",
                "a.sh",
                "#!/bin/bash\nfoo() { echo hi; } # comment\n",
            ),
            ("json", "a.json", "{\"a\": 1, \"b\": \"c\"}\n"),
            ("yaml", "a.yaml", "a: 1\nb: \"c\" # hi\n"),
            ("toml", "a.toml", "a = 1\nb = \"c\" # hi\n"),
            ("html", "a.html", "<html><!-- hi --><body>x</body></html>\n"),
            ("css", "a.css", "/* hi */ .a { color: red; }\n"),
            ("sql", "a.sql", "SELECT * FROM foo WHERE x = 1; -- hi\n"),
            ("javascript", "a.js", "function foo() { return 1; } // hi\n"),
            (
                "typescript",
                "a.ts",
                "function foo(): number { return 1; } // hi\n",
            ),
            ("java", "a.java", "class Foo { void bar() {} } // hi\n"),
            ("ruby", "a.rb", "def foo\n  1 # hi\nend\n"),
            (
                "php",
                "a.php",
                "<?php\nfunction foo() { return 1; } // hi\n",
            ),
            ("csharp", "a.cs", "class Foo { void Bar() {} } // hi\n"),
            ("make", "Makefile", "all:\n\techo hi # comment\n"),
            (
                "fortran",
                "a.f90",
                "program hi\n  integer :: x = 1\nend program hi\n",
            ),
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
            (
                "elixir",
                "a.ex",
                "defmodule Foo do\n  def bar, do: 1 # hi\nend\n",
            ),
            ("elm", "a.elm", "foo x = x + 1 -- hi\n"),
            ("zig", "a.zig", "pub fn main() void { } // hi\n"),
            ("dart", "a.dart", "void main() { print('hi'); } // hi\n"),
            ("scss", "a.scss", "// hi\n.a { color: red; }\n"),
            (
                "proto",
                "a.proto",
                "// hi\nmessage Foo { string bar = 1; }\n",
            ),
            (
                "cmake",
                "CMakeLists.txt",
                "# hi\nadd_executable(foo bar.c)\n",
            ),
            ("nix", "a.nix", "# hi\n{ foo = 1; }\n"),
            ("vim", "a.vim", "\" hi\nlet g:foo = 1\n"),
            ("lua", "a.lua", "-- hi\nfunction foo() return 1 end\n"),
            ("swift", "a.swift", "func foo() -> Int { return 1 } // hi\n"),
            ("go", "a.go", "package main\n// hi\nfunc main() {}\n"),
            (
                "vcl",
                "default.vcl",
                "vcl 4.1;\nsub vcl_recv { # hi\n    return (pass);\n}\n",
            ),
        ];
        for (name, path, source) in cases {
            check(name, path, source, true);
        }
    }

    /// Every capture name every vendored query produces must, after
    /// `normalize_capture`, start with one of the top-level scope names
    /// Helix themes are written against -- otherwise no theme could ever
    /// style it, and a new query (or a new upstream revision of one) that
    /// brings a novel nvim-ism would silently render uncolored.
    #[test]
    fn all_captures_normalize_into_helix_scopes() {
        const HELIX_TOP_LEVEL: &[&str] = &[
            "attribute",
            "type",
            "constructor",
            "constant",
            "string",
            "comment",
            "variable",
            "label",
            "punctuation",
            "keyword",
            "operator",
            "function",
            "tag",
            "namespace",
            "special",
            "markup",
            "diff",
        ];
        let mut bad = Vec::new();
        for name in names() {
            let lang = find_by_name(name).unwrap();
            let query = tree_sitter::Query::new(&(lang.language)(), lang.highlights_query)
                .unwrap_or_else(|e| panic!("{name}: query failed to compile: {e}"));
            for raw in query.capture_names() {
                let Some(scope) = normalize_capture(raw) else {
                    continue;
                };
                let head = scope.split('.').next().unwrap();
                if !HELIX_TOP_LEVEL.contains(&head) {
                    bad.push(format!("{name}: @{raw} -> {scope}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "captures outside Helix's scope vocabulary:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn normalize_capture_renames_nvim_conventions() {
        let n = |s: &str| normalize_capture(s);
        assert_eq!(n("number").as_deref(), Some("constant.numeric"));
        assert_eq!(n("float").as_deref(), Some("constant.numeric.float"));
        assert_eq!(
            n("conditional").as_deref(),
            Some("keyword.control.conditional")
        );
        assert_eq!(
            n("keyword.import").as_deref(),
            Some("keyword.control.import")
        );
        assert_eq!(n("property").as_deref(), Some("variable.other.member"));
        assert_eq!(n("text.title").as_deref(), Some("markup.heading"));
        assert_eq!(n("string.elm").as_deref(), Some("string"));
        assert_eq!(n("function.elm").as_deref(), Some("function"));
        // Already-Helix names pass through untouched, tail included.
        assert_eq!(
            n("keyword.control.import").as_deref(),
            Some("keyword.control.import")
        );
        assert_eq!(n("variable.builtin").as_deref(), Some("variable.builtin"));
        assert_eq!(
            n("punctuation.section.braces").as_deref(),
            Some("punctuation.section.braces")
        );
        // Head-only renames keep their tail.
        assert_eq!(
            n("property.foo").as_deref(),
            Some("variable.other.member.foo")
        );
        // Dropped outright.
        assert_eq!(n("_name"), None);
        assert_eq!(n("spell"), None);
        assert_eq!(n("source.glsl"), None);
        assert_eq!(n("text.warning"), None);
    }

    #[test]
    fn scopes_intern_to_stable_ids() {
        let a = Scope::intern("keyword.control");
        let b = Scope::intern("keyword.control");
        let c = Scope::intern("keyword");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.name(), "keyword.control");
        assert_eq!(c.name(), "keyword");
    }
}
