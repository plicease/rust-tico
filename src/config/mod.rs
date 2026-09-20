//! Configuration loading: `~/.nanorc` (nano-format, syntax-highlighting
//! directives ignored) and `~/.ticorc` (tico's own `[main]`/`[keybindings]`/
//! `[syntax]` INI-style format), applied in that precedence order (ticorc
//! wins on conflicts), and finally overridden by CLI flags.

pub mod nanorc;
mod settings;
pub mod ticorc;

use crate::keymap::KeyMap;
use crate::options::Options;
use std::path::PathBuf;

pub struct LoadedConfig {
    pub options: Options,
    pub keymap: KeyMap,
    pub warnings: Vec<String>,
}

fn nanorc_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(sys) = std::env::var("TICO_SYSTEM_NANORC") {
        paths.push(PathBuf::from(sys));
    } else {
        paths.push(PathBuf::from("/etc/nanorc"));
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            paths.push(PathBuf::from(xdg).join("nano/nanorc"));
        } else {
            paths.push(home.join(".config/nano/nanorc"));
        }
        paths.push(home.join(".nanorc"));
    }
    paths
}

fn ticorc_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg).join("tico/ticorc");
        if p.exists() {
            return Some(p);
        }
    }
    dirs::home_dir().map(|h| h.join(".ticorc"))
}

/// Load configuration: system nanorc, then the first user nanorc found
/// (`~/.nanorc`, `$XDG_CONFIG_HOME/nano/nanorc`, `~/.config/nano/nanorc`,
/// whichever is found first, matching nano's own search order), then
/// `~/.ticorc` (which takes precedence over nanorc on conflicting settings).
///
/// If `explicit_rcfile` is `Some`, only that single file is read (mirrors
/// nano's `--rcfile`), and `~/.ticorc` is still applied afterward unless
/// `ignore_ticorc` is set.
pub fn load(explicit_rcfile: Option<&str>, ignore_rcfiles: bool) -> LoadedConfig {
    let mut options = Options::default();
    let mut keymap = KeyMap::defaults();
    let mut warnings = Vec::new();

    if ignore_rcfiles {
        return LoadedConfig { options, keymap, warnings };
    }

    if let Some(path) = explicit_rcfile {
        if let Ok(text) = std::fs::read_to_string(path) {
            nanorc::parse(&text, &mut options, &mut keymap, &mut warnings);
        } else {
            warnings.push(format!("could not read rcfile: {path}"));
        }
    } else {
        // System-wide file, always read if present.
        if let Ok(text) = std::fs::read_to_string("/etc/nanorc") {
            nanorc::parse(&text, &mut options, &mut keymap, &mut warnings);
        }
        // First user nanorc found, in nano's documented search order.
        let candidates = nanorc_paths();
        for path in candidates.into_iter().skip(1) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                nanorc::parse(&text, &mut options, &mut keymap, &mut warnings);
                break;
            }
        }
    }

    if let Some(path) = ticorc_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            ticorc::parse(&text, &mut options, &mut keymap, &mut warnings);
        }
    }

    LoadedConfig { options, keymap, warnings }
}
