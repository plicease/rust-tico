//! Terminal rendering and the interactive event loop, built on crossterm.
//! The on-screen layout (title bar, buffer, status line, two-line shortcut
//! bar) mirrors GNU nano's, with "tico" shown wherever nano would show its
//! own name.

use crate::app::{DiffOutcome, Editor, Mode, Prompt, PromptKind};
use crate::buffer::Pos;
use crate::keymap::{Action, Binding, Key as TKey, KeyMap, Menu};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{
    Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
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

/// Keeps a `watch::FileWatcher` pointed at whichever file the current
/// buffer has open, recreating it whenever that changes (buffer switch,
/// load, save-as, ...). When native watching isn't available for the
/// current path (unsupported platform, no path yet, ...), `watcher` is
/// `None` and the caller should keep doing its own periodic check.
struct DiskWatch {
    watcher: Option<crate::watch::FileWatcher>,
    path: Option<std::path::PathBuf>,
}

impl DiskWatch {
    fn new() -> DiskWatch {
        DiskWatch {
            watcher: None,
            path: None,
        }
    }

    fn sync(&mut self, current: Option<&std::path::Path>) {
        if self.path.as_deref() != current {
            self.path = current.map(|p| p.to_path_buf());
            self.watcher = current.and_then(crate::watch::FileWatcher::new);
        }
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

    let mut disk_watch = DiskWatch::new();

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
            let watched_path = if editor.buf().ignore_external_changes {
                None
            } else {
                editor.buf().path.as_deref()
            };
            disk_watch.sync(watched_path);
            // With a live native watcher, only bother running the (still
            // cheap, but not free) stat-based check when it actually
            // flagged something; without one (unsupported platform, or
            // setup failed for this path), fall back to the previous
            // behavior of checking on every idle poll.
            let should_check = match &disk_watch.watcher {
                Some(w) => w.take_changed(),
                None => true,
            };
            if should_check {
                dirty = maybe_check_external_change(editor);
            }
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
    maybe_warn_highlighting_disabled_for_size(editor);
    render(editor)?;
    if editor.bell_pending {
        editor.bell_pending = false;
        print!("\x07");
        io::stdout().flush()?;
    }
    Ok(())
}

/// One-time "syntax highlighting disabled: file too large" notice for the
/// current buffer, the first time it's found over
/// `options.max_syntax_highlight_bytes` (checked on every render, but the
/// check itself is just a length read, and `highlighting_size_warning_shown`
/// keeps it from repeating). Runs before render() so the notice shows up
/// in the very frame that would otherwise have silently skipped
/// highlighting -- including the first frame right after opening a large
/// file, or right after switching to one.
fn maybe_warn_highlighting_disabled_for_size(editor: &mut Editor) {
    if !editor.options.syntax_highlighting || editor.buf().language.is_none() {
        return;
    }
    let max = editor.options.max_syntax_highlight_bytes;
    if editor.buf().rope.len_bytes() as u64 <= max || editor.buf().highlighting_size_warning_shown {
        return;
    }
    editor.buf_mut().highlighting_size_warning_shown = true;
    editor.set_status_mild(format!(
        "Syntax highlighting disabled: file is larger than {}",
        format_byte_size(max)
    ));
}

/// Render a byte count the way it was most likely configured -- whichever
/// of B/KB/MB/GB divides it evenly (falling back to plain bytes), so a
/// `max_syntax_highlight_size = 4MB` setting is echoed back as "4MB", not
/// "4194304 bytes".
fn format_byte_size(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;
    if bytes != 0 && bytes.is_multiple_of(GB) {
        format!("{}GB", bytes / GB)
    } else if bytes != 0 && bytes.is_multiple_of(MB) {
        format!("{}MB", bytes / MB)
    } else if bytes != 0 && bytes.is_multiple_of(KB) {
        format!("{}KB", bytes / KB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Build the "file changed on disk, you have unsaved edits" choice prompt —
/// shared by the initial detection and by backing out of the merge-diff
/// viewer, so both offer the same [R]eload/[K]eep/[M]erge/[I]gnore choice.
/// No separate [C]ancel: it was always identical to Keep mine in effect, so
/// Esc still works as the usual escape hatch (see handle_conflict_choice)
/// without being advertised as its own, redundant option.
fn external_conflict_prompt() -> Prompt {
    Prompt {
        kind: PromptKind::ExternalChangeConflict,
        menu: Menu::YesNo,
        label: "File changed on disk and you have unsaved edits: [R]eload  [K]eep mine  [M]erge  [I]gnore All"
            .to_string(),
        input: String::new(),
        cursor: 0,
        history_pos: None,
        saved_input: None,
    }
}

/// Returns true if editor state changed (and so needs a redraw).
fn maybe_check_external_change(editor: &mut Editor) -> bool {
    use crate::fileio::ExternalChange;
    if editor.buf().ignore_external_changes {
        return false;
    }
    match crate::fileio::check_external_change(editor.buf()) {
        ExternalChange::Unchanged => return false,
        ExternalChange::ChangedNoLocalEdits => {
            let _ = crate::fileio::reload(editor.buf_mut());
            editor.set_status("File reloaded (changed on disk)");
        }
        ExternalChange::ChangedWithLocalEdits => {
            editor.mode = Mode::Prompt(external_conflict_prompt());
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
        Mode::Help {
            lines,
            top,
            return_to,
        } => {
            handle_help_key(editor, lines, top, return_to, key);
        }
        Mode::Diff {
            lines,
            top,
            outcome,
        } => {
            handle_diff_key(editor, lines, top, outcome, key);
        }
        Mode::Prompt(prompt) => handle_prompt_key(editor, prompt, key),
        Mode::Quit => editor.mode = Mode::Quit,
    }
}

/// Handle a keystroke while the `^G` help viewer is open: scroll its body,
/// or close it (via `^X`/`^C`/Esc) and return to whatever was active
/// before — the main editing window, or the prompt help was opened from.
fn handle_help_key(
    editor: &mut Editor,
    lines: Vec<String>,
    top: usize,
    return_to: Option<Box<Prompt>>,
    key: KeyEvent,
) {
    let body_len = lines.len().saturating_sub(1);
    let body_rows = help_body_rows(editor);
    let max_top = body_len.saturating_sub(body_rows);
    let mut top = top.min(max_top);
    let mut close = false;

    if matches!(key.code, KeyCode::Esc) {
        close = true;
    } else if let Some(tkey) = normalize_key(key)
        && let Some(Binding::Action(action)) =
            editor.keymap.lookup_menu_only(Menu::Help, tkey).cloned()
    {
        match action {
            Action::Cancel => close = true,
            Action::Up => top = top.saturating_sub(1),
            Action::Down => top = (top + 1).min(max_top),
            Action::PageUp => top = top.saturating_sub(body_rows),
            Action::PageDown => top = (top + body_rows).min(max_top),
            Action::FirstLine => top = 0,
            Action::LastLine => top = max_top,
            _ => {}
        }
    }

    editor.mode = if close {
        match return_to {
            Some(prompt) => Mode::Prompt(*prompt),
            None => Mode::Editing,
        }
    } else {
        Mode::Help {
            lines,
            top,
            return_to,
        }
    };
}

/// The movement actions Shift-selection applies to -- matches nano's
/// `wanted_to_move()`, the set of functions its own shift-held handling
/// treats as "just moving the cursor".
fn is_movement_action(action: Action) -> bool {
    matches!(
        action,
        Action::Left
            | Action::Right
            | Action::Up
            | Action::Down
            | Action::Home
            | Action::End
            | Action::PrevWord
            | Action::NextWord
            | Action::BeginPara
            | Action::EndPara
            | Action::PrevBlock
            | Action::NextBlock
            | Action::PageUp
            | Action::PageDown
            | Action::FirstLine
            | Action::LastLine
    )
}

fn handle_editing_key(editor: &mut Editor, key: KeyEvent) {
    // Shift-selection (nano's "soft mark"): crossterm reports Shift as a
    // modifier on the same key codes as plain movement (no separate
    // Shift+Left binding needed), so the resolved action is identical
    // either way -- only whether to also manage a mark around it differs.
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let tkey = normalize_key(key);
    let binding = tkey.and_then(|tk| editor.keymap.lookup(Menu::Main, tk).cloned());
    let is_movement = matches!(&binding, Some(Binding::Action(a)) if is_movement_action(*a));

    if shift && is_movement && editor.buf().mark.is_none() {
        let cur = editor.buf().cursor;
        editor.buf_mut().mark = Some(cur);
        editor.buf_mut().softmark = true;
    }
    let before = editor.buf().cursor;

    if let Some(binding) = binding {
        apply_binding(editor, binding);
    } else if let KeyCode::Char(c) = key.code
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        if editor.options.view {
            editor.set_status_mild("Key is invalid in view mode");
        } else {
            editor.insert_char(c);
            // Plain self-insertion bypasses execute(), which is what
            // normally keeps the cursor in view (vertically and, for a
            // long line, horizontally) after an action.
            editor.scroll_to_cursor();
        }
    }
    editor.maybe_update_lock_modified_flag();

    // Any plain (non-Shift) movement or edit drops a soft mark -- matches
    // nano's own post-dispatch check (a hard mark, set via `^^`/`M-A`,
    // isn't touched here at all).
    if !editor.buffers.is_empty()
        && !shift
        && editor.buf().softmark
        && editor.buf().mark.is_some()
        && (editor.buf().cursor != before || is_movement)
    {
        editor.buf_mut().mark = None;
        editor.buf_mut().softmark = false;
    }
}

fn apply_binding(editor: &mut Editor, binding: Binding) {
    match binding {
        // These need to run an external process (and, for the alt-speller
        // and formatter, hand the terminal over to it), which the
        // UI-agnostic `Editor::execute` can't do, so intercept them here
        // rather than dispatching through it (e.g. the default keymap's
        // `F12` for Speller, matching nano's own direct Main-menu binding).
        Binding::Action(Action::Speller) if editor.blocked_in_view_mode(Action::Speller) => {
            editor.set_status_mild("Key is invalid in view mode");
        }
        Binding::Action(Action::Formatter) if editor.blocked_in_view_mode(Action::Formatter) => {
            editor.set_status_mild("Key is invalid in view mode");
        }
        Binding::Action(Action::Speller) => run_speller(editor),
        Binding::Action(Action::Formatter) => run_formatter(editor),
        Binding::Action(Action::Linter) => run_linter(editor),
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
        PromptKind::ExternalChangeConflict => return handle_conflict_choice(editor, prompt, key),
        PromptKind::LockConflict { .. } => return handle_lock_conflict_choice(editor, prompt, key),
        PromptKind::ReplaceConfirm(_) => return handle_replace_confirm_choice(editor, prompt, key),
        PromptKind::Linter { .. } => return handle_linter_choice(editor, prompt, key),
        _ => {}
    }

    // Any keystroke other than Tab drops a filename-completion listing
    // that's currently shown (matches nano: typing something else clears
    // the "(more)" grid rather than leaving it stale on screen).
    if !matches!(normalize_key(key), Some(TKey::Ctrl('I'))) {
        editor.file_completions = None;
    }

    if let Some(tkey) = normalize_key(key) {
        if tkey == TKey::Ctrl('C') || matches!(key.code, KeyCode::Esc) {
            editor.mode = Mode::Editing;
            // Canceling out of a spell-fix prompt stops the word-by-word
            // loop, but (matching nano's `fix_spello`/`spell_check`) still
            // reports success rather than "Cancelled".
            if matches!(prompt.kind, PromptKind::SpellFix { .. }) {
                editor.set_status("Finished checking spelling");
            } else {
                editor.set_status("Cancelled");
            }
            return;
        }
        if tkey == TKey::Ctrl('M') {
            submit_prompt(editor, prompt);
            return;
        }
        if tkey == TKey::Ctrl('H') || tkey == TKey::Backspace {
            // In `--modernbindings`, Ctrl+H is Help for most prompt menus
            // (physical Backspace is unaffected — always deletes); check
            // the live keymap rather than hardcoding editing here always,
            // so this stays correct if that binding is customized further.
            if tkey == TKey::Ctrl('H')
                && editor.keymap.lookup_menu_only(prompt.menu, tkey)
                    == Some(&Binding::Action(Action::Help))
            {
                apply_prompt_action(editor, &mut prompt, Action::Help);
                return;
            }
            if prompt.cursor > 0 {
                let idx = prompt
                    .input
                    .char_indices()
                    .nth(prompt.cursor - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                prompt.input.remove(idx);
                prompt.cursor -= 1;
            }
            prompt.history_pos = None;
            prompt.saved_input = None;
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        // `Tab` at the `^R` Read File prompt (only in file-insert mode,
        // not Execute Command — matches nano's `MINSERTFILE` gate).
        if tkey == TKey::Ctrl('I')
            && let PromptKind::InsertFile { execute: false, .. } = prompt.kind
        {
            apply_filename_completion(editor, &mut prompt);
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
        if let Some(Binding::Action(action)) =
            editor.keymap.lookup_menu_only(prompt.menu, tkey).cloned()
        {
            if apply_prompt_action(editor, &mut prompt, action) {
                return;
            }
            editor.mode = Mode::Prompt(prompt);
            return;
        }
    }
    if let KeyCode::Char(c) = key.code
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        let idx = prompt
            .input
            .char_indices()
            .nth(prompt.cursor)
            .map(|(i, _)| i)
            .unwrap_or(prompt.input.len());
        prompt.input.insert(idx, c);
        prompt.cursor += 1;
        prompt.history_pos = None;
        prompt.saved_input = None;
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
/// here; anything else recognized by the keymap but not listed below is
/// ignored rather than silently doing the wrong thing.
fn apply_prompt_action(editor: &mut Editor, prompt: &mut Prompt, action: Action) -> bool {
    match action {
        // `^G` opens the help screen for whichever prompt is currently up
        // (Search and Replace get their own text — see help.rs); closing
        // it (via handle_help_key) returns here to the same prompt.
        Action::Help => {
            let lines = crate::help::build(prompt.menu, &editor.keymap, editor.screen_cols);
            editor.mode = Mode::Help {
                lines,
                top: 0,
                return_to: Some(Box::new(prompt.clone())),
            };
            true
        }
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
        // `M-F` at the Read File prompt: only the label wording changes
        // (confirmed against the installed nano — the shortcut-bar entry
        // itself always just reads "New Buffer", not a toggle-state pair
        // like FlipReplace's "Replace"/"No Replace").
        Action::FlipNewBuffer => {
            let PromptKind::InsertFile {
                new_buffer,
                execute,
            } = &mut prompt.kind
            else {
                return false;
            };
            *new_buffer = !*new_buffer;
            prompt.label = crate::app::insert_prompt_label(*new_buffer, *execute);
            false
        }
        // `^X` flips the Insert-File/Execute-Command prompt between its two
        // modes in place, keeping whatever was already typed (matches
        // nano's `flip_execute`, bound to the same key in both MINSERTFILE
        // and MEXECUTE).
        Action::FlipExecute => {
            let PromptKind::InsertFile {
                new_buffer,
                execute,
            } = &mut prompt.kind
            else {
                return false;
            };
            *execute = !*execute;
            prompt.menu = if *execute {
                Menu::Execute
            } else {
                Menu::Insert
            };
            prompt.label = crate::app::insert_prompt_label(*new_buffer, *execute);
            false
        }
        // Recognized (bound, shown in the shortcut bar and ^G help) but not
        // actually implemented yet: report that plainly rather than either
        // hiding the option or silently doing nothing when pressed. Status
        // messages don't show while a prompt is up (the status line is the
        // prompt itself), so this closes the prompt to make the message
        // visible, same as a real result would.
        Action::FlipConvert => {
            editor.mode = Mode::Editing;
            editor.set_status("No Conversion: not yet implemented");
            true
        }
        Action::Browser => {
            editor.mode = Mode::Editing;
            editor.set_status("File Browser: not yet implemented");
            true
        }
        // `^T`/`^Y`/`^O` from within the Insert-File/Execute-Command
        // prompt run the tool immediately, ignoring whatever was typed —
        // matches nano's `ran_a_tool` flag, which makes `insert_a_file_or`
        // break out of its loop (closing the prompt) as soon as one of
        // these fires.
        Action::Speller => {
            editor.mode = Mode::Editing;
            if editor.blocked_in_view_mode(Action::Speller) {
                editor.set_status_mild("Key is invalid in view mode");
            } else {
                run_speller(editor);
            }
            true
        }
        Action::Formatter => {
            editor.mode = Mode::Editing;
            if editor.blocked_in_view_mode(Action::Formatter) {
                editor.set_status_mild("Key is invalid in view mode");
            } else {
                run_formatter(editor);
            }
            true
        }
        Action::Linter => {
            editor.mode = Mode::Editing;
            run_linter(editor);
            true
        }
        // Bound (matching nano's full MEXECUTE menu, so the shortcut bar
        // and ^G help show them) but not actually implemented: same
        // plain-report convention as FlipConvert/Browser above.
        Action::FullJustify => {
            editor.mode = Mode::Editing;
            editor.set_status("Full Justify: not yet implemented");
            true
        }
        Action::CutRestOfFile => {
            editor.mode = Mode::Editing;
            editor.set_status("Cut Till End: not yet implemented");
            true
        }
        Action::FlipPipe => {
            editor.mode = Mode::Editing;
            editor.set_status("Pipe Text: not yet implemented");
            true
        }
        Action::Suspend => {
            editor.mode = Mode::Editing;
            editor.set_status("suspend: not supported in this build");
            true
        }
        _ => false,
    }
}

/// `Tab` at the `^R` Read File prompt: complete the typed fragment to the
/// longest common prefix among matching directory entries (nano's
/// `input_tab`/`filename_completion`), or, when the fragment starts with
/// `~` and contains no `/` yet, among system usernames instead (nano's
/// `username_completion`) — and list them all in `editor.file_completions`
/// when there's more than one, rendered as a grid in place of the buffer
/// (`render_completions_grid`).
fn apply_filename_completion(editor: &mut Editor, prompt: &mut Prompt) {
    // Matches nano: completion only applies at the end of the input.
    if prompt.cursor != prompt.input.chars().count() {
        return;
    }
    let morsel = prompt.input.clone();

    if morsel.starts_with('~') && !morsel.contains('/') {
        apply_username_completion(editor, prompt, &morsel);
        return;
    }

    let (dir_part, fragment) = match morsel.rfind('/') {
        Some(i) => (morsel[..=i].to_string(), morsel[i + 1..].to_string()),
        None => (String::new(), morsel.clone()),
    };
    // The directory is resolved with `~` expanded, but the completed text
    // keeps whatever the user actually typed (so `~/Doc<Tab>` completes to
    // `~/Documents/`, not the expanded home path).
    let expanded_dir = crate::fileio::expand_leading_tilde(&dir_part);
    let dir_path = if expanded_dir.is_empty() {
        std::path::PathBuf::from(".")
    } else {
        std::path::PathBuf::from(&expanded_dir)
    };
    let Ok(entries) = std::fs::read_dir(&dir_path) else {
        return;
    };
    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(&fragment))
        .collect();
    if matches.is_empty() {
        return;
    }
    matches.sort();

    let mut common = matches[0].clone();
    for m in &matches[1..] {
        common = common_prefix(&common, m);
    }

    let mut new_input = format!("{dir_part}{common}");
    if matches.len() == 1 && dir_path.join(&common).is_dir() {
        new_input.push('/');
    }
    if new_input != morsel {
        prompt.input = new_input;
        prompt.cursor = prompt.input.chars().count();
    }
    if matches.len() > 1 {
        editor.file_completions = Some(matches);
    }
}

/// `Tab` on a bare `~fragment` (no `/` yet): complete against system
/// usernames instead of filenames — matches nano's `username_completion`.
/// Unlike plain filename completion, a single match never gets a trailing
/// `/` appended (nano doesn't either: the completed `~name` isn't itself a
/// real path to check `is_dir` against), so finishing into that user's
/// home directory still takes one more keystroke plus a further Tab.
fn apply_username_completion(editor: &mut Editor, prompt: &mut Prompt, morsel: &str) {
    let matches = username_completion_matches(&crate::fileio::list_usernames(), &morsel[1..]);
    if matches.is_empty() {
        return;
    }

    let mut common = matches[0].clone();
    for m in &matches[1..] {
        common = common_prefix(&common, m);
    }
    if common != morsel {
        prompt.input = common;
        prompt.cursor = prompt.input.chars().count();
    }
    if matches.len() > 1 {
        editor.file_completions = Some(matches);
    }
}

/// The `~`-prefixed usernames (sorted) among `users` that start with
/// `fragment` — the pure matching logic behind `apply_username_completion`,
/// kept separate from `list_usernames()`'s real system lookup so it can be
/// tested against a fixed, portable list instead of the actual (and
/// environment-dependent) `/etc/passwd`.
fn username_completion_matches(users: &[String], fragment: &str) -> Vec<String> {
    let mut matches: Vec<String> = users
        .iter()
        .filter(|name| name.starts_with(fragment))
        .map(|name| format!("~{name}"))
        .collect();
    matches.sort();
    matches
}

/// The longest common leading substring of `a` and `b`.
fn common_prefix(a: &str, b: &str) -> String {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
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
    let suffix = if prompt.menu == Menu::Replace {
        " (to replace)"
    } else {
        ""
    };
    prompt.label = crate::app::search_prompt_label("Search", suffix, &editor.search);
}

fn handle_exit_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::Exit { .. } = &prompt.kind else {
        return;
    };
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

fn handle_conflict_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    // Get Help is normally ^G, but moves to ^H under `--modernbindings`
    // like every other prompt menu (see install_modern_overrides) — check
    // the live keymap rather than hardcoding ^G, so this stays correct
    // there and for any further nanorc/ticorc customization.
    if let Some(tkey) = normalize_key(key)
        && editor.keymap.lookup_menu_only(Menu::YesNo, tkey) == Some(&Binding::Action(Action::Help))
    {
        let lines = crate::help::build_conflict_help(editor.screen_cols);
        editor.mode = Mode::Help {
            lines,
            top: 0,
            return_to: Some(Box::new(prompt)),
        };
        return;
    }
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let _ = crate::fileio::reload(editor.buf_mut());
            editor.mode = Mode::Editing;
            editor.set_status("Reloaded from disk; local edits discarded");
        }
        // Esc is folded in here rather than removed outright: it and
        // [C]ancel used to be identical to Keep mine in every way but the
        // status message, so dropping Cancel as a separately-advertised
        // (redundant) choice still leaves Esc as the usual escape hatch.
        KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Esc => {
            if let Some(path) = editor.buf().path.clone() {
                editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
            }
            editor.mode = Mode::Editing;
            editor.set_status("Kept your local edits");
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            editor.begin_merge_preview();
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            editor.buf_mut().ignore_external_changes = true;
            if let Some(path) = editor.buf().path.clone() {
                editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
            }
            editor.mode = Mode::Editing;
            editor.set_status("Ignoring further on-disk changes to this file");
        }
        _ => editor.mode = Mode::Prompt(prompt),
    }
}

/// Handle a keystroke while the merge-diff viewer (`Mode::Diff`) is open:
/// scroll it, or act on it — Apply/Cancel for a clean-merge preview, any
/// key to dismiss an unmergeable-conflict preview (returning to the
/// reload/keep/cancel choice, same as before this became a full-screen
/// view).
fn handle_diff_key(
    editor: &mut Editor,
    lines: Vec<String>,
    top: usize,
    outcome: DiffOutcome,
    key: KeyEvent,
) {
    let body_len = lines.len().saturating_sub(1);
    let body_rows = help_body_rows(editor);
    let max_top = body_len.saturating_sub(body_rows);
    let mut top = top.min(max_top);

    match &outcome {
        DiffOutcome::Conflict => {
            // No automatic resolution possible; any key returns to the
            // main conflict choice so the user can pick reload/keep
            // instead — matches the prior prompt-based behavior.
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::ExternalChangeConflict,
                menu: Menu::YesNo,
                label: "Could not merge automatically: [R]eload  [K]eep mine  [I]gnore All"
                    .to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
            return;
        }
        DiffOutcome::ApplyMerge { merged_text } => match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let text = merged_text.clone();
                editor.buf_mut().rope = ropey::Rope::from_str(&text);
                editor.buf_mut().invalidate_highlight_cache();
                editor.buf_mut().modified = true;
                if let Some(path) = editor.buf().path.clone() {
                    editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
                }
                editor.mode = Mode::Editing;
                editor.set_status("Merged");
                return;
            }
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                // Back out to the reload/keep/merge/cancel choice, not
                // straight to editing — this is a step within resolving
                // the conflict, not a dismissal of it.
                editor.mode = Mode::Prompt(external_conflict_prompt());
                return;
            }
            KeyCode::Up => top = top.saturating_sub(1),
            KeyCode::Down => top = (top + 1).min(max_top),
            KeyCode::PageUp => top = top.saturating_sub(body_rows),
            KeyCode::PageDown => top = (top + body_rows).min(max_top),
            KeyCode::Home => top = 0,
            KeyCode::End => top = max_top,
            _ => {}
        },
    }
    editor.mode = Mode::Diff {
        lines,
        top,
        outcome,
    };
}

fn handle_lock_conflict_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::LockConflict { lock_path, target } = &prompt.kind else {
        return;
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let _ = crate::lockfile::write_lock(lock_path, target, false);
            editor.buf_mut().lock_filename = Some(lock_path.clone());
            editor.mode = Mode::Editing;
        }
        KeyCode::Char('n')
        | KeyCode::Char('N')
        | KeyCode::Char('c')
        | KeyCode::Char('C')
        | KeyCode::Esc => {
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
    let PromptKind::ReplaceConfirm(state) = prompt.kind else {
        return;
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            editor.replace_choice(state, crate::app::ReplaceChoice::Yes)
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            editor.replace_choice(state, crate::app::ReplaceChoice::No)
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            editor.replace_choice(state, crate::app::ReplaceChoice::All)
        }
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
            editor.replace_choice(state, crate::app::ReplaceChoice::Cancel)
        }
        _ => {
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(state),
                ..prompt
            });
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
            let text = if text.is_empty() {
                editor.search.last_pattern.clone().unwrap_or_default()
            } else {
                text
            };
            editor.mode = Mode::Editing;
            if text.is_empty() {
                return;
            }
            editor.history.add_search(&text);
            let backwards = editor.search.backwards;
            editor.run_search(&text, backwards);
        }
        PromptKind::Replace1 => {
            let text = if text.is_empty() {
                editor.search.last_pattern.clone().unwrap_or_default()
            } else {
                text
            };
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
                let target_line = if line < 0 {
                    (total + line).max(0)
                } else {
                    (line - 1).max(0)
                };
                let col = col_s.trim().parse::<i64>().unwrap_or(1).max(1) as usize - 1;
                editor.buf_mut().cursor = Pos::new(target_line as usize, col);
            }
        }
        PromptKind::InsertFile {
            new_buffer,
            execute,
        } => {
            editor.mode = Mode::Editing;
            if text.is_empty() {
                // Matches nano: an empty filename/command with New Buffer
                // on opens a blank buffer instead of canceling; off, it
                // cancels.
                if new_buffer {
                    editor.buffers.push(crate::buffer::Buffer::empty());
                    editor.current = editor.buffers.len() - 1;
                } else {
                    editor.set_status("Cancelled");
                }
                return;
            }
            if execute {
                submit_execute_command(editor, &text, new_buffer);
                return;
            }
            // `~`/`~/rest` expands to the current user's home directory;
            // `~user`/`~user/rest` to that user's (matching nano exactly —
            // see expand_leading_tilde).
            let path = std::path::PathBuf::from(crate::fileio::expand_leading_tilde(&text));
            if path.is_dir() {
                editor.set_status_alert(format!("'{}' is a directory", path.display()));
                return;
            }
            if new_buffer {
                let syntax_override = editor.options.syntax_name.clone();
                if !path.exists() {
                    // A nonexistent filename also yields a blank buffer,
                    // per nano's own hint text for this prompt.
                    let mut buf = crate::buffer::Buffer::from_text("", Some(path));
                    // Detected the same way as a file given on the command
                    // line (extension -> shebang -> modeline, or -Y/--syntax)
                    // — a buffer opened via ^R shouldn't get plain text just
                    // because it didn't come from argv.
                    buf.language = crate::syntax::detect_with_override(
                        buf.path.as_deref(),
                        &buf.to_string(),
                        syntax_override.as_deref(),
                    );
                    editor.buffers.push(buf);
                    editor.current = editor.buffers.len() - 1;
                    editor.set_status("New File");
                } else {
                    match crate::fileio::load_file(&path) {
                        Ok(mut buf) => {
                            let msg = crate::fileio::describe_read(&buf.to_string());
                            buf.language = crate::syntax::detect_with_override(
                                buf.path.as_deref(),
                                &buf.to_string(),
                                syntax_override.as_deref(),
                            );
                            editor.buffers.push(buf);
                            editor.current = editor.buffers.len() - 1;
                            editor.set_status(msg);
                        }
                        Err(e) => editor
                            .set_status_alert(format!("Error reading {}: {e}", path.display())),
                    }
                }
            } else {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let msg = crate::fileio::describe_read(&content);
                        editor.buf_mut().insert_str(&content);
                        editor.set_status(msg);
                    }
                    Err(e) => {
                        editor.set_status_alert(format!("Error reading {}: {e}", path.display()))
                    }
                }
            }
        }
        PromptKind::SpellFix { word, remaining } => {
            editor.mode = Mode::Editing;
            if text != word && !text.is_empty() {
                replace_whole_word(editor.buf_mut(), &word, &text);
                editor.buf_mut().modified = true;
            }
            advance_spell_fix(editor, remaining);
        }
        PromptKind::WriteOut { exiting } => {
            editor.mode = Mode::Editing;
            let path = std::path::PathBuf::from(text);
            // A marked region (outside of the exit-time save prompt)
            // writes just the selection to `path` as a standalone file --
            // it doesn't touch the current buffer's own path/modified/
            // disk-state, matching nano's write_region_to_file, and
            // doesn't clear the mark (confirmed against the installed
            // nano: the selection stays highlighted afterward).
            if !exiting && let Some((start, end)) = editor.selection_range() {
                let selected = editor.buf().text_range(start, end);
                match std::fs::write(&path, &selected) {
                    Ok(()) => editor.set_status(format!("Wrote {}", path.display())),
                    Err(e) => editor.set_status(format!("Error writing file: {e}")),
                }
                return;
            }
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
// Execute Command / Speller / Formatter / Linter
// ---------------------------------------------------------------------

/// A whole-word (alphanumeric-or-underscore-bounded) match, case-sensitive.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The position of the first whole-word occurrence of `word` in `buf`,
/// scanning from the top — used to seed the spell-fix loop's spotlight,
/// matching nano's own `fix_spello` (which likewise searches for the exact,
/// case-sensitive word).
fn find_whole_word(buf: &crate::buffer::Buffer, word: &str) -> Option<Pos> {
    let wchars = word.chars().count();
    if wchars == 0 {
        return None;
    }
    for line_idx in 0..buf.line_count() {
        let chars: Vec<char> = buf.line(line_idx).chars().collect();
        let mut col = 0;
        while col + wchars <= chars.len() {
            if chars[col..col + wchars].iter().collect::<String>() == word {
                let before_ok = col == 0 || !is_word_char(chars[col - 1]);
                let after_ok = col + wchars == chars.len() || !is_word_char(chars[col + wchars]);
                if before_ok && after_ok {
                    return Some(Pos::new(line_idx, col));
                }
            }
            col += 1;
        }
    }
    None
}

/// Replace every whole-word, case-sensitive occurrence of `word` in `buf`
/// with `replacement` — matches nano's `fix_spello`, which (via
/// `do_replace_loop`) fixes every instance of a misspelling at once rather
/// than asking per-occurrence. Returns whether anything changed.
fn replace_whole_word(buf: &mut crate::buffer::Buffer, word: &str, replacement: &str) -> bool {
    let wchars = word.chars().count();
    if wchars == 0 {
        return false;
    }
    let mut matches: Vec<Pos> = Vec::new();
    for line_idx in 0..buf.line_count() {
        let chars: Vec<char> = buf.line(line_idx).chars().collect();
        let mut col = 0;
        while col + wchars <= chars.len() {
            if chars[col..col + wchars].iter().collect::<String>() == word {
                let before_ok = col == 0 || !is_word_char(chars[col - 1]);
                let after_ok = col + wchars == chars.len() || !is_word_char(chars[col + wchars]);
                if before_ok && after_ok {
                    matches.push(Pos::new(line_idx, col));
                    col += wchars;
                    continue;
                }
            }
            col += 1;
        }
    }
    if matches.is_empty() {
        return false;
    }
    for start in matches.into_iter().rev() {
        let end = Pos::new(start.line, start.col + wchars);
        buf.delete_range(start, end);
        buf.cursor = start;
        buf.insert_str(replacement);
    }
    true
}

/// Split a configured command (`set speller`/`--speller`, or a syntax's
/// built-in `linter`/`formatter`) into a program and its arguments on
/// whitespace — matching nano's own `construct_argument_list`, which uses
/// `strtok(..., " ")` and likewise has no quoting support.
fn split_command(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

/// Write `text` to a fresh temp file, for handing to an external
/// speller/formatter (matches nano's `safe_tempfile`).
fn write_temp_file(text: &str) -> io::Result<std::path::PathBuf> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("tico.{}.{unique}.tmp", std::process::id()));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Hand the terminal over to `cmd` for the duration of its run — leaving
/// the alternate screen and raw mode exactly like nano's `endwin()` before
/// `treat()`/spawning a program, then restoring both (and forcing a full
/// repaint, since the program may have written anything to the screen)
/// once it exits.
fn run_suspended(mut cmd: std::process::Command) -> io::Result<std::process::ExitStatus> {
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    let result = cmd.status();
    let _ = enable_raw_mode();
    let _ = execute!(
        io::stdout(),
        EnterAlternateScreen,
        Hide,
        Clear(ClearType::All)
    );
    result
}

/// `^T` Execute Command's submit: run `text` in the shell, and insert its
/// captured output into the buffer at the cursor (or, with New Buffer on,
/// into a fresh blank buffer) — matches nano's `execute_command` (the
/// plain, non-pipe case; nano's `|command` pipe-to-stdin form isn't
/// implemented).
fn submit_execute_command(editor: &mut Editor, command: &str, new_buffer: bool) {
    if new_buffer {
        editor.buffers.push(crate::buffer::Buffer::empty());
        editor.current = editor.buffers.len() - 1;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    editor.set_status("Executing...");
    match std::process::Command::new(&shell)
        .arg("-c")
        .arg(command)
        .output()
    {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            if !text.is_empty() {
                editor.buf_mut().insert_str(&text);
                editor.buf_mut().modified = true;
            }
            editor.history.add_execute(command);
            editor.set_status("Executing...");
        }
        Err(e) => editor.set_status_alert(format!("Could not fork: {e}")),
    }
}

/// `F12` (Main menu) / `^T` from within the Execute-Command prompt: spell
/// check the current buffer (or, if a region is marked, just that region).
fn run_speller(editor: &mut Editor) {
    match editor.options.speller.clone() {
        Some(cmd) if !cmd.is_empty() => run_alt_speller(editor, &cmd),
        _ => run_internal_speller(editor),
    }
}

/// The configured `set speller`/`--speller` program: an interactive tool
/// (aspell -c, ispell, ...) that edits a temp copy of the text directly,
/// matching nano's `treat()` — the terminal is handed over to it, and the
/// buffer is replaced with the temp file's contents if it changed.
fn run_alt_speller(editor: &mut Editor, speller_cmd: &str) {
    let text = editor.tool_input_text();
    let tmp = match write_temp_file(&text) {
        Ok(p) => p,
        Err(e) => {
            editor.set_status_alert(format!("Error writing temp file: {e}"));
            return;
        }
    };
    let before = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
    let mut argv = split_command(speller_cmd);
    if argv.is_empty() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let program = argv.remove(0);
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&argv).arg(&tmp);
    match run_suspended(cmd) {
        Ok(status) => {
            let code = status.code();
            if !code.is_some_and(|c| (0..=2).contains(&c)) {
                editor.set_status_alert(format!("Error invoking '{speller_cmd}'"));
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            // Exit code 1 or 2 means the program is unhappy about
            // something; nano shows that ALERT-level complaint and, per
            // its status-bar importance rule (a lower-priority message
            // never overwrites a still-showing higher one), leaves it up
            // instead of replacing it with the routine "Nothing
            // changed"/"Finished..." message that follows.
            let complained = code != Some(0);
            if complained {
                editor.set_status_alert(format!("Program '{speller_cmd}' complained"));
            }
            let after = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
            if after != before {
                match std::fs::read_to_string(&tmp) {
                    Ok(new_text) => {
                        editor.replace_tool_input(&new_text);
                        if !complained {
                            editor.set_status("Finished checking spelling");
                        }
                    }
                    Err(e) => editor.set_status_alert(format!("Error reading temp file: {e}")),
                }
            } else if !complained {
                editor.set_status("Nothing changed");
            }
        }
        Err(e) => editor.set_status_alert(format!("Error invoking '{speller_cmd}': {e}")),
    }
    let _ = std::fs::remove_file(&tmp);
}

/// No `set speller`/`--speller` configured: nano's own default — run
/// `hunspell -l` (falling back to `spell`) over the text, sort and dedupe
/// the misspelled words it lists (`sort -f | uniq`), then offer each one
/// in turn via the `SpellFix` prompt (see `advance_spell_fix`).
fn run_internal_speller(editor: &mut Editor) {
    let text = editor.tool_input_text();
    let words = match run_word_lister("hunspell", &["-l"], &text)
        .or_else(|| run_word_lister("spell", &[], &text))
    {
        Some(w) => w,
        None => {
            editor.set_status_alert("Error invoking spell checker");
            return;
        }
    };
    let mut sorted = words;
    sorted.sort_by_key(|w| w.to_lowercase());
    let mut deduped: Vec<String> = Vec::with_capacity(sorted.len());
    for w in sorted {
        if deduped.last() != Some(&w) {
            deduped.push(w);
        }
    }
    advance_spell_fix(editor, deduped);
}

/// Run `program args...` with `text` piped to its stdin, returning its
/// stdout split into non-blank lines — `None` if the program couldn't be
/// spawned (e.g. not installed), so the caller can fall back to the next
/// one in nano's own preference order.
fn run_word_lister(program: &str, args: &[&str], text: &str) -> Option<Vec<String>> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Advance the internal spell-fix loop: pop words off the front of
/// `remaining` until one is actually found in the buffer (a speller can
/// report a word that doesn't literally occur, e.g. across a line
/// boundary), open the `SpellFix` prompt spotlighting it, or, once the
/// list is exhausted, report done — matches nano's `spell_check`.
fn advance_spell_fix(editor: &mut Editor, mut remaining: Vec<String>) {
    while !remaining.is_empty() {
        let word = remaining.remove(0);
        if let Some(pos) = find_whole_word(editor.buf(), &word) {
            editor.buf_mut().cursor = pos;
            editor.scroll_to_cursor();
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::SpellFix {
                    word: word.clone(),
                    remaining,
                },
                menu: Menu::Spell,
                label: "Edit a replacement".to_string(),
                cursor: word.chars().count(),
                input: word,
                history_pos: None,
                saved_input: None,
            });
            return;
        }
    }
    editor.mode = Mode::Editing;
    editor.set_status("Finished checking spelling");
}

