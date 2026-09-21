//! Parser for tico's own `~/.ticorc`: an INI-style file with three sections.
//!
//! - `[main]`: the same `set`-style option vocabulary as `~/.nanorc` (see
//!   `nanorc(5)`), one option per line, without the leading `set`/`unset`
//!   keyword: `optionname` or `optionname = value` to set/configure it,
//!   and `unset optionname` to explicitly turn a boolean off.
//! - `[keybindings]`: `[menu.]key = function` (menu defaults to `main`),
//!   e.g. `^G = help` or `search.^Y = older`. A quoted value produces a
//!   literal-string/macro binding, as in nano's `bind key "string" menu`.
//!   `key = unbind` removes a binding.
//! - `[syntax]`: tico's own syntax-highlighting configuration; not
//!   required to resemble nano's `color`/`icolor` format at all.
//!   (Reserved for future use.)
//!
//! Settings here take precedence over `~/.nanorc`.

use super::settings;
use crate::keymap::{Action, Binding, Key, KeyMap, Menu};
use crate::options::Options;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Section {
    None,
    Main,
    KeyBindings,
    Syntax,
}

pub fn parse(text: &str, options: &mut Options, keymap: &mut KeyMap, warnings: &mut Vec<String>) {
    let mut section = Section::None;
    for (lineno, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim().to_ascii_lowercase();
            section = match name.as_str() {
                "main" => Section::Main,
                "keybindings" => Section::KeyBindings,
                "syntax" => Section::Syntax,
                _ => {
                    warnings.push(format!("ticorc:{}: unknown section [{name}]", lineno + 1));
                    Section::None
                }
            };
            continue;
        }
        match section {
            Section::Main => parse_main_line(line, options, warnings, lineno),
            Section::KeyBindings => parse_keybinding_line(line, keymap, warnings, lineno),
            Section::Syntax => {
                // Reserved: tico's own highlighting config, not yet
                // implemented. Lines here are accepted but currently unused.
            }
            Section::None => {
                warnings.push(format!(
                    "ticorc:{}: setting outside of a [section]: `{line}`",
                    lineno + 1
                ));
            }
        }
    }
}

fn parse_main_line(line: &str, options: &mut Options, warnings: &mut Vec<String>, lineno: usize) {
    let (first, rest) = split_first(line);
    let (name, enable, value_part) = if first.eq_ignore_ascii_case("unset") {
        let (n, r) = split_first(rest);
        (n, false, r)
    } else if first.eq_ignore_ascii_case("set") {
        let (n, r) = split_first(rest);
        (n, true, r)
    } else {
        (first, true, rest)
    };
    if name.is_empty() {
        return;
    }
    let value = extract_value(value_part);
    if let Err(e) = settings::apply(options, &name.to_ascii_lowercase(), value.as_deref(), enable) {
        warnings.push(format!("ticorc:{}: {}", lineno + 1, e));
    }
}

/// `optionname value`, `optionname = value`, or bare `optionname`.
fn extract_value(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(inner) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner.to_string())
    } else {
        Some(rest.to_string())
    }
}

fn parse_keybinding_line(line: &str, keymap: &mut KeyMap, warnings: &mut Vec<String>, lineno: usize) {
    let Some(eq) = line.find('=') else {
        warnings.push(format!("ticorc:{}: expected `key = function`: `{line}`", lineno + 1));
        return;
    };
    let lhs = line[..eq].trim();
    let rhs = line[eq + 1..].trim();

    let (menu_name, key_spec) = match lhs.split_once('.') {
        Some((m, k)) => (m, k),
        None => ("main", lhs),
    };
    let Some(key) = Key::parse(key_spec) else {
        warnings.push(format!("ticorc:{}: invalid key spec `{key_spec}`", lineno + 1));
        return;
    };

    if rhs.eq_ignore_ascii_case("unbind") || rhs.eq_ignore_ascii_case("none") {
        if menu_name.eq_ignore_ascii_case("all") {
            keymap.unbind_all_menus(key);
        } else if let Some(menu) = Menu::from_name(&menu_name.to_ascii_lowercase()) {
            keymap.unbind(menu, key);
        } else {
            warnings.push(format!("ticorc:{}: unknown menu `{menu_name}`", lineno + 1));
        }
        return;
    }

    if !crate::keymap::is_rebindable(&key) {
        warnings.push(format!("ticorc:{}: `{key_spec}` cannot be rebound", lineno + 1));
        return;
    }

    let binding = if let Some(inner) = rhs.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Binding::Macro(inner.to_string())
    } else if let Some(action) = Action::from_name(&rhs.to_ascii_lowercase()) {
        Binding::Action(action)
    } else {
        warnings.push(format!("ticorc:{}: unknown function `{rhs}`", lineno + 1));
        return;
    };

    if menu_name.eq_ignore_ascii_case("all") {
        keymap.bind_all_menus(Menu::ALL, key, binding);
    } else if let Some(menu) = Menu::from_name(&menu_name.to_ascii_lowercase()) {
        keymap.bind(menu, key, binding);
    } else {
        warnings.push(format!("ticorc:{}: unknown menu `{menu_name}`", lineno + 1));
    }
}

fn split_first(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}
