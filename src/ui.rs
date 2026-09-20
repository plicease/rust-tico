//! Terminal rendering and the interactive event loop, built on crossterm.
//! The on-screen layout (title bar, buffer, status line, two-line shortcut
//! bar) mirrors GNU nano's, with "tico" shown wherever nano would show its
//! own name.

use crate::app::{Editor, Mode, Prompt, PromptKind};
use crate::buffer::Pos;
use crate::keymap::{Action, Binding, Key as TKey, Menu};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use std::io::{self, Write};
use std::time::Duration;

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> io::Result<RawModeGuard> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(RawModeGuard)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub fn run(editor: &mut Editor) -> io::Result<()> {
    let _guard = RawModeGuard::new()?;
    if let Ok((cols, rows)) = size() {
        editor.screen_cols = cols as usize;
        editor.screen_rows = rows as usize;
    }

    // Full clear happens exactly once (here) and again on resize; every
    // other render overwrites each row's full width in place, so nothing
    // ever needs re-blanking (which is what caused the visible flicker:
    // clearing the whole screen before every redraw, even when idle).
    execute!(io::stdout(), Clear(ClearType::All))?;
    render_and_ring(editor)?;

    loop {
        if matches!(editor.mode, Mode::Quit) {
            break;
        }

        let mut dirty = false;

        if event::poll(Duration::from_millis(600))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    handle_key(editor, key);
                    dirty = true;
                }
                Event::Resize(cols, rows) => {
                    editor.screen_cols = cols as usize;
                    editor.screen_rows = rows as usize;
                    execute!(io::stdout(), Clear(ClearType::All))?;
                    dirty = true;
                }
                _ => {}
            }
        } else if matches!(editor.mode, Mode::Editing) {
            dirty = maybe_check_external_change(editor);
            dirty |= editor.tick_spotlight_deadline();
        }

        // Check Quit before rendering: an action (e.g. Exit with no
        // unsaved changes) may have just closed the last buffer, and
        // render() assumes there's always at least one to draw.
        if matches!(editor.mode, Mode::Quit) {
            break;
        }

        if dirty {
            render_and_ring(editor)?;
        }
    }
    Ok(())
}

/// Render, then ring the terminal bell exactly once if an Alert-level
/// message was just posted (matching nano's beep() for ALERT-importance
/// statusline() calls).
fn render_and_ring(editor: &mut Editor) -> io::Result<()> {
    render(editor)?;
    if editor.bell_pending {
        editor.bell_pending = false;
        print!("\x07");
        io::stdout().flush()?;
    }
    Ok(())
}

/// Returns true if editor state changed (and so needs a redraw).
fn maybe_check_external_change(editor: &mut Editor) -> bool {
    use crate::fileio::ExternalChange;
    match crate::fileio::check_external_change(editor.buf()) {
        ExternalChange::Unchanged => return false,
        ExternalChange::ChangedNoLocalEdits => {
            let _ = crate::fileio::reload(editor.buf_mut());
            editor.set_status("File reloaded (changed on disk)");
        }
        ExternalChange::ChangedWithLocalEdits => {
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::ExternalChangeConflict,
                menu: Menu::YesNo,
                label: "File changed on disk and you have unsaved edits: [R]eload  [K]eep mine  [M]erge  [C]ancel"
                    .to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        }
    }
    true
}

// ---------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------

fn handle_key(editor: &mut Editor, key: KeyEvent) {
    match std::mem::replace(&mut editor.mode, Mode::Editing) {
        Mode::Editing => {
            editor.mode = Mode::Editing;
            // Matches nano's get_kbinput(): the countdown ticks once per
            // keystroke read from the main edit window, *before* that
            // keystroke is dispatched (so if the dispatch itself shows a
            // fresh message, this tick doesn't immediately eat into it).
            editor.tick_status_countdown();
            // A search match's highlight is cleared by the very next
            // keystroke, same as its status-message countdown above —
            // confirmed by timing the installed nano (a key press ends its
            // half-delay wait immediately, rather than waiting out the
            // ~1.5s timeout). Any spotlight still around here is a timed
            // one; the persistent replace-confirm kind only exists while
            // in Mode::Prompt, not here.
            editor.clear_spotlight();
            handle_editing_key(editor, key);
        }
        Mode::Help(lines) => {
            editor.mode = Mode::Help(lines);
            // Any key closes the help viewer, matching ^X/Cancel/Exit; full
            // scrolling within the viewer is not yet implemented.
            editor.mode = Mode::Editing;
        }
        Mode::Prompt(prompt) => handle_prompt_key(editor, prompt, key),
        Mode::Quit => editor.mode = Mode::Quit,
    }
}

fn handle_editing_key(editor: &mut Editor, key: KeyEvent) {
    if let Some(tkey) = normalize_key(key) {
        if let Some(binding) = editor.keymap.lookup(Menu::Main, tkey).cloned() {
            apply_binding(editor, binding);
            editor.maybe_update_lock_modified_flag();
            return;
        }
    }
    if let KeyCode::Char(c) = key.code {
        if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            editor.insert_char(c);
        }
    }
    editor.maybe_update_lock_modified_flag();
}

