# tree-sitter-template-toolkit (vendored)

A tree-sitter grammar for Template Toolkit 2 (TT2), the Perl templating
language. Copied unmodified from
[RuvimSypa/tree-sitter-template-toolkit](https://github.com/RuvimSypa/tree-sitter-template-toolkit)
at commit `14026c1` (2026-08-15); MIT, copyright (c) 2026 Ruvym Sypa, see
`LICENSE`.

It's vendored here rather than pulled from crates.io because upstream
ships no Rust bindings or crate — just the generated `src/` and the
`grammar.js` it came from. `build.rs` compiles `src/parser.c` and
`src/scanner.c` the same way it does for VCL, and `src/syntax/languages.rs`
declares the `tree_sitter_template_toolkit` entry point.

tico uses only the directive side of the grammar: `[% ... %]` tags are
highlighted, and the `content` between them is left plain whatever
language it's in (HTML, VCL, ...). Upstream's editor integration injects
HTML into `content`; tico deliberately doesn't, since a template is as
likely to be generating VCL or config as markup.

To update: check out a newer upstream commit, copy `LICENSE`,
`grammar.js`, `tree-sitter.json` and `src/` over this directory, and
update the commit reference above. Upstream commits its generated
`src/`, so no tree-sitter CLI run is needed.
