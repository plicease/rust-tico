//! Command-line argument parsing, matching GNU nano 8.7.1's option set
//! (short/long flags captured verbatim from the installed `nano --help`).

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "tico", version, about = "Yet Another Text Editor (a nano-compatible TUI editor)")]
pub struct Cli {
    #[arg(short = 'A', long = "smarthome")]
    pub smarthome: bool,
    #[arg(short = 'B', long = "backup")]
    pub backup: bool,
    #[arg(short = 'C', long = "backupdir", value_name = "dir")]
    pub backupdir: Option<String>,
    #[arg(short = 'D', long = "boldtext")]
    pub boldtext: bool,
    #[arg(short = 'E', long = "tabstospaces")]
    pub tabstospaces: bool,
    #[arg(short = 'F', long = "multibuffer")]
    pub multibuffer: bool,
    #[arg(short = 'G', long = "locking")]
    pub locking: bool,
    #[arg(short = 'H', long = "historylog")]
    pub historylog: bool,
    #[arg(short = 'I', long = "ignorercfiles")]
    pub ignorercfiles: bool,
    #[arg(short = 'J', long = "guidestripe", value_name = "number")]
    pub guidestripe: Option<u32>,
    #[arg(short = 'K', long = "rawsequences")]
    pub rawsequences: bool,
    #[arg(short = 'L', long = "nonewlines")]
    pub nonewlines: bool,
    #[arg(short = 'M', long = "trimblanks")]
    pub trimblanks: bool,
    #[arg(short = 'N', long = "noconvert")]
    pub noconvert: bool,
    #[arg(short = 'O', long = "bookstyle")]
    pub bookstyle: bool,
    #[arg(short = 'P', long = "positionlog")]
    pub positionlog: bool,
    #[arg(short = 'Q', long = "quotestr", value_name = "regex")]
    pub quotestr: Option<String>,
    #[arg(short = 'R', long = "restricted")]
    pub restricted: bool,
    #[arg(short = 'S', long = "softwrap")]
    pub softwrap: bool,
    #[arg(short = 'T', long = "tabsize", value_name = "number")]
    pub tabsize: Option<u32>,
    #[arg(short = 'U', long = "quickblank")]
    pub quickblank: bool,
    #[arg(short = 'W', long = "wordbounds")]
    pub wordbounds: bool,
    #[arg(short = 'X', long = "wordchars", value_name = "string")]
    pub wordchars: Option<String>,
    #[arg(short = 'Y', long = "syntax", value_name = "name")]
    pub syntax: Option<String>,
    #[arg(short = 'Z', long = "zap")]
    pub zap: bool,
    #[arg(short = 'a', long = "atblanks")]
    pub atblanks: bool,
    #[arg(short = 'b', long = "breaklonglines")]
    pub breaklonglines: bool,
    #[arg(short = 'c', long = "constantshow")]
    pub constantshow: bool,
    #[arg(short = 'd', long = "rebinddelete")]
    pub rebinddelete: bool,
    #[arg(short = 'e', long = "emptyline")]
    pub emptyline: bool,
    #[arg(short = 'f', long = "rcfile", value_name = "file")]
    pub rcfile: Option<String>,
    #[arg(short = 'g', long = "showcursor")]
    pub showcursor: bool,
    #[arg(short = 'i', long = "autoindent")]
    pub autoindent: bool,
    #[arg(short = 'j', long = "jumpyscrolling")]
    pub jumpyscrolling: bool,
    #[arg(short = 'k', long = "cutfromcursor")]
    pub cutfromcursor: bool,
    #[arg(short = 'l', long = "linenumbers")]
    pub linenumbers: bool,
    #[arg(short = 'm', long = "mouse")]
    pub mouse: bool,
    #[arg(short = 'n', long = "noread")]
    pub noread: bool,
    #[arg(short = 'o', long = "operatingdir", value_name = "dir")]
    pub operatingdir: Option<String>,
    #[arg(short = 'p', long = "preserve")]
    pub preserve: bool,
    #[arg(short = 'q', long = "indicator")]
    pub indicator: bool,
    #[arg(short = 'r', long = "fill", value_name = "number")]
    pub fill: Option<i32>,
    #[arg(short = 's', long = "speller", value_name = "program")]
    pub speller: Option<String>,
    #[arg(short = 't', long = "saveonexit")]
    pub saveonexit: bool,
    #[arg(short = 'u', long = "unix")]
    pub unix: bool,
    #[arg(short = 'v', long = "view")]
    pub view: bool,
    #[arg(short = 'w', long = "nowrap")]
    pub nowrap: bool,
    #[arg(short = 'x', long = "nohelp")]
    pub nohelp: bool,
    #[arg(short = 'y', long = "afterends")]
    pub afterends: bool,
    #[arg(short = 'z', long = "listsyntaxes")]
    pub listsyntaxes: bool,
    #[arg(short = '@', long = "colonparsing")]
    pub colonparsing: bool,
    #[arg(short = '%', long = "stateflags")]
    pub stateflags: bool,
    #[arg(short = '_', long = "minibar")]
    pub minibar: bool,
    #[arg(short = '0', long = "zero")]
    pub zero: bool,
    #[arg(short = '/', long = "modernbindings")]
    pub modernbindings: bool,

