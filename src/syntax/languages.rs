//! The language registry and detection cascade: filename extension (or
//! exact filename, for things like `Makefile`) -> shebang line -> modeline
//! (vim- or Emacs-style), matching the priority order other editors use.
//! All on by default; see `crate::options::Options::syntax_highlighting`
//! for the master on/off toggle.

use std::path::Path;

pub struct LanguageDef {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    pub shebangs: &'static [&'static str],
    /// Extra names recognized in a modeline besides `name` itself.
    pub modeline_aliases: &'static [&'static str],
    pub language: fn() -> tree_sitter::Language,
    pub highlights_query: &'static str,
}

macro_rules! lang_fn {
    ($fn_name:ident, $krate:ident) => {
        fn $fn_name() -> tree_sitter::Language {
            $krate::LANGUAGE.into()
        }
    };
    ($fn_name:ident, $krate:ident, $konst:ident) => {
        fn $fn_name() -> tree_sitter::Language {
            $krate::$konst.into()
        }
    };
}

/// A handful of grammar crates still expose the older `fn language() ->
/// Language` API instead of the newer `const LANGUAGE: LanguageFn`.
macro_rules! lang_fn_old_api {
    ($fn_name:ident, $krate:ident) => {
        fn $fn_name() -> tree_sitter::Language {
            $krate::language()
        }
    };
}

lang_fn!(lang_perl, tree_sitter_perl);
lang_fn!(lang_c, tree_sitter_c);
lang_fn!(lang_cpp, tree_sitter_cpp);
lang_fn!(lang_html, tree_sitter_html);
lang_fn!(lang_css, tree_sitter_css);
lang_fn!(lang_sql, tree_sitter_sequel);
lang_fn!(lang_python, tree_sitter_python);
lang_fn!(lang_rust, tree_sitter_rust);
lang_fn!(lang_bash, tree_sitter_bash);
lang_fn!(lang_json, tree_sitter_json);
lang_fn!(lang_yaml, tree_sitter_yaml);
lang_fn!(lang_toml, tree_sitter_toml_ng);
lang_fn!(lang_make, tree_sitter_make);
lang_fn!(lang_fortran, tree_sitter_fortran);
lang_fn!(lang_go, tree_sitter_go);
lang_fn!(lang_javascript, tree_sitter_javascript);
lang_fn!(lang_typescript, tree_sitter_typescript, LANGUAGE_TYPESCRIPT);
lang_fn!(lang_java, tree_sitter_java);
lang_fn!(lang_ruby, tree_sitter_ruby);
lang_fn!(lang_php, tree_sitter_php, LANGUAGE_PHP);
lang_fn!(lang_csharp, tree_sitter_c_sharp);
lang_fn!(lang_swift, tree_sitter_swift);
lang_fn!(lang_haskell, tree_sitter_haskell);
lang_fn!(lang_scala, tree_sitter_scala);
lang_fn!(lang_objc, tree_sitter_objc);
lang_fn!(lang_r, tree_sitter_r);
lang_fn!(lang_julia, tree_sitter_julia);
lang_fn!(lang_xml, tree_sitter_xml, LANGUAGE_XML);
lang_fn!(lang_diff, tree_sitter_diff);
lang_fn!(lang_ini, tree_sitter_ini);
lang_fn!(lang_elixir, tree_sitter_elixir);
lang_fn!(lang_elm, tree_sitter_elm);
lang_fn!(lang_zig, tree_sitter_zig);
lang_fn!(lang_dart, tree_sitter_dart);
lang_fn_old_api!(lang_scss, tree_sitter_scss);
lang_fn!(lang_proto, tree_sitter_proto);
lang_fn!(lang_cmake, tree_sitter_cmake);
lang_fn!(lang_nix, tree_sitter_nix);
lang_fn_old_api!(lang_vim, tree_sitter_vim);
lang_fn!(lang_lua, tree_sitter_lua);
lang_fn!(lang_markdown, tree_sitter_md);