fn apply_binding(editor: &mut Editor, binding: Binding) {
    match binding {
        Binding::Action(action) => editor.execute(action),
        Binding::Macro(text) => {
            // Literal-string bindings; `{function}` substitution is not yet
            // implemented, so braces are inserted literally.
            for c in text.chars() {
                editor.insert_char(c);
            }
        }
    }
}

fn handle_prompt_key(editor: &mut Editor, mut prompt: Prompt, key: KeyEvent) {
    // Single-keystroke choice prompts (yes/no/conflict resolution) are
    // handled directly, without going through the text-editing path.
    match &prompt.kind {
        PromptKind::Exit { .. } => return handle_exit_choice(editor, prompt, key),
        PromptKind::ExternalChangeConflict => return handle_conflict_choice(editor, key),
        PromptKind::MergeConflict | PromptKind::MergePreviewClean { .. } => {
            return handle_merge_preview_choice(editor, prompt, key)
        }
        PromptKind::LockConflict { .. } => return handle_lock_conflict_choice(editor, prompt, key),
        PromptKind::ReplaceConfirm(_) => return handle_replace_confirm_choice(editor, prompt, key),
        _ => {}
    }

    if let Some(tkey) = normalize_key(key) {
        if tkey == TKey::Ctrl('C') || matches!(key.code, KeyCode::Esc) {
            editor.mode = Mode::Editing;
            editor.set_status("Cancelled");
            return;
        }
        if tkey == TKey::Ctrl('M') {
            submit_prompt(editor, prompt);
            return;
        }
        if tkey == TKey::Ctrl('H') {
            if prompt.cursor > 0 {
                let idx = prompt.input.char_indices().nth(prompt.cursor - 1).map(|(i, _)| i).unwrap_or(0);
                prompt.input.remove(idx);
                prompt.cursor -= 1;
            }
            prompt.history_pos = None;
            prompt.saved_input = None;
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        if tkey == TKey::Left {
            prompt.cursor = prompt.cursor.saturating_sub(1);
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        if tkey == TKey::Right {
            prompt.cursor = (prompt.cursor + 1).min(prompt.input.chars().count());
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        // Menu-specific bindings (e.g. ^Y/^V to jump straight to the first
        // or last line from the Search/GotoLine prompts, without needing
        // to type anything) — see keymap.rs's install_prompt_defaults for
        // the full, source-verified list.
        if let Some(Binding::Action(action)) = editor.keymap.lookup_menu_only(prompt.menu, tkey).cloned() {
            if apply_prompt_action(editor, &mut prompt, action) {
                return;
            }
            editor.mode = Mode::Prompt(prompt);
            return;
        }
    }
    if let KeyCode::Char(c) = key.code {
        if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            let idx = prompt.input.char_indices().nth(prompt.cursor).map(|(i, _)| i).unwrap_or(prompt.input.len());
            prompt.input.insert(idx, c);
            prompt.cursor += 1;
            prompt.history_pos = None;
            prompt.saved_input = None;
        }
    }
    editor.mode = Mode::Prompt(prompt);
}

/// Apply an Action bound within a prompt menu, which generally means
/// something different from its effect while editing the buffer directly.
/// Returns true if the prompt was closed (editor.mode has already been
/// set); false if it should stay open (the caller restores
/// `Mode::Prompt(prompt)`).
///
/// Only the bindings that make sense to act on immediately are handled
/// here; others (FlipReplace/FlipGoto prompt-kind switching, Older/Newer
/// history recall — history isn't implemented yet) are recognized by the
/// keymap but not yet wired to a prompt-time effect, and are ignored here
/// rather than silently doing the wrong thing.
fn apply_prompt_action(editor: &mut Editor, prompt: &mut Prompt, action: Action) -> bool {
    match action {
        Action::FirstLine => {
            editor.buf_mut().cursor = Pos::new(0, 0);
            editor.scroll_to_cursor();
            editor.mode = Mode::Editing;
            true
        }
        Action::LastLine => {
            let last = editor.buf().line_count().saturating_sub(1);
            editor.buf_mut().cursor = Pos::new(last, 0);
            editor.scroll_to_cursor();
            editor.mode = Mode::Editing;
            true
        }
        Action::CaseSens => {
            editor.search.case_sensitive = !editor.search.case_sensitive;
            refresh_search_label(editor, prompt);
            false
        }
        Action::Regexp => {
            editor.search.use_regex = !editor.search.use_regex;
            refresh_search_label(editor, prompt);
            false
        }
        Action::Backwards => {
            editor.search.backwards = !editor.search.backwards;
            refresh_search_label(editor, prompt);
            false
        }
        // ^R in the Search prompt switches it into a replace operation
        // (keeping whatever was typed); ^R again from there switches back
        // to plain search — matches nano's flip_replace, bound to MWHEREIS
        // and MREPLACE both.
        Action::FlipReplace => {
            match prompt.menu {
                Menu::Search => {
                    prompt.kind = PromptKind::Replace1;
                    prompt.menu = Menu::Replace;
                }
                Menu::Replace => {
                    prompt.kind = PromptKind::WhereIs;
                    prompt.menu = Menu::Search;
                }
                _ => return false,
            }
            refresh_search_label(editor, prompt);
            false
        }
        // ^T flips between the Search and GotoLine prompts (MWHEREIS and
        // MGOTOLINE both bind it to flip_goto).
        Action::FlipGoto => {
            match prompt.menu {
                Menu::Search => {
                    prompt.kind = PromptKind::GotoLine;
                    prompt.menu = Menu::GotoLine;
                    prompt.label = "Enter line number, column number".to_string();
                }
                Menu::GotoLine => {
                    prompt.kind = PromptKind::WhereIs;
                    prompt.menu = Menu::Search;
                    refresh_search_label(editor, prompt);
                }
                _ => return false,
            }
            false
        }
        Action::Older => {
            cycle_history(editor, prompt, true);
            false
        }
        Action::Newer => {
            cycle_history(editor, prompt, false);
            false
        }
        _ => false,
    }
}

/// Recall history at a Search/Replace/ReplaceWith/Execute prompt with
/// Older (Up/^P) or Newer (Down/^N) — matches nano's get_older_item()/
/// get_newer_item(), confirmed against the installed nano: Older steps
/// backward through that menu's history (most recent first), Newer steps
/// forward, and stepping Newer past the most recent entry restores
/// whatever was live-typed before browsing started.
fn cycle_history(editor: &mut Editor, prompt: &mut Prompt, older: bool) {
    let list: &[String] = match prompt.menu {
        Menu::Search | Menu::Replace => &editor.history.search,
        Menu::ReplaceWith => &editor.history.replace,
        Menu::Execute => &editor.history.execute,
        _ => return,
    };
    if list.is_empty() {
        return;
    }
    if older {
        let next = match prompt.history_pos {
            None => 0,
            Some(i) if i + 1 < list.len() => i + 1,
            Some(i) => i,
        };
        if prompt.history_pos.is_none() {
            prompt.saved_input = Some(prompt.input.clone());
        }
        prompt.history_pos = Some(next);
        prompt.input = list[list.len() - 1 - next].clone();
        prompt.cursor = prompt.input.chars().count();
    } else {
        match prompt.history_pos {
            None => {}
            Some(0) => {
                prompt.history_pos = None;
                prompt.input = prompt.saved_input.take().unwrap_or_default();
                prompt.cursor = prompt.input.chars().count();
            }
            Some(i) => {
                let next = i - 1;
                prompt.history_pos = Some(next);
                prompt.input = list[list.len() - 1 - next].clone();
                prompt.cursor = prompt.input.chars().count();
            }
        }
    }
}

/// Rebuild a Search/Replace prompt's label from the current toggle state,
/// preserving whichever suffix belongs to its menu (see
/// `app::search_prompt_label`).
fn refresh_search_label(editor: &Editor, prompt: &mut Prompt) {
    let suffix = if prompt.menu == Menu::Replace { " (to replace)" } else { "" };
    prompt.label = crate::app::search_prompt_label("Search", suffix, &editor.search);
}

fn handle_exit_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::Exit { .. } = &prompt.kind else { return };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            editor.mode = Mode::Editing;
            editor.begin_writeout_for_exit();
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            editor.close_current_buffer();
            if !matches!(editor.mode, Mode::Quit) {
                editor.mode = Mode::Editing;
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
            editor.mode = Mode::Editing;
            editor.set_status("Cancelled");
        }
        _ => editor.mode = Mode::Prompt(prompt),
    }
}

