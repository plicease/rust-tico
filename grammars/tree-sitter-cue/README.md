# tree-sitter-cue (vendored)

A tree-sitter grammar for [CUE](https://cuelang.org/), the configuration
language. Copied unmodified from
[eonpatapon/tree-sitter-cue](https://github.com/eonpatapon/tree-sitter-cue)
at commit `dd7b90e` (2026-04-14); MIT, copyright (c) 2018-2021
Jean-Philippe Braun and Amaan Qureshi, see `LICENSE`. It's the grammar
Helix and nvim-treesitter use.

It's vendored here rather than pulled from crates.io because upstream's
Rust bindings (both the `tree-sitter-cue` 0.0.1 crate from 2023 and the
git HEAD) still pin `tree-sitter ~0.20`, which can't unify with the 0.26
the rest of tico uses. The generated parser itself is current (ABI 15).
`build.rs` compiles `src/parser.c` and `src/scanner.c` the same way it
does for VCL and Template Toolkit, and `src/syntax/languages.rs`
declares the `tree_sitter_cue` entry point.

To update: check out a newer upstream commit, copy `LICENSE`,
`grammar.js` and `src/` over this directory, and update the commit
reference above. Upstream commits its generated `src/`, so no
tree-sitter CLI run is needed.