    /// Files to edit, optionally preceded by +LINE[,COLUMN]. A name of `-`
    /// reads from standard input.
    #[arg(trailing_var_arg = true)]
    pub files: Vec<String>,
}

impl Cli {
    /// Apply the CLI flags on top of already-loaded config-file options
    /// (CLI overrides nanorc/ticorc, per nanorc(5): "command-line options
    /// override nanorc settings").
    pub fn apply(&self, options: &mut crate::options::Options) {
        macro_rules! flag {
            ($field:ident) => {
                if self.$field {
                    options.$field = true;
                }
            };
        }
        flag!(smarthome);
        flag!(backup);
        flag!(boldtext);
        flag!(tabstospaces);
        flag!(multibuffer);
        flag!(locking);
        flag!(historylog);
        flag!(rawsequences);
        flag!(nonewlines);
        flag!(trimblanks);
        flag!(noconvert);
        flag!(bookstyle);
        flag!(positionlog);
        flag!(restricted);
        flag!(softwrap);
        flag!(quickblank);
        flag!(wordbounds);
        flag!(zap);
        flag!(atblanks);
        flag!(breaklonglines);
        flag!(constantshow);
        flag!(rebinddelete);
        flag!(emptyline);
        flag!(showcursor);
        flag!(autoindent);
        flag!(jumpyscrolling);
        flag!(cutfromcursor);
        flag!(linenumbers);
        flag!(mouse);
        flag!(preserve);
        flag!(indicator);
        flag!(saveonexit);
        flag!(unix);
        flag!(view);
        flag!(nowrap);
        flag!(nohelp);
        flag!(afterends);
        flag!(colonparsing);
        flag!(minibar);
        flag!(zero);
        flag!(modernbindings);

        if self.stateflags {
            options.stateflags = true;
        }
        if let Some(v) = &self.backupdir {
            options.backupdir = Some(v.clone());
        }
        if let Some(v) = self.guidestripe {
            options.guidestripe = Some(v);
        }
        if let Some(v) = &self.quotestr {
            options.quotestr = v.clone();
        }
        if let Some(v) = self.tabsize {
            options.tabsize = v;
        }
        if let Some(v) = &self.wordchars {
            options.wordchars = Some(v.clone());
        }
        if let Some(v) = &self.syntax {
            options.syntax_name = Some(v.clone());
        }
        if let Some(v) = &self.operatingdir {
            options.operatingdir = Some(v.clone());
        }
        if let Some(v) = self.fill {
            options.fill = v;
        }
        if let Some(v) = &self.speller {
            options.speller = Some(v.clone());
        }
        options.ignorercfiles = self.ignorercfiles;
        options.rcfile = self.rcfile.clone();
    }
}

/// Parsed positional argument: an optional `+LINE[,COLUMN]` followed by a
/// filename.
pub struct FileArg {
    pub path: String,
    pub line: Option<i64>,
    pub column: Option<i64>,
}

/// Interpret `cli.files` as nano does: a leading `+LINE[,COLUMN]` token
/// attaches to the filename that follows it.
pub fn parse_file_args(files: &[String]) -> Vec<FileArg> {
    let mut out = Vec::new();
    let mut pending: Option<(Option<i64>, Option<i64>)> = None;
    for f in files {
        if let Some(spec) = f.strip_prefix('+') {
            let (line_s, col_s) = match spec.split_once(',') {
                Some((l, c)) => (l, Some(c)),
                None => (spec, None),
            };
            let line = line_s.parse::<i64>().ok();
            let col = col_s.and_then(|c| c.parse::<i64>().ok());
            pending = Some((line, col));
            continue;
        }
        let (line, column) = pending.take().unwrap_or((None, None));
        out.push(FileArg { path: f.clone(), line, column });
    }
    out
}