/// `^O` from the Execute-Command prompt (or a rebound Main-menu key): run
/// the current buffer's configured formatter (a per-language default, e.g.
/// `gofmt -w`, matching nano's shipped nanorc `formatter` directives).
/// Same terminal-handoff/temp-file contract as `run_alt_speller` (nano
/// implements both through the shared `treat()`).
fn run_formatter(editor: &mut Editor) {
    let Some(formatter_cmd) = editor.buf().language.and_then(|l| l.formatter) else {
        editor.set_status_mild("No formatter is defined for this type of file");
        return;
    };
    let text = editor.tool_input_text();
    let tmp = match write_temp_file(&text) {
        Ok(p) => p,
        Err(e) => {
            editor.set_status_alert(format!("Error writing temp file: {e}"));
            return;
        }
    };
    let before = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
    let mut argv = split_command(formatter_cmd);
    if argv.is_empty() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let program = argv.remove(0);
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&argv).arg(&tmp);
    match run_suspended(cmd) {
        Ok(status) => {
            let code = status.code();
            if !code.is_some_and(|c| (0..=2).contains(&c)) {
                editor.set_status_alert(format!("Error invoking '{formatter_cmd}'"));
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            // See run_alt_speller: a "complained" ALERT outranks the
            // routine messages that follow, so it stays up instead of
            // being overwritten by them.
            let complained = code != Some(0);
            if complained {
                editor.set_status_alert(format!("Program '{formatter_cmd}' complained"));
            }
            let after = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
            if after != before {
                match std::fs::read_to_string(&tmp) {
                    Ok(new_text) => {
                        editor.replace_tool_input(&new_text);
                        if !complained {
                            editor.set_status("Buffer has been processed");
                        }
                    }
                    Err(e) => editor.set_status_alert(format!("Error reading temp file: {e}")),
                }
            } else if !complained {
                editor.set_status("Nothing changed");
            }
        }
        Err(e) => editor.set_status_alert(format!("Error invoking '{formatter_cmd}': {e}")),
    }
    let _ = std::fs::remove_file(&tmp);
}