fn handle_conflict_choice(editor: &mut Editor, key: KeyEvent) {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let _ = crate::fileio::reload(editor.buf_mut());
            editor.mode = Mode::Editing;
            editor.set_status("Reloaded from disk; local edits discarded");
        }
        KeyCode::Char('k') | KeyCode::Char('K') => {
            if let Some(path) = editor.buf().path.clone() {
                editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
            }
            editor.mode = Mode::Editing;
            editor.set_status("Kept your local edits");
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            editor.begin_merge_preview();
        }
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
            editor.mode = Mode::Editing;
        }
        _ => {}
    }
}

fn handle_merge_preview_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    match &prompt.kind {
        PromptKind::MergeConflict => {
            // No automatic resolution possible; return to the main conflict
            // choice so the user can pick reload/keep instead.
            if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(_)) {
                editor.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::ExternalChangeConflict,
                    menu: Menu::YesNo,
                    label: "Could not merge automatically: [R]eload  [K]eep mine  [C]ancel".to_string(),
                    input: String::new(),
                    cursor: 0,
                    history_pos: None,
                    saved_input: None,
                });
            }
        }
        PromptKind::MergePreviewClean { merged_text } => match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Enter => {
                let text = merged_text.clone();
                editor.buf_mut().rope = ropey::Rope::from_str(&text);
                editor.buf_mut().modified = true;
                if let Some(path) = editor.buf().path.clone() {
                    editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
                }
                editor.mode = Mode::Editing;
                editor.set_status("Merged");
            }
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                editor.mode = Mode::Editing;
            }
            _ => {}
        },
        _ => {}
    }
}