const LANGUAGES: &[LanguageDef] = &[
    LanguageDef {
        name: "perl",
        extensions: &["pl", "pm", "t", "xs"],
        filenames: &[],
        shebangs: &["perl", "perl5"],
        modeline_aliases: &["cperl"],
        language: lang_perl,
        highlights_query: include_str!("queries/perl.scm"),
    },
    LanguageDef {
        name: "c",
        extensions: &["c", "h"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_c,
        highlights_query: include_str!("queries/c.scm"),
    },
    LanguageDef {
        name: "cpp",
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "c++", "h++"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["c++"],
        language: lang_cpp,
        highlights_query: include_str!("queries/cpp.scm"),
    },
    LanguageDef {
        name: "html",
        extensions: &["html", "htm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_html,
        highlights_query: include_str!("queries/html.scm"),
    },
    LanguageDef {
        name: "css",
        extensions: &["css"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_css,
        highlights_query: include_str!("queries/css.scm"),
    },
    LanguageDef {
        name: "sql",
        extensions: &["sql"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_sql,
        highlights_query: include_str!("queries/sequel.scm"),
    },
    LanguageDef {
        name: "python",
        extensions: &["py", "pyw", "pyi"],
        filenames: &[],
        shebangs: &["python", "python2", "python3"],
        modeline_aliases: &["py"],
        language: lang_python,
        highlights_query: include_str!("queries/python.scm"),
    },
    LanguageDef {
        name: "rust",
        extensions: &["rs"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_rust,
        highlights_query: include_str!("queries/rust.scm"),
    },
    LanguageDef {
        name: "bash",
        extensions: &["sh", "bash", "zsh", "ksh"],
        filenames: &[".bashrc", ".bash_profile", ".zshrc", ".profile"],
        shebangs: &["bash", "sh", "zsh", "ksh", "dash"],
        modeline_aliases: &["sh", "zsh"],
        language: lang_bash,
        highlights_query: include_str!("queries/bash.scm"),
    },
    LanguageDef {
        name: "json",
        extensions: &["json"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_json,
        highlights_query: include_str!("queries/json.scm"),
    },
    LanguageDef {
        name: "yaml",
        extensions: &["yaml", "yml"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_yaml,
        highlights_query: include_str!("queries/yaml.scm"),
    },
    LanguageDef {
        name: "toml",
        extensions: &["toml"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_toml,
        highlights_query: include_str!("queries/toml.scm"),
    },
    LanguageDef {
        name: "make",
        extensions: &["mk", "mak"],
        filenames: &["Makefile", "makefile", "GNUmakefile"],
        shebangs: &["make"],
        modeline_aliases: &["makefile"],
        language: lang_make,
        highlights_query: include_str!("queries/make.scm"),
    },
    LanguageDef {
        name: "fortran",
        extensions: &["f", "for", "f90", "f95", "f03", "f08"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["f90"],
        language: lang_fortran,
        highlights_query: include_str!("queries/fortran.scm"),
    },
    LanguageDef {
        name: "go",
        extensions: &["go"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_go,
        highlights_query: include_str!("queries/go.scm"),
    },
    LanguageDef {
        name: "javascript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        shebangs: &["node", "nodejs"],
        modeline_aliases: &["js"],
        language: lang_javascript,
        highlights_query: include_str!("queries/javascript.scm"),
    },
    LanguageDef {
        name: "typescript",
        extensions: &["ts", "mts", "cts"],
        filenames: &[],
        shebangs: &["ts-node", "deno"],
        modeline_aliases: &["ts"],
        language: lang_typescript,
        highlights_query: include_str!("queries/typescript.scm"),
    },
    LanguageDef {
        name: "java",
        extensions: &["java"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_java,
        highlights_query: include_str!("queries/java.scm"),
    },
    LanguageDef {
        name: "ruby",
        extensions: &["rb", "rake", "gemspec"],
        filenames: &["Rakefile", "Gemfile"],
        shebangs: &["ruby"],
        modeline_aliases: &["rb"],
        language: lang_ruby,
        highlights_query: include_str!("queries/ruby.scm"),
    },
    LanguageDef {
        name: "php",
        extensions: &["php", "php3", "php4", "php5", "phtml"],
        filenames: &[],
        shebangs: &["php"],
        modeline_aliases: &[],
        language: lang_php,
        highlights_query: include_str!("queries/php.scm"),
    },
    LanguageDef {
        name: "csharp",
        extensions: &["cs"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["cs", "c#"],
        language: lang_csharp,
        highlights_query: include_str!("queries/c-sharp.scm"),
    },
    LanguageDef {
        name: "swift",
        extensions: &["swift"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_swift,
        highlights_query: include_str!("queries/swift.scm"),
    },
    LanguageDef {
        name: "haskell",
        extensions: &["hs", "lhs"],
        filenames: &[],
        shebangs: &["runghc", "runhaskell"],
        modeline_aliases: &[],
        language: lang_haskell,
        highlights_query: include_str!("queries/haskell.scm"),
    },
    LanguageDef {
        name: "scala",
        extensions: &["scala", "sc"],
        filenames: &[],
        shebangs: &["scala"],
        modeline_aliases: &[],
        language: lang_scala,
        highlights_query: include_str!("queries/scala.scm"),
    },
    LanguageDef {
        name: "objc",
        extensions: &["m", "mm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["objc", "objective-c"],
        language: lang_objc,
        highlights_query: include_str!("queries/objc.scm"),
    },
    LanguageDef {
        name: "r",
        extensions: &["r", "R"],
        filenames: &[],
        shebangs: &["Rscript"],
        modeline_aliases: &[],
        language: lang_r,
        highlights_query: include_str!("queries/r.scm"),
    },
    LanguageDef {
        name: "julia",
        extensions: &["jl"],
        filenames: &[],
        shebangs: &["julia"],
        modeline_aliases: &[],
        language: lang_julia,
        highlights_query: include_str!("queries/julia.scm"),
    },
    LanguageDef {
        name: "xml",
        extensions: &["xml", "xsd", "xsl", "svg", "plist"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_xml,
        highlights_query: include_str!("queries/xml.scm"),
    },
    LanguageDef {
        name: "diff",
        extensions: &["diff", "patch"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_diff,
        highlights_query: include_str!("queries/diff.scm"),
    },
    LanguageDef {
        name: "ini",
        extensions: &["ini", "cfg"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["cfg", "dosini"],
        language: lang_ini,
        highlights_query: include_str!("queries/ini.scm"),
    },
    LanguageDef {
        name: "elixir",
        extensions: &["ex", "exs"],
        filenames: &[],
        shebangs: &["elixir"],
        modeline_aliases: &[],
        language: lang_elixir,
        highlights_query: include_str!("queries/elixir.scm"),
    },
    LanguageDef {
        name: "elm",
        extensions: &["elm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_elm,
        highlights_query: include_str!("queries/elm.scm"),
    },
    LanguageDef {
        name: "zig",
        extensions: &["zig"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_zig,
        highlights_query: include_str!("queries/zig.scm"),
    },
    LanguageDef {
        name: "dart",
        extensions: &["dart"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_dart,
        highlights_query: include_str!("queries/dart.scm"),
    },
    LanguageDef {
        name: "scss",
        extensions: &["scss"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_scss,
        highlights_query: include_str!("queries/scss.scm"),
    },
    LanguageDef {
        name: "proto",
        extensions: &["proto"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["protobuf"],
        language: lang_proto,
        highlights_query: include_str!("queries/proto.scm"),
    },
    LanguageDef {
        name: "cmake",
        extensions: &["cmake"],
        filenames: &["CMakeLists.txt"],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_cmake,
        highlights_query: include_str!("queries/cmake.scm"),
    },
    LanguageDef {
        name: "nix",
        extensions: &["nix"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_nix,
        highlights_query: include_str!("queries/nix.scm"),
    },
    LanguageDef {
        name: "vim",
        extensions: &["vim"],
        filenames: &[".vimrc", "vimrc"],
        shebangs: &[],
        modeline_aliases: &["viml"],
        language: lang_vim,
        highlights_query: include_str!("queries/vim.scm"),
    },
    LanguageDef {
        name: "lua",
        extensions: &["lua"],
        filenames: &[],
        shebangs: &["lua"],
        modeline_aliases: &[],
        language: lang_lua,
        highlights_query: include_str!("queries/lua.scm"),
    },
    LanguageDef {
        name: "markdown",
        extensions: &["md", "markdown"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["md"],
        language: lang_markdown,
        highlights_query: include_str!("queries/markdown.scm"),
    },
];

/// Detect a buffer's language: by filename extension or exact filename
/// first, then by shebang line, then by a vim- or Emacs-style modeline
/// found in the first or last few lines — matching how other editors
/// layer these signals, most-specific (and cheapest to check) first.
pub fn detect(path: Option<&Path>, text: &str) -> Option<&'static LanguageDef> {
    if let Some(path) = path {
        if let Some(lang) = detect_by_filename(path) {
            return Some(lang);
        }
    }
    let first_line = text.lines().next().unwrap_or("");
    if let Some(lang) = detect_by_shebang(first_line) {
        return Some(lang);
    }
    if let Some(lang) = detect_by_modeline(text) {
        return Some(lang);
    }
    None
}

fn detect_by_filename(path: &Path) -> Option<&'static LanguageDef> {
    let filename = path.file_name().and_then(|f| f.to_str());
    if let Some(filename) = filename {
        for lang in LANGUAGES {
            if lang.filenames.iter().any(|f| *f == filename) {
                return Some(lang);
            }
        }
    }
    let ext = path.extension().and_then(|e| e.to_str())?;
    for lang in LANGUAGES {
        if lang.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            return Some(lang);
        }
    }
    None
}

/// Parse a shebang line, following `env` indirection (e.g.
/// `#!/usr/bin/env perl` -> `perl`), and match it against each language's
/// known interpreter names.
fn detect_by_shebang(first_line: &str) -> Option<&'static LanguageDef> {
    let rest = first_line.strip_prefix("#!")?;
    let mut parts = rest.split_whitespace();
    let mut interpreter = parts.next()?;
    let interpreter_base = interpreter.rsplit('/').next().unwrap_or(interpreter);
    if interpreter_base == "env" {
        interpreter = parts.next()?;
    } else {
        interpreter = interpreter_base;
    }
    // Strip a trailing version number, e.g. "python3" already handled by
    // exact shebang lists below, but "perl5.34" or "python3.11" isn't.
    let interpreter_trimmed = interpreter.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    for lang in LANGUAGES {
        if lang.shebangs.iter().any(|s| *s == interpreter || *s == interpreter_trimmed) {
            return Some(lang);
        }
    }
    None
}

/// Scan the first and last few lines for a vim modeline (`vim: set ft=X`,
/// `vim: syntax=X`, `vim: ft=X`) or an Emacs one (`-*- mode: X -*-` or the
/// shorthand `-*- X -*-`), matching each language's canonical name or one
/// of its aliases.
fn detect_by_modeline(text: &str) -> Option<&'static LanguageDef> {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let head = lines.iter().take(5);
    let tail = lines.iter().rev().take(5);
    for line in head.chain(tail) {
        if let Some(name) = parse_vim_modeline(line).or_else(|| parse_emacs_modeline(line)) {
            if let Some(lang) = find_by_name(&name) {
                return Some(lang);
            }
        }
    }
    let _ = n;
    None
}

fn parse_vim_modeline(line: &str) -> Option<String> {
    // Matches "vim: ft=perl", "vim: set ft=perl:", "vim: syntax=perl",
    // "ex: syntax=perl", each optionally followed by more ":"-separated
    // options and/or a trailing ":".
    for marker in ["vim:", "vi:", "ex:"] {
        if let Some(pos) = line.find(marker) {
            let rest = &line[pos + marker.len()..];
            let rest = rest.strip_prefix(" set ").or_else(|| rest.strip_prefix("set ")).unwrap_or(rest);
            for field in rest.split(|c: char| c == ':' || c.is_whitespace()) {
                if let Some(v) = field.strip_prefix("ft=").or_else(|| field.strip_prefix("filetype=")) {
                    return Some(v.to_string());
                }
                if let Some(v) = field.strip_prefix("syntax=") {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn parse_emacs_modeline(line: &str) -> Option<String> {
    let start = line.find("-*-")?;
    let rest = &line[start + 3..];
    let end = rest.find("-*-")?;
    let inner = rest[..end].trim();
    if let Some(pos) = inner.find("mode:").or_else(|| inner.find("Mode:")) {
        let after = &inner[pos + "mode:".len()..];
        let name = after.split(';').next().unwrap_or(after).trim();
        return Some(name.to_string());
    }
    // Shorthand form: "-*- Perl -*-" (just the mode name, no "mode:" key).
    if !inner.is_empty() && !inner.contains(':') {
        return Some(inner.trim().to_string());
    }
    None
}

/// Look up a language by its canonical `name` or one of its
/// `modeline_aliases`, case-insensitively. Used both for modeline detection
/// and for matching a heredoc terminator (e.g. `<<SQL`) to a language.
pub(crate) fn find_by_name(name: &str) -> Option<&'static LanguageDef> {
    let name = name.to_ascii_lowercase();
    LANGUAGES.iter().find(|l| l.name == name || l.modeline_aliases.iter().any(|a| *a == name))
}