/// `^Y` from the Execute-Command prompt (or a rebound Main-menu key): run
/// the current buffer's configured linter and open the interactive result
/// viewer (`PromptKind::Linter`) on whatever it reports — matches nano's
/// `do_linter`, without the "jump to a different open buffer" case (tico's
/// linter only ever targets the current buffer's own file).
fn run_linter(editor: &mut Editor) {
    let Some(linter_cmd) = editor.buf().language.and_then(|l| l.linter) else {
        editor.set_status_mild("No linter is defined for this type of file");
        return;
    };
    let Some(path) = editor.buf().path.clone() else {
        editor.set_status_mild("No linter is defined for this type of file");
        return;
    };
    let mut argv = split_command(linter_cmd);
    if argv.is_empty() {
        return;
    }
    let program = argv.remove(0);
    editor.set_status("Invoking linter...");
    let output = std::process::Command::new(&program)
        .args(&argv)
        .arg(&path)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            editor.set_status_alert(format!("Error invoking '{linter_cmd}': {e}"));
            return;
        }
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let messages = parse_linter_output(&combined);
    if messages.is_empty() {
        editor.set_status(format!("Got 0 parsable lines from command: {linter_cmd}"));
        return;
    }
    goto_lint_message(editor, &messages[0]);
    editor.mode = Mode::Prompt(Prompt {
        kind: PromptKind::Linter { messages, index: 0 },
        menu: Menu::Linter,
        label: String::new(),
        input: String::new(),
        cursor: 0,
        history_pos: None,
        saved_input: None,
    });
    if let Mode::Prompt(prompt) = &mut editor.mode
        && let PromptKind::Linter { messages, index } = &prompt.kind
    {
        prompt.label = messages[*index].msg.clone();
    }
}