fn handle_lock_conflict_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::LockConflict { lock_path, target } = &prompt.kind else { return };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let _ = crate::lockfile::write_lock(lock_path, target, false);
            editor.buf_mut().lock_filename = Some(lock_path.clone());
            editor.mode = Mode::Editing;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
            // Matches nano: declining leaves this buffer unopened. If it
            // was the only one, fall back to a blank buffer rather than
            // quitting (nano's read_files_from_cmdline() does the same
            // when every given file was declined or invalid).
            editor.buffers.remove(editor.current);
            if editor.buffers.is_empty() {
                editor.buffers.push(crate::buffer::Buffer::empty());
                editor.current = 0;
            } else if editor.current >= editor.buffers.len() {
                editor.current = editor.buffers.len() - 1;
            }
            editor.mode = Mode::Editing;
        }
        _ => editor.mode = Mode::Prompt(prompt),
    }
}

/// The "Replace this instance?" prompt: Y/y = Yes, N/n = No, A/a = All,
/// ^C/Esc = Cancel — matches nano's `ask_user(YESORALLORNO, ...)` exactly
/// (src/prompt.c), including that Esc cancels there too (Cancel is bound
/// to whatever key the MYESNO menu's cancel function has, which includes
/// Esc via the generic Ctrl-C/Esc handling used throughout this UI).
fn handle_replace_confirm_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::ReplaceConfirm(state) = prompt.kind else { return };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => editor.replace_choice(state, crate::app::ReplaceChoice::Yes),
        KeyCode::Char('n') | KeyCode::Char('N') => editor.replace_choice(state, crate::app::ReplaceChoice::No),
        KeyCode::Char('a') | KeyCode::Char('A') => editor.replace_choice(state, crate::app::ReplaceChoice::All),
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
            editor.replace_choice(state, crate::app::ReplaceChoice::Cancel)
        }
        _ => {
            editor.mode = Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(state), ..prompt });
        }
    }
}

fn submit_prompt(editor: &mut Editor, prompt: Prompt) {
    let text = prompt.input.clone();
    match prompt.kind {
        PromptKind::WhereIs => {
            // Pressing Enter with nothing typed reuses the remembered last
            // search term (shown bracketed in the label) - confirmed
            // against the installed nano.
            let text = if text.is_empty() { editor.search.last_pattern.clone().unwrap_or_default() } else { text };
            editor.mode = Mode::Editing;
            if text.is_empty() {
                return;
            }
            editor.history.add_search(&text);
            let backwards = editor.search.backwards;
            editor.run_search(&text, backwards);
        }
        PromptKind::Replace1 => {
            let text = if text.is_empty() { editor.search.last_pattern.clone().unwrap_or_default() } else { text };
            if text.is_empty() {
                editor.mode = Mode::Editing;
                return;
            }
            editor.history.add_search(&text);
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::Replace2 { search: text },
                menu: Menu::ReplaceWith,
                label: "Replace with".to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        }
        PromptKind::Replace2 { search } => {
            editor.history.add_replace(&text);
            editor.begin_replace_loop(search, text);
        }
        PromptKind::GotoLine => {
            editor.mode = Mode::Editing;
            let (line_s, col_s) = text.split_once(',').unwrap_or((text.as_str(), ""));
            if let Ok(line) = line_s.trim().parse::<i64>() {
                let total = editor.buf().line_count() as i64;
                let target_line = if line < 0 { (total + line).max(0) } else { (line - 1).max(0) };
                let col = col_s.trim().parse::<i64>().unwrap_or(1).max(1) as usize - 1;
                editor.buf_mut().cursor = Pos::new(target_line as usize, col);
            }
        }
        PromptKind::WriteOut { exiting } => {
            editor.mode = Mode::Editing;
            let path = std::path::PathBuf::from(text);
            match crate::fileio::save_file(editor.buf_mut(), &path) {
                Ok(()) => {
                    editor.set_status(format!("Wrote {}", path.display()));
                    if exiting {
                        editor.close_current_buffer();
                    }
                }
                Err(e) => editor.set_status(format!("Error writing file: {e}")),
            }
        }
        _ => {
            editor.mode = Mode::Editing;
        }
    }
}

// ---------------------------------------------------------------------
// Crossterm key normalization
// ---------------------------------------------------------------------

