//! Shared implementation of nanorc's `set`/`unset NAME [ARG]` vocabulary,
//! used both by the nanorc parser and by `~/.ticorc`'s `[main]` section
//! (which uses the exact same option names, just without the `set` keyword).

use crate::options::{self, Options};

/// Apply one `set`/`unset` directive. `enable` is `true` for `set`, `false`
/// for `unset`. `arg` is the raw remainder of the line after the option
/// name (already stripped of surrounding quotes, if any), for options that
/// take a value. Returns `Err` with a message for an unknown option name.
pub fn apply(options: &mut Options, name: &str, arg: Option<&str>, enable: bool) -> Result<(), String> {
    macro_rules! flag {
        ($field:ident) => {{
            options.$field = enable;
            return Ok(());
        }};
    }
    match name {
        "afterends" => flag!(afterends),
        "allow_insecure_backup" => flag!(allow_insecure_backup),
        "atblanks" => flag!(atblanks),
        "autoindent" => flag!(autoindent),
        "backup" => flag!(backup),
        "boldtext" => flag!(boldtext),
        "bookstyle" => flag!(bookstyle),
        "breaklonglines" => flag!(breaklonglines),
        "casesensitive" => flag!(casesensitive),
        "colonparsing" => flag!(colonparsing),
        "constantshow" => flag!(constantshow),
        "cutfromcursor" => flag!(cutfromcursor),
        "emptyline" => flag!(emptyline),
        "historylog" => flag!(historylog),
        "indicator" => flag!(indicator),
        "jumpyscrolling" => flag!(jumpyscrolling),
        "linenumbers" => flag!(linenumbers),
        "locking" => flag!(locking),
        "magic" => flag!(magic),
        "minibar" => flag!(minibar),
        "mouse" => flag!(mouse),
        "multibuffer" => flag!(multibuffer),
        "noconvert" => flag!(noconvert),
        "nohelp" => flag!(nohelp),
        "nonewlines" => flag!(nonewlines),
        "nowrap" => flag!(nowrap),
        "positionlog" => flag!(positionlog),
        "preserve" => flag!(preserve),
        "quickblank" => flag!(quickblank),
        "rawsequences" => flag!(rawsequences),
        "rebinddelete" => flag!(rebinddelete),
        "regexp" => flag!(regexp),
        "saveonexit" => flag!(saveonexit),
        "showcursor" => flag!(showcursor),
        "smarthome" => flag!(smarthome),
        "softwrap" => flag!(softwrap),
        "stateflags" => flag!(stateflags),
        "tabstospaces" => flag!(tabstospaces),
        "trimblanks" => flag!(trimblanks),
        "unix" => flag!(unix),
        "wordbounds" => flag!(wordbounds),
        "zap" => flag!(zap),
        "zero" => flag!(zero),

        // tico-only extension: not a nano option, but harmless to accept
        // under the same `set`/`unset` vocabulary in ~/.ticorc's [main].
        "syntax_highlighting" => flag!(syntax_highlighting),

        "backupdir" => {
            options.backupdir = arg.map(|s| s.to_string());
            Ok(())
        }
        "brackets" => {
            if let Some(v) = arg {
                options.brackets = v.to_string();
            }
            Ok(())
        }
        "fill" => {
            if let Some(v) = arg.and_then(|s| s.parse::<i32>().ok()) {
                options.fill = v;
            }
            Ok(())
        }
        "guidestripe" => {
            options.guidestripe = arg.and_then(|s| s.parse::<u32>().ok());
            Ok(())
        }
        "matchbrackets" => {
            if let Some(v) = arg {
                options.matchbrackets = v.to_string();
            }
            Ok(())
        }
        "operatingdir" => {
            options.operatingdir = arg.map(|s| s.to_string());
            Ok(())
        }
        "punct" => {
            if let Some(v) = arg {
                options.punct = v.to_string();
            }
            Ok(())
        }
        "quotestr" => {
            if let Some(v) = arg {
                options.quotestr = v.to_string();
            }
            Ok(())
        }
        "speller" => {
            options.speller = arg.map(|s| s.to_string());
            Ok(())
        }
        "tabsize" => {
            if let Some(v) = arg.and_then(|s| s.parse::<u32>().ok()) {
                if v > 0 {
                    options.tabsize = v;
                }
            }
            Ok(())
        }
        "whitespace" => {
            if let Some(v) = arg {
                let mut chars = v.chars();
                if let (Some(a), Some(b)) = (chars.next(), chars.next()) {
                    options.whitespace = (a, b);
                }
            }
            Ok(())
        }
        "wordchars" => {
            options.wordchars = arg.map(|s| s.to_string());
            Ok(())
        }

        "errorcolor" => set_color(&mut options.errorcolor, arg),
        "functioncolor" => set_color(&mut options.functioncolor, arg),
        "keycolor" => set_color(&mut options.keycolor, arg),
        "minicolor" => set_color(&mut options.minicolor, arg),
        "numbercolor" => set_color(&mut options.numbercolor, arg),
        "promptcolor" => set_color(&mut options.promptcolor, arg),
        "scrollercolor" => set_color(&mut options.scrollercolor, arg),
        "selectedcolor" => set_color(&mut options.selectedcolor, arg),
        "spotlightcolor" => set_color(&mut options.spotlightcolor, arg),
        "statuscolor" => set_color(&mut options.statuscolor, arg),
        "stripecolor" => set_color(&mut options.stripecolor, arg),
        "titlecolor" => set_color(&mut options.titlecolor, arg),

        _ => Err(format!("unknown option: {name}")),
    }
}

fn set_color(field: &mut options::ColorPair, arg: Option<&str>) -> Result<(), String> {
    if let Some(v) = arg {
        if let Some(cp) = options::parse_color_pair(v) {
            *field = cp;
            return Ok(());
        }
        return Err(format!("invalid color spec: {v}"));
    }
    Ok(())
}