/// Parse `filename:line:col: message` (or `filename:line,col: message`, or
/// bare `filename:line: message` with the column defaulting to 1) lines —
/// matches nano's own linter-output parser in `do_linter`.
/// Parse a leading (optionally signed) decimal integer, ignoring any
/// trailing non-digit text — matches C's `strtol(s, NULL, 10)`, which
/// nano's linter parser relies on to tolerate e.g. `12,2` as a line number
/// (reading `12` and leaving the rest for a separate comma-split).
fn parse_leading_int(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let neg = s.starts_with('-');
    let digits_start = if neg || s.starts_with('+') { 1 } else { 0 };
    let digit_len = s[digits_start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .count();
    if digit_len == 0 {
        return None;
    }
    s[..digits_start + digit_len].parse().ok()
}

fn parse_linter_output(output: &str) -> Vec<crate::app::LintMessage> {
    let mut messages = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        // The message is everything after the first space anywhere in the
        // line, independent of how the fields before it are split — matches
        // nano's `spacer = strstr(complaint, " ")`.
        let Some(spacer) = line.find(' ') else {
            continue;
        };
        let Some((filename, after_filename)) = line.split_once(':') else {
            continue;
        };
        let Some((linestring, after_line)) = after_filename.split_once(':') else {
            continue;
        };
        let Some(lineno) = parse_leading_int(linestring).filter(|&n| n > 0) else {
            continue;
        };
        // `strtok(NULL, " ")` on the remainder: skips any leading spaces,
        // then reads up to the next one.
        let colstring = after_line.trim_start_matches(' ').split(' ').next();
        let mut colno = colstring
            .and_then(parse_leading_int)
            .filter(|&c| c > 0)
            .unwrap_or(0);
        if colno <= 0 {
            colno = 1;
            // "line,column" form: the part after a comma in `linestring`.
            if let Some((_, colpart)) = linestring.split_once(',')
                && let Some(c) = parse_leading_int(colpart)
            {
                colno = c;
            }
        }
        messages.push(crate::app::LintMessage {
            filename: filename.to_string(),
            line: lineno as usize,
            col: colno as usize,
            msg: line[spacer + 1..].to_string(),
        });
    }
    messages
}

/// Move the cursor to a lint message's reported location, matching nano's
/// `goto_line_posx` + `adjust_viewport(CENTERING)` in `do_linter`.
fn goto_lint_message(editor: &mut Editor, msg: &crate::app::LintMessage) {
    let line = msg
        .line
        .saturating_sub(1)
        .min(editor.buf().line_count().saturating_sub(1));
    let col = msg
        .col
        .saturating_sub(1)
        .min(editor.buf().line(line).chars().count());
    editor.buf_mut().cursor = Pos::new(line, col);
    editor.scroll_to_cursor();
}

/// Step through linter results with PageUp/PageDown ("Previous/Next Linter
/// message"), or close the viewer with Cancel/Enter — matches nano's
/// `MLINTER` navigation loop in `do_linter`.
fn handle_linter_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::Linter {
        messages,
        mut index,
    } = prompt.kind
    else {
        unreachable!()
    };
    if matches!(key.code, KeyCode::Esc) {
        editor.mode = Mode::Editing;
        editor.status = None;
        return;
    }
    let Some(tkey) = normalize_key(key) else {
        editor.mode = Mode::Prompt(Prompt {
            kind: PromptKind::Linter { messages, index },
            ..prompt
        });
        return;
    };
    // "At first/last message" briefly replaces the current message when a
    // boundary is hit (nano flashes it for ~600ms then restores the
    // message; tico's synchronous input loop has no timed flash, so it
    // just shows until the next keystroke instead).
    let mut boundary_label = None;
    match editor.keymap.lookup_menu_only(Menu::Linter, tkey) {
        Some(Binding::Action(Action::Cancel)) => {
            editor.mode = Mode::Editing;
            editor.status = None;
            return;
        }
        Some(Binding::Action(Action::PageUp)) => {
            if index > 0 {
                index -= 1;
            } else {
                boundary_label = Some("At first message");
                editor.bell_pending = true;
            }
        }
        Some(Binding::Action(Action::PageDown)) => {
            if index + 1 < messages.len() {
                index += 1;
            } else {
                boundary_label = Some("At last message");
                editor.bell_pending = true;
            }
        }
        _ => {}
    }
    goto_lint_message(editor, &messages[index]);
    let label = boundary_label
        .map(str::to_string)
        .unwrap_or_else(|| messages[index].msg.clone());
    editor.mode = Mode::Prompt(Prompt {
        kind: PromptKind::Linter { messages, index },
        menu: Menu::Linter,
        label,
        input: String::new(),
        cursor: 0,
        history_pos: None,
        saved_input: None,
    });
}

// ---------------------------------------------------------------------
// Crossterm key normalization
// ---------------------------------------------------------------------

fn normalize_key(key: KeyEvent) -> Option<TKey> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        KeyCode::Backspace => Some(TKey::Backspace),
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
        KeyCode::Char(c) if alt && shift && c.is_ascii_alphabetic() => {
            Some(TKey::ShiftMeta(c.to_ascii_uppercase()))
        }
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

    if let Mode::Help { lines, top, .. } = &editor.mode {
        render_help_screen(editor, &mut out, lines, *top)?;
        return out.flush();
    }
    if let Mode::Diff {
        lines,
        top,
        outcome,
    } = &editor.mode
    {
        render_diff_screen(editor, &mut out, lines, *top, outcome)?;
        return out.flush();
    }

    let cols = editor.screen_cols;
    let rows = editor.screen_rows;
    if editor.options.zero {
        render_buffer(editor, &mut out, 0, rows)?;
        finish_cursor(editor, &mut out, 0)?;
        return out.flush();
    }

    queue!(out, MoveTo(0, 0))?;
    render_title_bar(editor, &mut out, cols)?;

    let help_rows = if editor.options.nohelp { 0 } else { 2 };
    let text_start_row = 1u16;
    let text_rows = rows.saturating_sub(2 + help_rows);
    if let Some(matches) = &editor.file_completions {
        render_completions_grid(&mut out, text_start_row, text_rows, cols, matches)?;
    } else {
        render_buffer(editor, &mut out, text_start_row, text_rows)?;
    }

    let status_row = text_start_row + text_rows as u16;
    render_status_line(editor, &mut out, status_row, cols)?;

    if help_rows > 0 {
        let prompt = if let Mode::Prompt(p) = &editor.mode {
            Some(p)
        } else {
            None
        };
        let entries = shortcut_bar_entries(&editor.keymap, prompt);
        render_shortcut_bar(&mut out, status_row + 1, cols, &entries)?;
    }

    finish_cursor(editor, &mut out, text_start_row)?;
    out.flush()
}

/// Number of rows available for the help viewer's scrollable body: the
/// whole screen minus the title row and the (always-shown, regardless of
/// `nohelp`) two-line shortcut bar.
fn help_body_rows(editor: &Editor) -> usize {
    editor.screen_rows.saturating_sub(3).max(1)
}

/// The `^G` help viewer takes over the whole screen: a centered,
/// reverse-video title (`lines[0]`) where the title bar would normally be,
/// the scrollable body starting at `top` (an index into `lines[1..]`), and
/// its own shortcut bar in place of the usual status line + shortcuts.
fn render_help_screen(
    editor: &Editor,
    out: &mut impl Write,
    lines: &[String],
    top: usize,
) -> io::Result<()> {
    let cols = editor.screen_cols;
    let rows = editor.screen_rows;

    render_centered_title_row(
        out,
        cols,
        lines.first().map(|s| s.as_str()).unwrap_or("Help"),
    )?;
    render_scrollable_body(
        out,
        cols,
        &lines[1.min(lines.len())..],
        top,
        help_body_rows(editor),
        None,
        syntax_color,
    )?;
    let entries = resolve_shortcuts(&editor.keymap, Menu::Help, HELP_SHORTCUTS);
    render_shortcut_bar(out, rows.saturating_sub(2) as u16, cols, &entries)
}