fn normalize_key(key: KeyEvent) -> Option<TKey> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        KeyCode::Backspace => Some(TKey::Ctrl('H')),
        KeyCode::Tab => Some(TKey::Ctrl('I')),
        KeyCode::BackTab => Some(TKey::ShiftTab),
        KeyCode::Enter => Some(TKey::Ctrl('M')),
        KeyCode::Esc => None,
        KeyCode::Left if ctrl => Some(TKey::CtrlLeft),
        KeyCode::Right if ctrl => Some(TKey::CtrlRight),
        KeyCode::Up if ctrl => Some(TKey::CtrlUp),
        KeyCode::Down if ctrl => Some(TKey::CtrlDown),
        KeyCode::Left if alt => Some(TKey::MetaLeft),
        KeyCode::Right if alt => Some(TKey::MetaRight),
        KeyCode::Up if alt => Some(TKey::MetaUp),
        KeyCode::Down if alt => Some(TKey::MetaDown),
        KeyCode::Left => Some(TKey::Left),
        KeyCode::Right => Some(TKey::Right),
        KeyCode::Up => Some(TKey::Up),
        KeyCode::Down => Some(TKey::Down),
        KeyCode::Home if ctrl => Some(TKey::CtrlHome),
        KeyCode::End if ctrl => Some(TKey::CtrlEnd),
        KeyCode::Home if alt => Some(TKey::MetaHome),
        KeyCode::End if alt => Some(TKey::MetaEnd),
        KeyCode::Home => Some(TKey::Home),
        KeyCode::End => Some(TKey::End),
        KeyCode::PageUp if alt => Some(TKey::MetaPgUp),
        KeyCode::PageDown if alt => Some(TKey::MetaPgDn),
        KeyCode::PageUp => Some(TKey::PageUp),
        KeyCode::PageDown => Some(TKey::PageDown),
        KeyCode::Delete if ctrl && shift => Some(TKey::ShiftCtrlDel),
        KeyCode::Delete if ctrl => Some(TKey::CtrlDel),
        KeyCode::Delete if alt => Some(TKey::MetaDel),
        KeyCode::Delete => Some(TKey::Ctrl('D')),
        KeyCode::Insert if alt => Some(TKey::MetaIns),
        KeyCode::Insert => Some(TKey::Ins),
        KeyCode::F(n) => Some(TKey::F(n)),
        KeyCode::Char(c) if ctrl => {
            // crossterm's unix parser reports Ctrl+\, Ctrl+], Ctrl+^ and
            // Ctrl+_ (raw bytes 0x1C-0x1F) as Char('4')..Char('7') with
            // CONTROL set — those bytes are indistinguishable on the wire
            // from an actual Ctrl+digit, and crossterm picks the digit
            // form. Map back to the symbol form nano's docs, our keymap
            // defaults, and nanorc `bind` lines all use.
            let mapped = match c {
                '4' => '\\',
                '5' => ']',
                '6' => '^',
                '7' => '_',
                other => other.to_ascii_uppercase(),
            };
            Some(TKey::Ctrl(mapped))
        }
        KeyCode::Char(c) if alt && shift && c.is_ascii_alphabetic() => Some(TKey::ShiftMeta(c.to_ascii_uppercase())),
        // Meta+letter is case-insensitive by default in nano (a bare
        // Meta+letter keystroke does the same as Shift+Meta+letter unless
        // a specific Sh-M- binding overrides it), and our keymap stores
        // Meta letter bindings uppercase, so normalize here too.
        KeyCode::Char(c) if alt => Some(TKey::Meta(c.to_ascii_uppercase())),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

fn render(editor: &Editor) -> io::Result<()> {
    // No full-screen Clear here: every row below is redrawn at its full
    // width, so nothing needs re-blanking first (a per-frame Clear was the
    // cause of visible flicker). The screen is cleared once at startup and
    // again on resize, in `run()`.
    let mut out = io::stdout();
    // Hide the cursor for the duration of the redraw: otherwise the
    // terminal's real hardware cursor stays visible and visibly jumps to
    // every intermediate MoveTo position used while painting each row,
    // instead of moving straight to its final spot.
    queue!(out, Hide)?;

    let cols = editor.screen_cols;
    let rows = editor.screen_rows;
    if editor.options.zero {
        render_buffer(editor, &mut out, 0, rows)?;
        return finish_cursor(editor, &mut out, 0);
    }

    queue!(out, MoveTo(0, 0))?;
    render_title_bar(editor, &mut out, cols)?;

    let help_rows = if editor.options.nohelp { 0 } else { 2 };
    let text_start_row = 1u16;
    let text_rows = rows.saturating_sub(2 + help_rows);
    render_buffer(editor, &mut out, text_start_row, text_rows)?;

    let status_row = text_start_row + text_rows as u16;
    render_status_line(editor, &mut out, status_row, cols)?;

    if help_rows > 0 {
        let menu = if let Mode::Prompt(p) = &editor.mode { Some(p.menu) } else { None };
        render_shortcut_bar(&mut out, status_row + 1, cols, shortcuts_for_menu(menu))?;
    }

    finish_cursor(editor, &mut out, text_start_row)?;
    out.flush()
}

