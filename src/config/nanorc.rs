//! Parser for nano's `~/.nanorc` format (see `nanorc(5)`). Applies every
//! `set`/`unset`/`bind`/`unbind` directive; entirely skips `syntax` blocks
//! (and their `color`/`icolor`/`header`/`magic`/`formatter`/`linter`/
//! `comment`/`tabgives` bodies) as well as top-level `include` and
//! `extendsyntax` lines, since tico does its own syntax highlighting.

use super::settings;
use crate::keymap::{Action, Binding, Key, KeyMap, Menu};
use crate::options::Options;

/// Split off the first whitespace-delimited token, returning (token, rest).
fn split_first(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

/// Extract a possibly-quoted argument from the start of `s`: if `s` starts
/// with `"`, the argument runs from just after that quote to the *last*
/// `"` on the line (nano's documented rule), and the remainder is whatever
/// follows that last quote. Otherwise, the argument is just the first token.
fn take_arg(s: &str) -> (Option<String>, &str) {
    let s = s.trim_start();
    if s.is_empty() {
        return (None, "");
    }
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(last_q) = rest.rfind('"') {
            let arg = &rest[..last_q];
            let remainder = rest[last_q + 1..].trim_start();
            return (Some(arg.to_string()), remainder);
        }
        // Unterminated quote: treat the rest of the line as the argument.
        return (Some(rest.to_string()), "");
    }
    let (tok, rest) = split_first(s);
    (Some(tok.to_string()), rest)
}

const SYNTAX_BODY_COMMANDS: &[&str] = &[
    "color", "icolor", "header", "magic", "formatter", "linter", "comment", "tabgives",
];

pub fn parse(text: &str, options: &mut Options, keymap: &mut KeyMap, warnings: &mut Vec<String>) {
    let mut in_syntax_block = false;
    for (lineno, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (cmd, rest) = split_first(line);
        let cmd_lower = cmd.to_ascii_lowercase();

        if cmd_lower == "syntax" {
            in_syntax_block = true;
            continue;
        }
        if cmd_lower == "include" || cmd_lower == "extendsyntax" {
            // Syntax-highlighting-only directives: ignored entirely.
            continue;
        }
        if in_syntax_block && SYNTAX_BODY_COMMANDS.contains(&cmd_lower.as_str()) {
            continue;
        }
        // Any other recognized top-level command ends the syntax block.
        in_syntax_block = false;

        match cmd_lower.as_str() {
            "set" | "unset" => {
                let enable = cmd_lower == "set";
                let (name, after) = split_first(rest);
                if name.is_empty() {
                    warnings.push(format!("nanorc:{}: `{}` with no option name", lineno + 1, cmd_lower));
                    continue;
                }
                let (arg, _) = take_arg(after);
                if let Err(e) = settings::apply(options, &name.to_ascii_lowercase(), arg.as_deref(), enable) {
                    warnings.push(format!("nanorc:{}: {}", lineno + 1, e));
                }
            }
            "bind" => {
                let (key_spec, after) = split_first(rest);
                let Some(key) = Key::parse(key_spec) else {
                    warnings.push(format!("nanorc:{}: invalid key spec `{key_spec}`", lineno + 1));
                    continue;
                };
                if !crate::keymap::is_rebindable(&key) {
                    warnings.push(format!("nanorc:{}: `{key_spec}` cannot be rebound", lineno + 1));
                    continue;
                }
                let (arg, after) = take_arg(after);
                let Some(arg) = arg else {
                    warnings.push(format!("nanorc:{}: `bind` missing function/string", lineno + 1));
                    continue;
                };
                let (menu_name, _) = split_first(after);
                if menu_name.is_empty() {
                    warnings.push(format!("nanorc:{}: `bind` missing menu", lineno + 1));
                    continue;
                }
                let binding = if let Some(action) = Action::from_name(&arg) {
                    Binding::Action(action)
                } else {
                    Binding::Macro(arg)
                };
                apply_bind(keymap, &menu_name.to_ascii_lowercase(), key, binding, warnings, lineno);
            }
            "unbind" => {
                let (key_spec, after) = split_first(rest);
                let Some(key) = Key::parse(key_spec) else {
                    warnings.push(format!("nanorc:{}: invalid key spec `{key_spec}`", lineno + 1));
                    continue;
                };
                let (menu_name, _) = split_first(after);
                if menu_name.eq_ignore_ascii_case("all") {
                    keymap.unbind_all_menus(key);
                } else if let Some(menu) = Menu::from_name(&menu_name.to_ascii_lowercase()) {
                    keymap.unbind(menu, key);
                } else {
                    warnings.push(format!("nanorc:{}: unknown menu `{menu_name}`", lineno + 1));
                }
            }
            _ => {
                warnings.push(format!("nanorc:{}: unrecognized command `{cmd}`", lineno + 1));
            }
        }
    }
}

fn apply_bind(
    keymap: &mut KeyMap,
    menu_name: &str,
    key: Key,
    binding: Binding,
    warnings: &mut Vec<String>,
    lineno: usize,
) {
    if menu_name.eq_ignore_ascii_case("all") {
        keymap.bind_all_menus(Menu::ALL, key, binding);
    } else if let Some(menu) = Menu::from_name(menu_name) {
        keymap.bind(menu, key, binding);
    } else {
        warnings.push(format!("nanorc:{}: unknown menu `{menu_name}`", lineno + 1));
    }
}