/// The merge-diff viewer (`Mode::Diff`): same full-screen layout as the
/// help viewer (title row, scrollable body, bottom bar), but the bottom
/// bar offers Apply/Cancel for a clean-merge preview instead of just
/// closing, since dismissing this screen is itself a decision.
fn render_diff_screen(
    editor: &Editor,
    out: &mut impl Write,
    lines: &[String],
    top: usize,
    outcome: &DiffOutcome,
) -> io::Result<()> {
    let cols = editor.screen_cols;
    let rows = editor.screen_rows;

    render_centered_title_row(
        out,
        cols,
        lines.first().map(|s| s.as_str()).unwrap_or("Diff"),
    )?;
    let body = &lines[1.min(lines.len())..];
    let kinds = if editor.options.syntax_highlighting {
        diff_line_kinds(body)
    } else {
        None
    };
    render_scrollable_body(
        out,
        cols,
        body,
        top,
        help_body_rows(editor),
        kinds.as_deref(),
        diff_color,
    )?;

    let shortcuts: &[(&str, &str)] = match outcome {
        DiffOutcome::ApplyMerge { .. } => &[
            ("A", "Apply merge"),
            ("C", "Cancel"),
            ("^P", "Prev Line"),
            ("^N", "Next Line"),
            ("^Y", "Prev Page"),
            ("^V", "Next Page"),
        ],
        DiffOutcome::Conflict => &[("(any key)", "Continue")],
    };
    let entries: Vec<(String, &str)> = shortcuts.iter().map(|&(k, d)| (k.to_string(), d)).collect();
    render_shortcut_bar(out, rows.saturating_sub(2) as u16, cols, &entries)
}

/// Center `title` on its own reverse-video row at the top of the screen —
/// shared by the help and merge-diff full-screen viewers.
fn render_centered_title_row(out: &mut impl Write, cols: usize, title: &str) -> io::Result<()> {
    queue!(out, MoveTo(0, 0))?;
    let mut title_row = vec![' '; cols];
    let start = cols.saturating_sub(title.chars().count()) / 2;
    for (i, c) in title.chars().enumerate() {
        if start + i < cols {
            title_row[start + i] = c;
        }
    }
    let title_line: String = title_row.into_iter().collect();
    queue!(
        out,
        SetAttribute(Attribute::Reverse),
        Print(title_line),
        SetAttribute(Attribute::Reset)
    )
}

/// Draw `body_rows` rows of `body` starting at `top`, one screen row per
/// line, below the title row — shared by the help and merge-diff viewers.
fn render_scrollable_body(
    out: &mut impl Write,
    cols: usize,
    body: &[String],
    top: usize,
    body_rows: usize,
    kinds: Option<&[Vec<Option<crate::syntax::HighlightKind>>]>,
    color: fn(crate::syntax::HighlightKind) -> Color,
) -> io::Result<()> {
    for r in 0..body_rows {
        queue!(out, MoveTo(0, 1 + r as u16))?;
        let idx = top + r;
        let text = body.get(idx).map(|s| s.as_str()).unwrap_or("");
        let chars: Vec<char> = text.chars().take(cols).collect();
        let line_kinds = kinds.and_then(|k| k.get(idx));
        let len = chars.len();

        if let Some(line_kinds) = line_kinds.filter(|k| k.iter().any(Option::is_some)) {
            let mut i = 0;
            while i < len {
                let kind = line_kinds.get(i).copied().flatten();
                let mut j = i + 1;
                while j < len && line_kinds.get(j).copied().flatten() == kind {
                    j += 1;
                }
                let segment: String = chars[i..j].iter().collect();
                if let Some(kind) = kind {
                    queue!(
                        out,
                        SetForegroundColor(color(kind)),
                        Print(segment),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else {
                    queue!(out, Print(segment))?;
                }
                i = j;
            }
            if len < cols {
                queue!(out, Print(" ".repeat(cols - len)))?;
            }
        } else {
            let s: String = chars.into_iter().collect();
            queue!(out, Print(format!("{s:<cols$}", cols = cols)))?;
        }
    }
    Ok(())
}

fn finish_cursor(editor: &Editor, out: &mut impl Write, text_start_row: u16) -> io::Result<()> {
    if let Mode::Prompt(prompt) = &editor.mode {
        let row = editor
            .screen_rows
            .saturating_sub(if editor.options.nohelp { 1 } else { 3 });
        let col = prompt.label.chars().count() + 2 + prompt.cursor;
        queue!(
            out,
            MoveTo(
                col.min(editor.screen_cols.saturating_sub(1)) as u16,
                row as u16
            ),
            Show
        )?;
    } else {
        let buf = editor.buf();
        let screen_line = buf.cursor.line.saturating_sub(buf.top_line);
        let gutter = editor.gutter_width();
        let cursor_col = crate::buffer::display_width(
            &buf.line(buf.cursor.line),
            buf.cursor.col,
            editor.options.tabsize as usize,
        );
        // `left_col` is only ever nonzero for the cursor's own line (see
        // `render_buffer`), and a `<` marker takes up one column whenever
        // it's scrolled, shifting everything after it right by one.
        let show_left = buf.left_col > 0;
        let col = gutter + if show_left { 1 } else { 0 } + cursor_col.saturating_sub(buf.left_col);
        queue!(
            out,
            MoveTo(
                col.min(editor.screen_cols.saturating_sub(1)) as u16,
                (text_start_row as usize + screen_line) as u16
            ),
            Show
        )?;
    }
    Ok(())
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

    // When more than one buffer is open, show "[i/n]" (1-based current
    // buffer index / total buffer count) in the upper-right corner. Nano
    // shows this same indicator but replaces its version text with it; we
    // keep the "tico version" text on the left and add the indicator on
    // the right instead so neither is lost. `--view`'s "View" (nano's own
    // right-aligned "state" word) shares that same corner; the two are
    // independent, so both can show together.
    let mut indicator = String::new();
    if editor.options.view {
        indicator.push_str("View");
    }
    if editor.buffers.len() > 1 {
        if !indicator.is_empty() {
            indicator.push(' ');
        }
        indicator.push_str(&format!(
            "[{}/{}]",
            editor.current + 1,
            editor.buffers.len()
        ));
    }
    let right_w = if indicator.is_empty() {
        0
    } else {
        indicator.chars().count() + 2
    };

    // Reserve space for the left prefix and the right indicator (plus one
    // column of separation on each side); if the filename doesn't fit,
    // truncate it, keeping the tail (the most identifying part of a long
    // path) and prefixing "...".
    let left_w = left.chars().count();
    let available = cols.saturating_sub(left_w + right_w + 2);
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
    if !indicator.is_empty() {
        let ind_start = cols.saturating_sub(indicator.chars().count() + 2);
        for (i, c) in indicator.chars().enumerate() {
            let pos = ind_start + i;
            if pos < cols {
                line[pos] = c;
            }
        }
    }
    let s: String = line.into_iter().collect();
    queue!(
        out,
        SetAttribute(Attribute::Reverse),
        Print(s),
        SetAttribute(Attribute::Reset)
    )
}

fn render_status_line(
    editor: &Editor,
    out: &mut impl Write,
    row: u16,
    cols: usize,
) -> io::Result<()> {
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
        queue!(
            out,
            SetAttribute(Attribute::Reverse),
            Print(s),
            SetAttribute(Attribute::Reset)
        )
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
                queue!(
                    out,
                    SetAttribute(Attribute::Reverse),
                    Print(shown),
                    SetAttribute(Attribute::Reset)
                )?;
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
/// as (action, description) pairs — resolved against the *live* keymap at
/// render time (via `key_label_for`) rather than baking in a fixed key
/// label, so a `bind`/`unbind` in nanorc/ticorc, or `--modernbindings`,
/// shows up here immediately instead of leaving the bar showing stale
/// defaults.
const SHORTCUT_PRIORITY: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Exit, "Exit"),
    (Action::WriteOut, "Write Out"),
    (Action::Insert, "Read File"),
    (Action::WhereIs, "Where Is"),
    (Action::Replace, "Replace"),
    (Action::Cut, "Cut"),
    (Action::Paste, "Paste"),
    (Action::Execute, "Execute"),
    (Action::Justify, "Justify"),
    (Action::Location, "Location"),
    (Action::GotoLine, "Go To Line"),
    (Action::Undo, "Undo"),
    (Action::Redo, "Redo"),
    (Action::Mark, "Set Mark"),
    (Action::Copy, "Copy"),
    (Action::FindBracket, "To Bracket"),
    (Action::WhereWas, "Where Was"),
    (Action::FindPrevious, "Previous"),
    (Action::FindNext, "Next"),
];

/// The Search (WhereIs) prompt's shortcut list, captured the same way.
const SEARCH_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::CaseSens, "Case Sens"),
    (Action::Regexp, "Reg.exp."),
    (Action::Backwards, "Backwards"),
    (Action::FlipReplace, "Replace"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
    (Action::FlipGoto, "Go To Line"),
];

/// The "Search (to replace)" prompt: same as Search but without ^T
/// (MREPLACE isn't bound to flip_goto in nano) and ^R now offers to flip
/// *back* to plain search.
const REPLACE1_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::CaseSens, "Case Sens"),
    (Action::Regexp, "Reg.exp."),
    (Action::Backwards, "Backwards"),
    (Action::FlipReplace, "No Replace"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
];

const REPLACEWITH_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
];

const GOTOLINE_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::BeginPara, "Begin of Paragr."),
    (Action::EndPara, "End of Paragraph"),
    (Action::FirstLine, "First Line"),
    (Action::LastLine, "Last Line"),
    (Action::FlipGoto, "Go To Text"),
];

/// The `^R` Read File prompt's shortcut list, matching nano's full menu.
/// No-conversion (`M-N`) and the file browser (`^T`) aren't actually
/// implemented yet — see apply_prompt_action's FlipConvert/Browser arms —
/// but are still listed rather than silently omitted, since pressing them
/// does now give real feedback.
const INSERT_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::FlipNewBuffer, "New Buffer"),
    (Action::FlipConvert, "No Conversion"),
    (Action::Browser, "Browse"),
    (Action::FlipExecute, "Execute Command"),
];

/// The `^T` Execute Command prompt's shortcut list, matching nano's full
/// MEXECUTE menu (confirmed against the installed nano's own bottom bar).
/// Full Justify (`^J`), Cut Till End (`^V`), Pipe Text (`M-\`), and Suspend
/// (`^Z`) aren't actually implemented yet — see apply_prompt_action's
/// arms for them — but are still listed rather than silently omitted.
const EXECUTE_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
    (Action::FlipNewBuffer, "New Buffer"),
    (Action::FlipPipe, "Pipe Text"),
    (Action::Speller, "Spell Check"),
    (Action::Linter, "Linter"),
    (Action::FullJustify, "Full Justify"),
    (Action::Formatter, "Formatter"),
    (Action::CutRestOfFile, "Cut Till End"),
    (Action::Suspend, "Suspend"),
];

/// The linter's interactive result viewer (`MLINTER`): Cancel plus
/// PageUp/PageDown to step to the previous/next reported message.
const LINTER_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Cancel, "Cancel"),
    (Action::PageUp, "Previous Linter message"),
    (Action::PageDown, "Next Linter message"),
];

/// The `^G` help viewer's own bottom bar (confirmed against the installed
/// nano's help screen).
const HELP_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Up, "Prev Line"),
    (Action::PageUp, "Prev Page"),
    (Action::FirstLine, "First Line"),
    (Action::Cancel, "Close"),
    (Action::Down, "Next Line"),
    (Action::PageDown, "Next Page"),
    (Action::LastLine, "Last Line"),
];

/// The "file changed on disk, you have unsaved edits" choice prompt: R/K/M
/// aren't keymap-driven (this prompt is tico-original and matches those
/// raw keystrokes directly — see `handle_conflict_choice`), so only the
/// `^G` entry is resolved against the live keymap; the rest stay literal.
const EXTERNAL_CONFLICT_SHORTCUTS: &[(&str, &str)] = &[
    ("R", "Reload"),
    ("K", "Keep mine"),
    ("M", "Merge"),
    ("I", "Ignore All"),
];

/// "Save modified buffer?" — confirmed against the installed nano's own
/// bar. The blank third entry keeps Cancel in the bottom-right slot,
/// matching nano's layout (its Y/N/^C bar isn't a plain fill-in-order
/// grid: Yes/No stack in the left column, Cancel sits alone at bottom
/// right).
const EXIT_SHORTCUTS: &[(&str, &str)] = &[("Y", "Yes"), ("N", "No"), ("", ""), ("^C", "Cancel")];

/// "Replace this instance?" — confirmed against the installed nano's own
/// bar; unlike the exit prompt this one fills all four slots, so no blank
/// padding is needed.
const REPLACE_CONFIRM_SHORTCUTS: &[(&str, &str)] =
    &[("Y", "Yes"), ("N", "No"), ("A", "All"), ("^C", "Cancel")];

/// The lock-conflict prompt ("File is being edited by ...; open anyway?")
/// is tico-original (no nano equivalent — nano has no interactive
/// lock-file prompt). ^C/N both decline identically (see
/// handle_lock_conflict_choice), so — matching the same reasoning that
/// dropped the deconflict prompt's redundant Cancel — only Yes/No are
/// advertised here.
const LOCK_CONFLICT_SHORTCUTS: &[(&str, &str)] = &[("Y", "Yes"), ("N", "No")];