fn finish_cursor(editor: &Editor, out: &mut impl Write, text_start_row: u16) -> io::Result<()> {
    if let Mode::Prompt(prompt) = &editor.mode {
        let row = editor.screen_rows.saturating_sub(if editor.options.nohelp { 1 } else { 3 });
        let col = prompt.label.chars().count() + 2 + prompt.cursor;
        queue!(out, MoveTo(col.min(editor.screen_cols.saturating_sub(1)) as u16, row as u16), Show)?;
    } else {
        let buf = editor.buf();
        let screen_line = buf.cursor.line.saturating_sub(buf.top_line);
        let gutter = line_number_gutter_width(editor);
        let col = gutter + display_width(&buf.line(buf.cursor.line), buf.cursor.col, editor.options.tabsize as usize);
        queue!(
            out,
            MoveTo(col.min(editor.screen_cols.saturating_sub(1)) as u16, (text_start_row as usize + screen_line) as u16),
            Show
        )?;
    }
    Ok(())
}

fn line_number_gutter_width(editor: &Editor) -> usize {
    if !editor.options.linenumbers {
        return 0;
    }
    let digits = editor.buf().line_count().to_string().len();
    digits + 1
}

fn display_width(line: &str, up_to_col: usize, tabsize: usize) -> usize {
    let mut w = 0;
    for (i, c) in line.chars().enumerate() {
        if i >= up_to_col {
            break;
        }
        if c == '\t' {
            w += tabsize - (w % tabsize);
        } else {
            w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        }
    }
    w
}

fn render_title_bar(editor: &Editor, out: &mut impl Write, cols: usize) -> io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let left = format!("  tico {version}");
    let name = editor
        .buf()
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "New Buffer".to_string());
    let modified = if editor.buf().modified { " *" } else { "" };
    let mut center_text = format!("{name}{modified}");

    // Reserve space for the left prefix (plus one column of separation on
    // each side); if the filename doesn't fit, truncate it, keeping the
    // tail (the most identifying part of a long path) and prefixing "...".
    let left_w = left.chars().count();
    let available = cols.saturating_sub(left_w + 2);
    if center_text.chars().count() > available {
        if available > 3 {
            let tail: String = center_text
                .chars()
                .rev()
                .take(available - 3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            center_text = format!("...{tail}");
        } else {
            center_text.clear();
        }
    }

    let mut line = vec![' '; cols];
    for (i, c) in left.chars().enumerate() {
        if i < cols {
            line[i] = c;
        }
    }
    let start = (cols.saturating_sub(center_text.chars().count()) / 2).max(left_w + 1);
    for (i, c) in center_text.chars().enumerate() {
        if start + i < cols {
            line[start + i] = c;
        }
    }
    let s: String = line.into_iter().collect();
    queue!(out, SetAttribute(Attribute::Reverse), Print(s), SetAttribute(Attribute::Reset))
}

fn render_status_line(editor: &Editor, out: &mut impl Write, row: u16, cols: usize) -> io::Result<()> {
    queue!(out, MoveTo(0, row))?;
    if let Mode::Prompt(prompt) = &editor.mode {
        // nano's promptcolor defaults to the title bar's colors (reverse
        // video), confirmed against the installed nano's own escape-code
        // output for both the Search and WriteOut prompts.
        let text = format!("{}: {}", prompt.label, prompt.input);
        let mut s: String = text.chars().take(cols).collect();
        while s.chars().count() < cols {
            s.push(' ');
        }
        queue!(out, SetAttribute(Attribute::Reverse), Print(s), SetAttribute(Attribute::Reset))
    } else if let Some(msg) = &editor.status {
        // nano shows ordinary status-bar messages in reverse video, and
        // Alert-level ones (unwritable file, "is a directory", ...) bold
        // white-on-red instead (confirmed against the installed nano's own
        // escape-code output for both cases).
        let bracketed = format!("[ {msg} ]");
        let pad = cols.saturating_sub(bracketed.chars().count()) / 2;
        if pad > 0 {
            queue!(out, Print(" ".repeat(pad)))?;
        }
        let remaining = cols.saturating_sub(pad);
        let shown: String = bracketed.chars().take(remaining).collect();
        let shown_len = shown.chars().count();
        match editor.status_level {
            crate::app::StatusLevel::Normal => {
                queue!(out, SetAttribute(Attribute::Reverse), Print(shown), SetAttribute(Attribute::Reset))?;
            }
            crate::app::StatusLevel::Mild | crate::app::StatusLevel::Alert => {
                // Matches nano's captured escape codes exactly: ESC[1m
                // ESC[37m ESC[41m — bold, *standard* white (crossterm's
                // `Grey`, not `White`, which is bright/ANSI-97), on
                // standard (non-bright) red. nano uses this same
                // ERROR_MESSAGE color for both MILD and ALERT messages;
                // only ALERT also rings the bell (handled via
                // `bell_pending`, which `set_status_mild` never sets).
                queue!(
                    out,
                    SetAttribute(Attribute::Bold),
                    SetForegroundColor(Color::Grey),
                    SetBackgroundColor(Color::DarkRed),
                    Print(shown),
                    SetAttribute(Attribute::Reset)
                )?;
            }
        }
        let used = pad + shown_len;
        if used < cols {
            queue!(out, Print(" ".repeat(cols - used)))?;
        }
        Ok(())
    } else {
        queue!(out, Print(" ".repeat(cols)))
    }
}