/// Which shortcut list to show at the bottom for the current prompt (or
/// the main editing window, for `None`), with each entry's key label
/// resolved against the *live* keymap — nano rebuilds its two help lines
/// per-menu the same way (see e.g. `bottombars()` in its winio.c, which
/// looks up each function's current binding rather than a fixed table);
/// menus/prompts not yet curated here fall back to Main's list rather than
/// showing nothing.
fn shortcut_bar_entries(keymap: &KeyMap, prompt: Option<&Prompt>) -> Vec<(String, &'static str)> {
    let Some(p) = prompt else {
        return resolve_shortcuts(keymap, Menu::Main, SHORTCUT_PRIORITY);
    };
    if matches!(p.kind, PromptKind::ExternalChangeConflict) {
        let mut entries: Vec<(String, &str)> = EXTERNAL_CONFLICT_SHORTCUTS
            .iter()
            .map(|&(k, d)| (k.to_string(), d))
            .collect();
        entries.push((key_label_for(keymap, Menu::YesNo, Action::Help), "Get Help"));
        return entries;
    }
    // The other Y/N-style choice prompts: none of Y/N/A/^C go through the
    // keymap (they're raw keystrokes each handler matches directly — see
    // handle_exit_choice/handle_lock_conflict_choice/
    // handle_replace_confirm_choice), so these stay literal too.
    let literal: Option<&[(&str, &str)]> = match &p.kind {
        PromptKind::Exit { .. } => Some(EXIT_SHORTCUTS),
        PromptKind::LockConflict { .. } => Some(LOCK_CONFLICT_SHORTCUTS),
        PromptKind::ReplaceConfirm(_) => Some(REPLACE_CONFIRM_SHORTCUTS),
        _ => None,
    };
    if let Some(table) = literal {
        return table.iter().map(|&(k, d)| (k.to_string(), d)).collect();
    }
    let table: &[(Action, &str)] = match p.menu {
        Menu::Search => SEARCH_SHORTCUTS,
        Menu::Replace => REPLACE1_SHORTCUTS,
        Menu::ReplaceWith => REPLACEWITH_SHORTCUTS,
        Menu::GotoLine => GOTOLINE_SHORTCUTS,
        Menu::Help => HELP_SHORTCUTS,
        Menu::Insert => INSERT_SHORTCUTS,
        Menu::Execute => EXECUTE_SHORTCUTS,
        Menu::Linter => LINTER_SHORTCUTS,
        _ => return resolve_shortcuts(keymap, Menu::Main, SHORTCUT_PRIORITY),
    };
    resolve_shortcuts(keymap, p.menu, table)
}

/// Resolve each `(action, description)` pair in `table` to
/// `(current key label for that action in `menu`, description)`.
fn resolve_shortcuts(
    keymap: &KeyMap,
    menu: Menu,
    table: &[(Action, &'static str)],
) -> Vec<(String, &'static str)> {
    table
        .iter()
        .map(|&(action, desc)| (key_label_for(keymap, menu, action), desc))
        .collect()
}

/// The best key currently bound to `action` within `menu` — "best" meaning
/// the one `Key::display_rank` would show first among alternates (Ctrl
/// before function keys before Meta), matching how the `^G` help screen
/// already picks a primary key to display. Empty if nothing is bound
/// (e.g. the user `unbind`-ed it), rather than showing a stale label.
fn key_label_for(keymap: &KeyMap, menu: Menu, action: Action) -> String {
    keymap
        .entries()
        .filter(|((m, _), binding)| *m == menu && **binding == Binding::Action(action))
        .map(|((_, k), _)| *k)
        .min_by_key(|k| k.display_rank())
        .map(|k| k.describe())
        .unwrap_or_default()
}

fn render_shortcut_bar(
    out: &mut impl Write,
    row: u16,
    cols: usize,
    entries: &[(String, &str)],
) -> io::Result<()> {
    let max_label = entries
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(2);
    let max_desc = entries
        .iter()
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(4);
    let col_width = max_label + 1 + max_desc + 2;
    let n_cols = (cols / col_width).max(1);
    let n_pairs = n_cols.min(entries.len().div_ceil(2));

    for r in 0..2u16 {
        queue!(out, MoveTo(0, row + r))?;
        let mut written = 0usize;
        for c in 0..n_pairs {
            let idx = c * 2 + r as usize;
            if let Some((key, desc)) = entries.get(idx).filter(|(k, _)| !k.is_empty()) {
                // As in nano: the key combo is shown in reverse video, the
                // description in the terminal's normal colors.
                let key_padded = format!("{key:<lw$}", lw = max_label);
                queue!(
                    out,
                    SetAttribute(Attribute::Reverse),
                    Print(&key_padded),
                    SetAttribute(Attribute::Reset)
                )?;
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

/// The `^R` Read File prompt's `Tab`-completion listing, shown in place of
/// the buffer — matches nano's `input_tab` (`blank_edit()` + a sorted,
/// multi-column grid, bottom-aligned within the edit window, with
/// `"(more)"` in the last cell when it doesn't all fit).
fn render_completions_grid(
    out: &mut impl Write,
    start_row: u16,
    rows: usize,
    cols: usize,
    matches: &[String],
) -> io::Result<()> {
    for r in 0..rows {
        queue!(
            out,
            MoveTo(0, start_row + r as u16),
            Print(" ".repeat(cols))
        )?;
    }
    if matches.is_empty() || rows == 0 || cols == 0 {
        return Ok(());
    }
    let longest = matches
        .iter()
        .map(|m| m.chars().count())
        .max()
        .unwrap_or(0)
        .min(cols.saturating_sub(1));
    let col_width = longest + 2;
    let ncols = ((cols + 1) / col_width).max(1);
    let nrows = matches.len().div_ceil(ncols);
    let top_row = rows.saturating_sub(nrows);

    let mut row = top_row;
    for (i, name) in matches.iter().enumerate() {
        if row >= rows {
            break;
        }
        let col_idx = i % ncols;
        let is_last_row = row == rows - 1;
        let fills_row = (i + 1) % ncols == 0;
        let more_remain = i + 1 < matches.len();
        if is_last_row && fills_row && more_remain {
            queue!(
                out,
                MoveTo((col_width * col_idx) as u16, start_row + row as u16),
                Print("(more)")
            )?;
            break;
        }
        let display: String = name.chars().take(longest).collect();
        queue!(
            out,
            MoveTo((col_width * col_idx) as u16, start_row + row as u16),
            Print(&display)
        )?;
        if fills_row {
            row += 1;
        }
    }
    Ok(())
}

fn render_buffer(
    editor: &Editor,
    out: &mut impl Write,
    start_row: u16,
    rows: usize,
) -> io::Result<()> {
    let buf = editor.buf();
    let gutter = editor.gutter_width();
    let cols = editor.screen_cols;
    let tabsize = editor.options.tabsize as usize;

    // Memoized on the buffer itself, invalidated only by an actual edit or
    // language change (see `Buffer::highlighted_spans_cached`) -- a full
    // tree-sitter reparse plus query run is too expensive to redo on every
    // render, which used to happen even for pure cursor movement. Buffers
    // over `max_syntax_highlight_bytes` skip highlighting altogether
    // (tico-only safety valve; see `maybe_warn_highlighting_disabled_for_size`
    // for the one-time status notice) -- checking that is just a length
    // read, no parsing attempted.
    let too_large_to_highlight =
        buf.rope.len_bytes() as u64 > editor.options.max_syntax_highlight_bytes;
    let spans: Vec<crate::syntax::HighlightSpan> =
        if editor.options.syntax_highlighting && !too_large_to_highlight {
            buf.language
                .map(|lang| buf.highlighted_spans_cached(lang))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    let selection = editor.selection_range();

    for r in 0..rows {
        queue!(out, MoveTo(0, start_row + r as u16))?;
        let line_idx = buf.top_line + r;
        let mut rendered = String::new();
        // Syntax-highlight classification, one entry per char of `rendered`.
        let mut kinds: Vec<Option<crate::syntax::HighlightKind>> = Vec::new();
        // Character range within `rendered` (post-gutter, post-tab-expansion,
        // pre-horizontal-scroll) to paint with spotlightcolor, taking
        // precedence over syntax colors, if the active search/replace match
        // is on this line.
        let mut highlight: Option<(usize, usize)> = None;
        // Character range covering the marked selection on this line, if
        // any -- lower priority than `highlight` (an active search/replace
        // match), matching nano's own SELECTED_TEXT vs. spotlight layering.
        let mut selected: Option<(usize, usize)> = None;
        let mut gutter_chars = 0;
        let is_real_line = line_idx < buf.line_count();

        if is_real_line {
            if gutter > 0 {
                let prefix = format!("{:>width$} ", line_idx + 1, width = gutter - 1);
                kinds.extend(prefix.chars().map(|_| None));
                rendered.push_str(&prefix);
            }
            let raw = buf.line(line_idx);
            gutter_chars = rendered.chars().count();

            let line_start = buf.line_start_byte(line_idx);
            let char_kinds = map_spans_to_line(&raw, line_start, &spans);

            let (expanded, expanded_kinds) = expand_tabs_with_kinds(&raw, &char_kinds, tabsize);
            rendered.push_str(&expanded);
            kinds.extend(expanded_kinds);

            if let Some((pos, len)) = editor.spotlight
                && pos.line == line_idx
            {
                let start = gutter_chars + crate::buffer::display_width(&raw, pos.col, tabsize);
                let end = gutter_chars + crate::buffer::display_width(&raw, pos.col + len, tabsize);
                if end > start {
                    highlight = Some((start, end));
                }
            }

            if let Some((sel_start, sel_end)) = selection
                && line_idx >= sel_start.line
                && line_idx <= sel_end.line
            {
                let start_col = if line_idx == sel_start.line {
                    sel_start.col
                } else {
                    0
                };
                let end_col = if line_idx == sel_end.line {
                    sel_end.col
                } else {
                    raw.chars().count()
                };
                let start = gutter_chars + crate::buffer::display_width(&raw, start_col, tabsize);
                let end = gutter_chars + crate::buffer::display_width(&raw, end_col, tabsize);
                if end > start {
                    selected = Some((start, end));
                }
            }
        } else if gutter > 0 {
            rendered.push('~');
            kinds.push(None);
        }

        // Horizontal scroll: only the cursor's own line ever gets a nonzero
        // offset (nano scrolls just the current line sideways, not the
        // whole viewport — confirmed against the installed nano). A `<`
        // marker appears once scrolled; a `>` marker appears whenever the
        // line's text still overflows the available width, on any line.
        let full_chars: Vec<char> = rendered.chars().collect();
        let text_total = full_chars.len().saturating_sub(gutter_chars);
        let left = if is_real_line && line_idx == buf.cursor.line {
            buf.left_col.min(text_total)
        } else {
            0
        };
        let content_width = cols.saturating_sub(gutter_chars);
        let show_left = left > 0;
        let mut capacity = content_width.saturating_sub(if show_left { 1 } else { 0 });
        let show_right = left + capacity < text_total;
        if show_right {
            capacity = capacity.saturating_sub(1);
        }
        let vis_start = gutter_chars + left;
        let vis_end = (vis_start + capacity).min(full_chars.len());

        let mut chars: Vec<char> = full_chars[..gutter_chars].to_vec();
        if show_left {
            chars.push('<');
        }
        chars.extend_from_slice(&full_chars[vis_start..vis_end]);
        if show_right {
            chars.push('>');
        }
        let mut windowed_kinds: Vec<Option<crate::syntax::HighlightKind>> =
            kinds[..gutter_chars].to_vec();
        if show_left {
            windowed_kinds.push(None);
        }
        windowed_kinds.extend_from_slice(&kinds[vis_start..vis_end]);
        if show_right {
            windowed_kinds.push(None);
        }
        let kinds = windowed_kinds;
        let marker_shift = gutter_chars + if show_left { 1 } else { 0 };
        let clamp_to_view = |(s, e): (usize, usize)| {
            let clamp = |x: usize| marker_shift + x.clamp(vis_start, vis_end) - vis_start;
            (clamp(s), clamp(e))
        };
        let highlight = highlight.map(clamp_to_view);
        let selected = selected.map(clamp_to_view);

        let len = chars.len();
        let spot = highlight
            .map(|(s, e)| (s.min(len), e.min(len)))
            .filter(|(s, e)| s < e);
        let sel = selected
            .map(|(s, e)| (s.min(len), e.min(len)))
            .filter(|(s, e)| s < e);

        let (spot_fg, spot_bg) = spotlight_colors(&editor.options.spotlightcolor);
        let selection_style = selection_render_style(&editor.options.selectedcolor);
        let mut i = 0;
        while i < len {
            let in_spot = spot.is_some_and(|(s, e)| i >= s && i < e);
            let in_sel = !in_spot && sel.is_some_and(|(s, e)| i >= s && i < e);
            let kind = if in_spot || in_sel {
                None
            } else {
                kinds.get(i).copied().flatten()
            };
            let mut j = i + 1;
            while j < len {
                let j_in_spot = spot.is_some_and(|(s, e)| j >= s && j < e);
                let j_in_sel = !j_in_spot && sel.is_some_and(|(s, e)| j >= s && j < e);
                if j_in_spot != in_spot || j_in_sel != in_sel {
                    break;
                }
                let j_kind = if j_in_spot || j_in_sel {
                    None
                } else {
                    kinds.get(j).copied().flatten()
                };
                if j_kind != kind {
                    break;
                }
                j += 1;
            }
            let segment: String = chars[i..j].iter().collect();
            if in_spot {
                queue!(
                    out,
                    SetForegroundColor(spot_fg),
                    SetBackgroundColor(spot_bg),
                    Print(segment),
                    SetAttribute(Attribute::Reset)
                )?;
            } else if in_sel {
                match selection_style {
                    SelectionStyle::Reverse => {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(segment),
                            SetAttribute(Attribute::Reset)
                        )?;
                    }
                    SelectionStyle::Colored(fg, bg) => {
                        queue!(
                            out,
                            SetForegroundColor(fg),
                            SetBackgroundColor(bg),
                            Print(segment),
                            SetAttribute(Attribute::Reset)
                        )?;
                    }
                }
            } else if let Some(kind) = kind {
                queue!(
                    out,
                    SetForegroundColor(syntax_color(kind)),
                    Print(segment),
                    SetAttribute(Attribute::Reset)
                )?;
            } else {
                queue!(out, Print(segment))?;
            }
            i = j;
        }
        if len < cols {
            queue!(out, Print(" ".repeat(cols - len)))?;
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

/// How to paint the marked selection (`buf.mark`).
#[derive(Clone, Copy)]
enum SelectionStyle {
    /// nano's own default (`hilite_attribute`, `A_REVERSE`): plain reverse
    /// video, used whenever `selectedcolor` hasn't been configured.
    Reverse,
    /// An explicit `set selectedcolor` — a real color pair, like spotlight.
    Colored(Color, Color),
}

fn selection_render_style(cp: &crate::options::ColorPair) -> SelectionStyle {
    if cp.fg.is_none() && cp.bg.is_none() {
        return SelectionStyle::Reverse;
    }
    let fg = cp.fg.map(map_named_color).unwrap_or(Color::Reset);
    let bg = cp.bg.map(map_named_color).unwrap_or(Color::Reset);
    SelectionStyle::Colored(fg, bg)
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

/// Map whole-buffer byte-offset highlight spans onto one line's characters:
/// `raw` is that line's text, `line_start` its byte offset in the text
/// `spans` were computed from. Shared by the main editor buffer and the
/// merge-diff viewer, which both highlight a block of text line-by-line.
fn map_spans_to_line(
    raw: &str,
    line_start: usize,
    spans: &[crate::syntax::HighlightSpan],
) -> Vec<Option<crate::syntax::HighlightKind>> {
    let mut char_kinds = vec![None; raw.chars().count()];
    if spans.is_empty() {
        return char_kinds;
    }
    let line_end = line_start + raw.len();
    for span in spans {
        if span.end <= line_start || span.start >= line_end {
            continue;
        }
        let rel_start = span.start.max(line_start) - line_start;
        let rel_end = span.end.min(line_end) - line_start;
        let cs = raw[..rel_start].chars().count();
        let ce = raw[..rel_end].chars().count();
        for k in &mut char_kinds[cs..ce] {
            *k = Some(span.kind);
        }
    }
    char_kinds
}

/// Like tab expansion alone, but carries each source character's syntax
/// classification along to every column it expands to, so a tab adjacent to
/// a highlighted token doesn't break the highlighting.
fn expand_tabs_with_kinds(
    line: &str,
    kinds: &[Option<crate::syntax::HighlightKind>],
    tabsize: usize,
) -> (String, Vec<Option<crate::syntax::HighlightKind>>) {
    let mut out = String::new();
    let mut out_kinds = Vec::new();
    let mut w = 0;
    for (c, k) in line.chars().zip(kinds.iter().copied()) {
        if c == '\t' {
            let n = tabsize - (w % tabsize);
            for _ in 0..n {
                out.push(' ');
                out_kinds.push(k);
            }
            w += n;
        } else {
            out.push(c);
            out_kinds.push(k);
            w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        }
    }
    (out, out_kinds)
}

/// Map a `HighlightKind` to a terminal color. Not derived from any nanorc
/// syntax file (tico's own syntax highlighting is independent of nano's
/// per-language color files) but loosely follows the same conventions found
/// there: green comments, yellow strings, magenta-ish numbers/constants,
/// blue keywords.
fn syntax_color(kind: crate::syntax::HighlightKind) -> Color {
    use crate::syntax::HighlightKind as HK;
    match kind {
        HK::Comment => Color::DarkGreen,
        HK::String => Color::Yellow,
        HK::Number => Color::Magenta,
        HK::Keyword => Color::Blue,
        HK::Function => Color::Cyan,
        HK::Type => Color::DarkYellow,
        HK::Constant => Color::DarkMagenta,
        HK::Variable => Color::DarkCyan,
        HK::Module => Color::DarkBlue,
        HK::Attribute => Color::Green,
        HK::Tag => Color::Red,
    }
}

/// Color mapping for the merge-diff viewer, using the conventional
/// green-for-added/red-for-removed scheme instead of `syntax_color`'s
/// generic per-language palette (which would otherwise show additions as
/// plain string-yellow and deletions as keyword-blue, per `diff.scm`'s
/// arbitrary bucket choices — fine for general syntax highlighting, but not
/// what anyone expects from a diff).
fn diff_color(kind: crate::syntax::HighlightKind) -> Color {
    use crate::syntax::HighlightKind as HK;
    match kind {
        HK::String => Color::Green,    // (addition) / (new_file)
        HK::Keyword => Color::Red,     // (deletion) / (old_file)
        HK::Constant => Color::Yellow, // (commit)
        HK::Attribute => Color::Cyan,  // (location), e.g. an "@@" hunk header
        HK::Variable => Color::Blue,   // (command)
        _ => syntax_color(kind),
    }
}

/// Highlight `body` (already-split lines of a unified diff) with the
/// vendored "diff" tree-sitter grammar, one classification array per line
/// — mirrors how `render_buffer` highlights the main editor buffer, just
/// without a gutter, tabs, or horizontal scroll to account for. Returns
/// `None` if the "diff" language somehow isn't registered (never happens
/// in practice; guards against a future registry change more than
/// anything).
fn diff_line_kinds(body: &[String]) -> Option<Vec<Vec<Option<crate::syntax::HighlightKind>>>> {
    let lang = crate::syntax::find_by_name("diff")?;
    let text = body.join("\n");
    let spans = crate::syntax::highlight(&text, lang);
    let mut line_start = 0usize;
    let mut out = Vec::with_capacity(body.len());
    for line in body {
        out.push(map_spans_to_line(line, line_start, &spans));
        line_start += line.len() + 1; // +1 for the '\n' joiner
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_editor(text: &str) -> Editor {
        let mut ed = Editor::new(crate::options::Options::default(), KeyMap::defaults(false));
        ed.buffers[0] = crate::buffer::Buffer::from_text(text, None);
        ed
    }

    #[test]
    fn shift_right_sets_a_soft_mark_and_extends_the_selection() {
        let mut ed = test_editor("hello world");
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(ed.buf().mark, Some(Pos::new(0, 0)));
        assert!(ed.buf().softmark);
        assert_eq!(ed.buf().cursor, Pos::new(0, 1));

        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(
            ed.buf().mark,
            Some(Pos::new(0, 0)),
            "the mark's anchor shouldn't move on further shift-movement"
        );
        assert_eq!(ed.buf().cursor, Pos::new(0, 2));
    }

    #[test]
    fn plain_movement_after_shift_selection_drops_the_soft_mark() {
        let mut ed = test_editor("hello world");
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert!(ed.buf().mark.is_some());

        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            ed.buf().mark,
            None,
            "a plain movement key should collapse the selection"
        );
        assert!(!ed.buf().softmark);
        assert_eq!(ed.buf().cursor, Pos::new(0, 2), "the cursor still moves");
    }

    #[test]
    fn hard_mark_survives_plain_movement() {
        let mut ed = test_editor("hello world");
        ed.execute(Action::Mark);
        assert!(ed.buf().mark.is_some());
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(
            ed.buf().mark.is_some(),
            "^^/M-A sets a hard mark, unaffected by plain movement"
        );
    }

    #[test]
    fn typing_a_character_drops_a_soft_mark() {
        let mut ed = test_editor("hello world");
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert!(ed.buf().mark.is_some());
        handle_editing_key(
            &mut ed,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(ed.buf().mark.is_none());
        assert_eq!(ed.buf().to_string(), "hxello world");
    }

    #[test]
    fn view_mode_blocks_plain_character_insertion() {
        let mut ed = test_editor("hello");
        ed.options.view = true;
        handle_editing_key(
            &mut ed,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(ed.buf().to_string(), "hello");
        assert_eq!(ed.status.as_deref(), Some("Key is invalid in view mode"));
    }

    #[test]
    fn write_selection_to_file_writes_only_the_marked_text() {
        let path = std::env::temp_dir().join("tico_test_write_selection.txt");
        std::fs::remove_file(&path).ok();
        let mut ed = test_editor("hello\nworld\nagain\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0); // selects the first two lines whole
        let prompt = Prompt {
            kind: PromptKind::WriteOut { exiting: false },
            menu: Menu::WriteOut,
            label: "Write Selection to File".to_string(),
            input: path.to_str().unwrap().to_string(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        };
        submit_prompt(&mut ed, prompt);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld\n");
        assert!(
            ed.buf().mark.is_some(),
            "writing the selection shouldn't clear it (confirmed against the installed nano)"
        );
        assert!(
            ed.buf().path.is_none(),
            "the buffer's own path/state shouldn't change from a selection-only write"
        );
        std::fs::remove_file(&path).ok();
    }

    fn insert_prompt(new_buffer: bool, input: &str) -> Prompt {
        Prompt {
            kind: PromptKind::InsertFile {
                new_buffer,
                execute: false,
            },
            menu: Menu::Insert,
            label: crate::app::insert_prompt_label(new_buffer, false),
            input: input.to_string(),
            cursor: input.chars().count(),
            history_pos: None,
            saved_input: None,
        }
    }

    #[test]
    fn insert_file_reads_content_into_current_buffer_at_cursor() {
        let path = std::env::temp_dir().join("tico_test_insert_at_cursor.txt");
        std::fs::write(&path, "INSERTED\n").unwrap();
        let mut ed = test_editor("hello\nworld\n");
        ed.buf_mut().cursor = crate::buffer::Pos::new(1, 0); // start of "world"
        submit_prompt(&mut ed, insert_prompt(false, path.to_str().unwrap()));
        assert_eq!(ed.buf().to_string(), "hello\nINSERTED\nworld\n");
        assert_eq!(ed.buffers.len(), 1, "should not have opened a new buffer");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn common_prefix_of_strings() {
        assert_eq!(common_prefix("foobar", "foobaz"), "fooba");
        assert_eq!(common_prefix("foo", "bar"), "");
        assert_eq!(common_prefix("foo", "foo"), "foo");
        assert_eq!(common_prefix("foo", "foobar"), "foo");
    }

    fn tab_complete_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tab_completion_single_match_completes_fully() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_single");
        std::fs::write(dir.join("readme.txt"), "").unwrap();
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/rea", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt);
        assert_eq!(prompt.input, format!("{}/readme.txt", dir.display()));
        assert_eq!(prompt.cursor, prompt.input.chars().count());
        assert!(ed.file_completions.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_directory_match_appends_slash() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_dir");
        std::fs::create_dir(dir.join("subdir")).unwrap();
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/sub", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt);
        assert_eq!(prompt.input, format!("{}/subdir/", dir.display()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_multiple_matches_completes_common_prefix_and_lists() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_multi");
        std::fs::write(dir.join("foo_alpha.txt"), "").unwrap();
        std::fs::write(dir.join("foo_beta.txt"), "").unwrap();
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/foo_", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt);
        assert_eq!(prompt.input, format!("{}/foo_", dir.display()));
        let matches = ed
            .file_completions
            .expect("should list the ambiguous matches");
        let mut sorted = matches.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["foo_alpha.txt", "foo_beta.txt"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_no_matches_leaves_input_unchanged() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_none");
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/nope", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt);
        assert_eq!(prompt.input, format!("{}/nope", dir.display()));
        assert!(ed.file_completions.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_not_offered_in_execute_command_mode() {
        // Matches nano's MINSERTFILE-only gate: Execute Command (^T)
        // doesn't get filename completion.
        let dir = tab_complete_test_dir("tico_test_tabcomplete_execute");
        std::fs::write(dir.join("readme.txt"), "").unwrap();
        let mut ed = test_editor("x");
        let original_input = format!("{}/rea", dir.display());
        let prompt = Prompt {
            kind: PromptKind::InsertFile {
                new_buffer: false,
                execute: true,
            },
            menu: Menu::Execute,
            label: "Command to execute".to_string(),
            input: original_input.clone(),
            cursor: original_input.chars().count(),
            history_pos: None,
            saved_input: None,
        };
        handle_prompt_key(
            &mut ed,
            prompt,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        // The Tab key normalizes to Ctrl('I'), which has no binding in
        // Menu::Execute, so the prompt is left completely unchanged.
        if let Mode::Prompt(p) = &ed.mode {
            assert_eq!(p.input, original_input);
        } else {
            panic!("expected prompt to still be open");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn username_completion_matches_filters_by_prefix_and_sorts() {
        let users: Vec<String> = ["bob", "alice", "alicia"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            username_completion_matches(&users, "ali"),
            vec!["~alice", "~alicia"]
        );
        assert_eq!(username_completion_matches(&users, "bob"), vec!["~bob"]);
        assert!(username_completion_matches(&users, "nope").is_empty());
        // An empty fragment (a bare "~") matches everyone.
        assert_eq!(
            username_completion_matches(&users, ""),
            vec!["~alice", "~alicia", "~bob"]
        );
    }

    #[test]
    fn apply_username_completion_no_match_leaves_input_unchanged() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "~tico_test_no_such_user_xyz");
        let morsel = prompt.input.clone();
        apply_username_completion(&mut ed, &mut prompt, &morsel);
        assert_eq!(prompt.input, "~tico_test_no_such_user_xyz");
        assert!(ed.file_completions.is_none());
    }

    #[test]
    fn apply_username_completion_current_user_stays_stable_when_already_complete() {
        // Portable against whatever /etc/passwd actually contains: typing
        // the current user's full name is already the longest common
        // prefix of every matching entry (itself, and anything else that
        // happens to start with the same string), so completion must
        // leave it exactly as-is regardless of the environment.
        let Ok(user) = std::env::var("USER") else {
            return; // not set in this environment; skip rather than fail
        };
        let mut ed = test_editor("x");
        let fragment = format!("~{user}");
        let mut prompt = insert_prompt(false, &fragment);
        apply_username_completion(&mut ed, &mut prompt, &fragment);
        assert_eq!(prompt.input, fragment);
    }

    #[test]
    fn apply_username_completion_lists_real_matches_when_ambiguous() {
        // Find two real usernames on this system sharing a nonempty common
        // prefix, to exercise the >1-match listing path against the
        // actual system database rather than only the pure matcher above.
        let users = crate::fileio::list_usernames();
        let common = users.iter().enumerate().find_map(|(i, u)| {
            users[i + 1..]
                .iter()
                .map(|v| common_prefix(u, v))
                .find(|c| !c.is_empty())
        });
        let Some(common) = common else {
            return; // no ambiguous pair of usernames on this system; skip
        };
        let mut ed = test_editor("x");
        let fragment = format!("~{common}");
        let mut prompt = insert_prompt(false, &fragment);
        apply_username_completion(&mut ed, &mut prompt, &fragment);
        let matches = ed
            .file_completions
            .expect("an ambiguous fragment should list its matches");
        assert!(matches.len() > 1);
        assert!(matches.iter().all(|m| m.starts_with(&fragment)));
    }

    #[test]
    fn tab_completion_tilde_fragment_with_slash_is_not_username_completion() {
        // "~/..." contains a slash, so it must go through plain filename
        // completion (against the real home directory) rather than
        // username completion, even though it starts with `~`.
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "~/tico_test_no_such_dir_xyz123/rea");
        apply_filename_completion(&mut ed, &mut prompt);
        assert_eq!(prompt.input, "~/tico_test_no_such_dir_xyz123/rea");
    }

    #[test]
    fn insert_file_new_buffer_opens_a_separate_buffer() {
        let path = std::env::temp_dir().join("tico_test_insert_new_buffer.txt");
        std::fs::write(&path, "SEPARATE CONTENT\n").unwrap();
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.buf().to_string(), "SEPARATE CONTENT\n");
        assert_eq!(
            ed.buffers[0].to_string(),
            "original\n",
            "original buffer untouched"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn insert_file_new_buffer_gets_syntax_highlighting_like_argv() {
        let path = std::env::temp_dir().join("tico_test_insert_new_buffer.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buf().language.map(|l| l.name), Some("rust"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn insert_file_nonexistent_path_new_buffer_gets_syntax_highlighting_too() {
        let path = std::env::temp_dir().join("tico_test_insert_does_not_exist.py");
        std::fs::remove_file(&path).ok();
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buf().language.map(|l| l.name), Some("python"));
    }

    #[test]
    fn insert_file_empty_input_new_buffer_opens_blank_buffer() {
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, ""));
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.buf().to_string(), "");
    }

    #[test]
    fn insert_file_empty_input_without_new_buffer_cancels() {
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(false, ""));
        assert_eq!(ed.buffers.len(), 1);
        assert_eq!(ed.buf().to_string(), "original\n");
        assert_eq!(ed.status.as_deref(), Some("Cancelled"));
    }

    #[test]
    fn insert_file_nonexistent_path_new_buffer_gives_blank_named_buffer() {
        let path = std::env::temp_dir().join("tico_test_insert_does_not_exist.txt");
        std::fs::remove_file(&path).ok(); // just in case a prior run left it
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.buf().to_string(), "");
        assert_eq!(ed.buf().path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn flip_new_buffer_toggles_label_and_state() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipNewBuffer
        ));
        assert_eq!(
            prompt.kind,
            PromptKind::InsertFile {
                new_buffer: true,
                execute: false
            }
        );
        assert!(prompt.label.contains("new buffer"));
    }

    #[test]
    fn unimplemented_insert_actions_report_plainly_and_close_the_prompt() {
        for (action, expected) in [
            (Action::FlipConvert, "No Conversion: not yet implemented"),
            (Action::Browser, "File Browser: not yet implemented"),
        ] {
            let mut ed = test_editor("x");
            let mut prompt = insert_prompt(false, "");
            assert!(
                apply_prompt_action(&mut ed, &mut prompt, action),
                "{action:?}"
            );
            assert!(
                matches!(ed.mode, Mode::Editing),
                "{action:?} should close the prompt"
            );
            assert_eq!(ed.status.as_deref(), Some(expected), "{action:?}");
        }
    }

    #[test]
    fn flip_execute_toggles_mode_menu_and_label() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipExecute
        ));
        assert_eq!(
            prompt.kind,
            PromptKind::InsertFile {
                new_buffer: false,
                execute: true
            }
        );
        assert_eq!(prompt.menu, Menu::Execute);
        assert_eq!(prompt.label, "Command to execute");

        // Flipping back restores the Insert-File prompt.
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipExecute
        ));
        assert_eq!(
            prompt.kind,
            PromptKind::InsertFile {
                new_buffer: false,
                execute: false
            }
        );
        assert_eq!(prompt.menu, Menu::Insert);
        assert_eq!(prompt.label, "File to insert [from ./]");
    }

    #[test]
    fn execute_command_inserts_output_at_cursor() {
        let mut ed = test_editor("ab");
        ed.buf_mut().cursor = Pos::new(0, 1);
        submit_execute_command(&mut ed, "echo -n hello", false);
        assert_eq!(ed.buf().to_string(), "ahellob");
        assert!(ed.buf().modified);
        assert_eq!(ed.history.execute, vec!["echo -n hello".to_string()]);
    }

    #[test]
    fn execute_command_new_buffer_opens_a_separate_buffer() {
        let mut ed = test_editor("original");
        submit_execute_command(&mut ed, "echo -n hi", true);
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.buf().to_string(), "hi");
        assert_eq!(ed.buffers[0].to_string(), "original");
    }

    #[test]
    fn replace_whole_word_replaces_only_whole_word_matches() {
        let mut buf = crate::buffer::Buffer::from_text("teh cat sat on teh mat, nateh", None);
        assert!(replace_whole_word(&mut buf, "teh", "the"));
        assert_eq!(
            buf.to_string(),
            "the cat sat on the mat, nateh",
            "the trailing 'nateh' isn't a whole-word match and must be left alone"
        );
    }

    #[test]
    fn find_whole_word_finds_first_occurrence_only() {
        let buf = crate::buffer::Buffer::from_text("one\nteh two\nteh three", None);
        assert_eq!(find_whole_word(&buf, "teh"), Some(Pos::new(1, 0)));
        assert_eq!(find_whole_word(&buf, "missing"), None);
    }

    #[test]
    fn parse_linter_output_handles_colon_and_comma_column_forms() {
        let out = "main.rs:3:5: unused variable\nmain.rs:9: missing semicolon\nmain.rs:12,2: bad indent\nnot a lint line\n";
        let messages = parse_linter_output(out);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].line, 3);
        assert_eq!(messages[0].col, 5);
        assert_eq!(messages[0].msg, "unused variable");
        assert_eq!(messages[1].line, 9);
        assert_eq!(messages[1].col, 1);
        assert_eq!(messages[1].msg, "missing semicolon");
        assert_eq!(messages[2].line, 12);
        assert_eq!(messages[2].col, 2);
        assert_eq!(messages[2].msg, "bad indent");
    }

    #[test]
    fn format_byte_size_uses_the_largest_exact_unit() {
        assert_eq!(format_byte_size(4 * 1024 * 1024), "4MB");
        assert_eq!(format_byte_size(2 * 1024 * 1024 * 1024), "2GB");
        assert_eq!(format_byte_size(4096), "4KB");
        assert_eq!(format_byte_size(4097), "4097 bytes");
        assert_eq!(format_byte_size(0), "0 bytes");
    }

    #[test]
    fn warns_once_when_buffer_exceeds_the_size_limit() {
        let mut ed = test_editor(&"x".repeat(100));
        ed.options.max_syntax_highlight_bytes = 10;
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        ed.buf_mut().language = Some(lang);

        maybe_warn_highlighting_disabled_for_size(&mut ed);
        assert_eq!(
            ed.status.as_deref(),
            Some("Syntax highlighting disabled: file is larger than 10 bytes")
        );
        assert!(ed.buf().highlighting_size_warning_shown);

        // Doesn't repeat on a later check.
        ed.status = None;
        maybe_warn_highlighting_disabled_for_size(&mut ed);
        assert_eq!(ed.status, None);
    }

    #[test]
    fn no_size_warning_under_the_limit_or_without_a_detected_language() {
        let mut ed = test_editor("small");
        ed.options.max_syntax_highlight_bytes = 1_000_000;
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        ed.buf_mut().language = Some(lang);
        maybe_warn_highlighting_disabled_for_size(&mut ed);
        assert_eq!(ed.status, None, "well under the limit: no warning");

        let mut ed2 = test_editor(&"x".repeat(100));
        ed2.options.max_syntax_highlight_bytes = 10;
        maybe_warn_highlighting_disabled_for_size(&mut ed2);
        assert_eq!(
            ed2.status, None,
            "over the limit but no detected language: nothing to warn about"
        );
    }

    #[test]
    fn run_formatter_reports_when_none_configured() {
        let mut ed = test_editor("x");
        run_formatter(&mut ed);
        assert_eq!(
            ed.status.as_deref(),
            Some("No formatter is defined for this type of file")
        );
    }

    #[test]
    fn run_linter_reports_when_none_configured() {
        let mut ed = test_editor("x");
        run_linter(&mut ed);
        assert_eq!(
            ed.status.as_deref(),
            Some("No linter is defined for this type of file")
        );
    }

    #[test]
    fn shortcut_bar_reflects_modern_bindings() {
        let default_km = KeyMap::defaults(false);
        let modern_km = KeyMap::defaults(true);
        assert_eq!(key_label_for(&default_km, Menu::Main, Action::Help), "^G");
        assert_eq!(key_label_for(&modern_km, Menu::Main, Action::Help), "^H");
        assert_eq!(key_label_for(&default_km, Menu::Main, Action::Exit), "^X");
        assert_eq!(key_label_for(&modern_km, Menu::Main, Action::Exit), "^Q");
    }

    #[test]
    fn shortcut_bar_reflects_user_rebind() {
        let mut km = KeyMap::defaults(false);
        km.unbind(Menu::Main, crate::keymap::Key::Ctrl('X'));
        km.bind(
            Menu::Main,
            crate::keymap::Key::Ctrl('Q'),
            Binding::Action(Action::Exit),
        );
        assert_eq!(key_label_for(&km, Menu::Main, Action::Exit), "^Q");
    }

    #[test]
    fn key_label_for_unbound_action_is_empty() {
        let mut km = KeyMap::defaults(false);
        // Help has two default keys (^G and F1); unbind both.
        km.unbind(Menu::Main, crate::keymap::Key::Ctrl('G'));
        km.unbind(Menu::Main, crate::keymap::Key::F(1));
        assert_eq!(key_label_for(&km, Menu::Main, Action::Help), "");
    }

    #[test]
    fn main_shortcut_priority_resolves_to_nonempty_labels_by_default() {
        let km = KeyMap::defaults(false);
        let entries = resolve_shortcuts(&km, Menu::Main, SHORTCUT_PRIORITY);
        assert_eq!(entries.len(), SHORTCUT_PRIORITY.len());
        for (key, desc) in &entries {
            assert!(!key.is_empty(), "no key resolved for {desc:?}");
        }
    }
}