/// The default main-menu shortcut priority list, in the exact order GNU
/// nano 8.7.1 lays them out (captured directly from the installed binary),
/// as (key label, description) pairs, filled two rows at a time into as
/// many columns as fit the terminal width.
const SHORTCUT_PRIORITY: &[(&str, &str)] = &[
    ("^G", "Help"),
    ("^X", "Exit"),
    ("^O", "Write Out"),
    ("^R", "Read File"),
    ("^F", "Where Is"),
    ("^\\", "Replace"),
    ("^K", "Cut"),
    ("^U", "Paste"),
    ("^T", "Execute"),
    ("^J", "Justify"),
    ("^C", "Location"),
    ("^/", "Go To Line"),
    ("M-U", "Undo"),
    ("M-E", "Redo"),
    ("M-A", "Set Mark"),
    ("M-6", "Copy"),
    ("M-]", "To Bracket"),
    ("^B", "Where Was"),
    ("M-B", "Previous"),
    ("M-F", "Next"),
];

/// The Search (WhereIs) prompt's shortcut list, captured the same way.
const SEARCH_SHORTCUTS: &[(&str, &str)] = &[
    ("^G", "Help"),
    ("^C", "Cancel"),
    ("M-C", "Case Sens"),
    ("M-R", "Reg.exp."),
    ("M-B", "Backwards"),
    ("^R", "Replace"),
    ("^P", "Older"),
    ("^N", "Newer"),
    ("^T", "Go To Line"),
];

/// The "Search (to replace)" prompt: same as Search but without ^T
/// (MREPLACE isn't bound to flip_goto in nano) and ^R now offers to flip
/// *back* to plain search.
const REPLACE1_SHORTCUTS: &[(&str, &str)] = &[
    ("^G", "Help"),
    ("^C", "Cancel"),
    ("M-C", "Case Sens"),
    ("M-R", "Reg.exp."),
    ("M-B", "Backwards"),
    ("^R", "No Replace"),
    ("^P", "Older"),
    ("^N", "Newer"),
];

const REPLACEWITH_SHORTCUTS: &[(&str, &str)] = &[("^G", "Help"), ("^C", "Cancel"), ("^P", "Older"), ("^N", "Newer")];

const GOTOLINE_SHORTCUTS: &[(&str, &str)] = &[
    ("^G", "Help"),
    ("^C", "Cancel"),
    ("^W", "Begin of Paragr."),
    ("^O", "End of Paragraph"),
    ("^Y", "First Line"),
    ("^V", "Last Line"),
    ("^T", "Go To Text"),
];

/// Which shortcut list to show at the bottom for the given menu — nano
/// rebuilds its two help lines per-menu (see e.g. ask_user()'s
/// post_one_key calls); menus not yet curated here fall back to Main's
/// list rather than showing nothing.
fn shortcuts_for_menu(menu: Option<Menu>) -> &'static [(&'static str, &'static str)] {
    match menu {
        Some(Menu::Search) => SEARCH_SHORTCUTS,
        Some(Menu::Replace) => REPLACE1_SHORTCUTS,
        Some(Menu::ReplaceWith) => REPLACEWITH_SHORTCUTS,
        Some(Menu::GotoLine) => GOTOLINE_SHORTCUTS,
        _ => SHORTCUT_PRIORITY,
    }
}

fn render_shortcut_bar(out: &mut impl Write, row: u16, cols: usize, entries: &[(&str, &str)]) -> io::Result<()> {
    let max_label = entries.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(2);
    let max_desc = entries.iter().map(|(_, d)| d.chars().count()).max().unwrap_or(4);
    let col_width = max_label + 1 + max_desc + 2;
    let n_cols = (cols / col_width).max(1);
    let n_pairs = n_cols.min(entries.len().div_ceil(2));

    for r in 0..2u16 {
        queue!(out, MoveTo(0, row + r))?;
        let mut written = 0usize;
        for c in 0..n_pairs {
            let idx = c * 2 + r as usize;
            if let Some((key, desc)) = entries.get(idx) {
                // As in nano: the key combo is shown in reverse video, the
                // description in the terminal's normal colors.
                let key_padded = format!("{key:<lw$}", lw = max_label);
                queue!(out, SetAttribute(Attribute::Reverse), Print(&key_padded), SetAttribute(Attribute::Reset))?;
                let rest = format!(" {desc:<dw$}  ", dw = max_desc);
                queue!(out, Print(&rest))?;
                written += key_padded.chars().count() + rest.chars().count();
            } else {
                let pad = " ".repeat(col_width);
                queue!(out, Print(&pad))?;
                written += pad.chars().count();
            }
        }
        if written < cols {
            queue!(out, Print(" ".repeat(cols - written)))?;
        }
    }
    Ok(())
}

fn render_buffer(editor: &Editor, out: &mut impl Write, start_row: u16, rows: usize) -> io::Result<()> {
    let buf = editor.buf();
    let gutter = line_number_gutter_width(editor);
    let cols = editor.screen_cols;
    let tabsize = editor.options.tabsize as usize;
    for r in 0..rows {
        queue!(out, MoveTo(0, start_row + r as u16))?;
        let line_idx = buf.top_line + r;
        let mut rendered = String::new();
        // Character range within `rendered` (post-gutter, post-tab-expansion)
        // to paint with spotlightcolor, if the active search/replace match
        // is on this line.
        let mut highlight: Option<(usize, usize)> = None;
        if line_idx < buf.line_count() {
            if gutter > 0 {
                rendered.push_str(&format!("{:>width$} ", line_idx + 1, width = gutter - 1));
            }
            let raw = buf.line(line_idx);
            let gutter_chars = rendered.chars().count();
            rendered.push_str(&expand_tabs(&raw, tabsize));
            if let Some((pos, len)) = editor.spotlight {
                if pos.line == line_idx {
                    let start = gutter_chars + display_width(&raw, pos.col, tabsize);
                    let end = gutter_chars + display_width(&raw, pos.col + len, tabsize);
                    if end > start {
                        highlight = Some((start, end));
                    }
                }
            }
        } else if gutter > 0 {
            rendered.push('~');
        }

        let chars: Vec<char> = rendered.chars().take(cols).collect();
        let len = chars.len();
        match highlight.map(|(s, e)| (s.min(len), e.min(len))) {
            Some((s, e)) if s < e => {
                let (fg, bg) = spotlight_colors(&editor.options.spotlightcolor);
                let before: String = chars[..s].iter().collect();
                let mid: String = chars[s..e].iter().collect();
                let after: String = chars[e..].iter().collect();
                queue!(out, Print(before))?;
                queue!(out, SetForegroundColor(fg), SetBackgroundColor(bg), Print(mid), SetAttribute(Attribute::Reset))?;
                queue!(out, Print(after))?;
                if len < cols {
                    queue!(out, Print(" ".repeat(cols - len)))?;
                }
            }
            _ => {
                let s: String = chars.into_iter().collect();
                queue!(out, Print(format!("{s:<cols$}", cols = cols)))?;
            }
        }
    }
    Ok(())
}

/// Map a nanorc color spec to the crossterm colors that produce the same
/// escape codes as nano itself (crossterm's naming is inverted from
/// nano's: `Color::Red` is bright/light red, `Color::DarkRed` is the
/// standard-intensity red nano means by plain "red" — confirmed against
/// crossterm's own SGR-generation source).
fn spotlight_colors(cp: &crate::options::ColorPair) -> (Color, Color) {
    let fg = cp.fg.map(map_named_color).unwrap_or(Color::Black);
    let bg = cp.bg.map(map_named_color).unwrap_or(Color::Yellow);
    (fg, bg)
}

fn map_named_color(nc: crate::options::NamedColor) -> Color {
    use crate::options::Color as OC;
    match nc.color {
        OC::Black => {
            if nc.light {
                Color::DarkGrey
            } else {
                Color::Black
            }
        }
        OC::Red => {
            if nc.light {
                Color::Red
            } else {
                Color::DarkRed
            }
        }
        OC::Green => {
            if nc.light {
                Color::Green
            } else {
                Color::DarkGreen
            }
        }
        OC::Yellow => {
            if nc.light {
                Color::Yellow
            } else {
                Color::DarkYellow
            }
        }
        OC::Blue => {
            if nc.light {
                Color::Blue
            } else {
                Color::DarkBlue
            }
        }
        OC::Magenta => {
            if nc.light {
                Color::Magenta
            } else {
                Color::DarkMagenta
            }
        }
        OC::Cyan => {
            if nc.light {
                Color::Cyan
            } else {
                Color::DarkCyan
            }
        }
        OC::White => {
            if nc.light {
                Color::White
            } else {
                Color::Grey
            }
        }
        OC::Normal => Color::Reset,
        OC::Rgb(r, g, b) => Color::Rgb { r, g, b },
    }
}

fn expand_tabs(line: &str, tabsize: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = tabsize - (w % tabsize);
            for _ in 0..n {
                out.push(' ');
            }
            w += n;
        } else {
            out.push(c);
            w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        }
    }
    out
}
