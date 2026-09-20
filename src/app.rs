//! Editor application state and action dispatch: ties together buffers,
//! options, the keymap, cut/paste, search, and the various single-line
//! prompts (search, goto, save-as, yes/no, ...), independent of any
//! particular terminal backend.

use crate::buffer::{Buffer, Pos};
use crate::keymap::{Action, Menu};
use crate::options::Options;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    WhereIs,
    Replace1, // search term
    Replace2 { search: String }, // replacement term
    ReplaceConfirm(ReplaceLoopState),
    GotoLine,
    WriteOut { exiting: bool },
    Exit { discard_and_quit: bool },
    ExternalChangeConflict,
    MergeConflict,
    MergePreviewClean { merged_text: String },
    /// Someone else appears to be editing this file (a vim/nano-style lock
    /// file exists for it). `lock_path` is where to write our own lock if
    /// the user chooses to open anyway; `target` is the display path
    /// recorded inside it.
    LockConflict { lock_path: std::path::PathBuf, target: String },
    Help,
}

/// State threaded through an in-progress interactive replace, one match at
/// a time — matches nano's `do_replace_loop()` in src/search.c: find the
/// next occurrence, ask "Replace this instance?" (Yes/No/All/Cancel), act,
/// and repeat, wrapping around the buffer once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceLoopState {
    pub search: String,
    pub replacement: String,
    pub match_pos: Pos,
    pub match_len: usize,
    /// Where this replace operation began (the cursor position when it was
    /// kicked off); once the search wraps around and reaches here again,
    /// the loop stops rather than repeating forever.
    pub session_start: Pos,
    pub wrapped: bool,
    pub count: usize,
}

pub enum ReplaceChoice {
    Yes,
    No,
    All,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub menu: Menu,
    pub label: String,
    pub input: String,
    pub cursor: usize,
    /// Which history entry is currently shown (0 = most recent), while
    /// browsing history with Older/Newer; `None` means `input` is
    /// live-typed text, not a history entry.
    pub history_pos: Option<usize>,
    /// What `input` was before history browsing started, restored when
    /// Newer is pressed past the most recent entry.
    pub saved_input: Option<String>,
}

impl Prompt {
    pub fn new(kind: PromptKind, menu: Menu, label: impl Into<String>, input: impl Into<String>) -> Prompt {
        let input = input.into();
        let cursor = input.chars().count();
        Prompt { kind, menu, label: label.into(), input, cursor, history_pos: None, saved_input: None }
    }
}

pub enum Mode {
    Editing,
    Prompt(Prompt),
    /// The `^G` help viewer. `top` is the first scrolled-to body line
    /// (index into `lines`, which are wrapped and ready to draw as-is).
    /// `return_to` is the prompt to restore on close, when help was opened
    /// from one (e.g. `^G` inside a Search prompt) — `None` means it was
    /// opened from the main editing window, so closing goes back there.
    Help { lines: Vec<String>, top: usize, return_to: Option<Box<Prompt>> },
    Quit,
}

/// Severity of a status-bar message, matching the subset of nano's message
/// importance levels (src/definitions.h: VACUUM/HUSH/REMARK/INFO/NOTICE/
/// AHEM/MILD/ALERT) that affect rendering here: most messages are `Normal`
/// (nano's default STATUS_BAR color, reverse video); errors like "is a
/// directory" or "is unwritable" are `Alert` (nano's ERROR_MESSAGE color,
/// bold white-on-red, plus a bell); `Mild` warnings like "Directory is not
/// writable" use the same ERROR_MESSAGE color (MILD > NOTICE in nano's
/// enum) but without the bell (only importance == ALERT beeps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusLevel {
    #[default]
    Normal,
    Mild,
    Alert,
}

pub struct SearchState {
    pub last_pattern: Option<String>,
    pub case_sensitive: bool,
    pub use_regex: bool,
    pub backwards: bool,
}

impl Default for SearchState {
    fn default() -> Self {
        SearchState { last_pattern: None, case_sensitive: false, use_regex: false, backwards: false }
    }
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub current: usize,
    pub options: Options,
    pub keymap: crate::keymap::KeyMap,
    pub cutbuffer: String,
    pub cut_was_consecutive: bool,
    pub search: SearchState,
    pub status: Option<String>,
    pub status_level: StatusLevel,
    /// Keystrokes remaining before the status message is wiped, mirroring
    /// nano's `countdown` in src/winio.c: a status message is cleared after
    /// 20 keystrokes (or 1, with `quickblank`) in the main editing window —
    /// it is not a timer.
    status_countdown: u32,
    /// Set when an Alert-level message was just posted; the UI layer rings
    /// the terminal bell once and clears this, matching nano's beep() in
    /// statusline() for ALERT-importance messages.
    pub bell_pending: bool,
    /// The currently highlighted search/replace match, if any (position +
    /// length in characters), rendered black-on-yellow like nano's
    /// `spotlightcolor` (confirmed against the installed nano's own
    /// escape-code output).
    pub spotlight: Option<(Pos, usize)>,
    /// When the spotlight should be cleared on its own — `None` means it
    /// persists until something else clears it (used for the "Replace
    /// this instance?" match, which stays lit for as long as that prompt
    /// is up); `Some(deadline)` means a plain search match, which nano
    /// auto-clears after ~1.5s (or ~0.8s with quickblank) of no input —
    /// confirmed by timing the installed nano directly.
    pub spotlight_deadline: Option<std::time::Instant>,
    /// Search/Replace/Execute history, recalled with Up/Down or ^P/^N at
    /// those prompts. Built up in-session regardless of settings; only
    /// loaded from and saved to disk when `historylog` is on.
    pub history: crate::history::HistoryStore,
    pub mode: Mode,
    pub screen_rows: usize,
    pub screen_cols: usize,
}

impl Editor {
    pub fn new(options: Options, keymap: crate::keymap::KeyMap) -> Editor {
        // `set casesensitive` / `set regexp` in nanorc/ticorc set the
        // default search mode, same as nano; there's no CLI flag for
        // either (nano doesn't have one), only the config item.
        let search =
            SearchState { case_sensitive: options.casesensitive, use_regex: options.regexp, ..SearchState::default() };
        let history =
            if options.historylog { crate::history::HistoryStore::load() } else { crate::history::HistoryStore::new() };
        Editor {
            buffers: vec![Buffer::empty()],
            current: 0,
            options,
            keymap,
            cutbuffer: String::new(),
            cut_was_consecutive: false,
            search,
            status: None,
            status_level: StatusLevel::Normal,
            status_countdown: 0,
            bell_pending: false,
            spotlight: None,
            spotlight_deadline: None,
            history,
            mode: Mode::Editing,
            screen_rows: 24,
            screen_cols: 80,
        }
    }

    pub fn buf(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    pub fn buf_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_level = StatusLevel::Normal;
        self.status_countdown = if self.options.quickblank { 1 } else { 20 };
    }

    /// Like `set_status`, but for error-class messages (unwritable file,
    /// "is a directory", ...): rendered bold white-on-red instead of plain
    /// reverse video, and rings the terminal bell, matching nano's
    /// ALERT-importance messages.
    pub fn set_status_alert(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_level = StatusLevel::Alert;
        self.status_countdown = if self.options.quickblank { 1 } else { 20 };
        self.bell_pending = true;
    }

    /// Like `set_status_alert`, but for MILD-importance warnings (e.g.
    /// "Directory is not writable"): same coloring, no bell.
    pub fn set_status_mild(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_level = StatusLevel::Mild;
        self.status_countdown = if self.options.quickblank { 1 } else { 20 };
    }

    /// Highlight a plain search match, auto-clearing after ~1.5s (0.8s with
    /// quickblank) of no further input.
    fn set_spotlight_timed(&mut self, pos: Pos, len: usize) {
        self.spotlight = Some((pos, len));
        let ms = if self.options.quickblank { 800 } else { 1500 };
        self.spotlight_deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(ms));
    }

    /// Highlight the match currently up for replace confirmation; persists
    /// until explicitly cleared (when the prompt is answered), not timed.
    pub fn set_spotlight_persistent(&mut self, pos: Pos, len: usize) {
        self.spotlight = Some((pos, len));
        self.spotlight_deadline = None;
    }

    pub fn clear_spotlight(&mut self) {
        self.spotlight = None;
        self.spotlight_deadline = None;
    }

    /// If a timed spotlight's deadline has passed, clear it. Returns true
    /// if it just got cleared (so the caller knows to redraw).
    pub fn tick_spotlight_deadline(&mut self) -> bool {
        if let Some(deadline) = self.spotlight_deadline {
            if std::time::Instant::now() >= deadline {
                self.clear_spotlight();
                return true;
            }
        }
        false
    }

    /// Call once per keystroke handled while focused on the main edit
    /// window (not while a prompt is active), matching nano's
    /// `blank_it_when_expired()`. Wipes the status message once its
    /// countdown reaches zero. Returns true if the message was just wiped
    /// (so the caller knows a redraw is needed).
    pub fn tick_status_countdown(&mut self) -> bool {
        if self.status_countdown == 0 {
            return false;
        }
        self.status_countdown -= 1;
        if self.status_countdown == 0 {
            self.status = None;
            return true;
        }
        false
    }

    /// Number of rows available for buffer text (screen minus title bar,
    /// status line, and the two-line shortcut help unless `nohelp`).
    pub fn text_rows(&self) -> usize {
        let mut used = 2; // title bar + status/prompt line
        if !self.options.nohelp {
            used += 2;
        }
        if self.options.zero {
            used = 0;
        } else if self.options.minibar {
            used = 1;
        }
        self.screen_rows.saturating_sub(used).max(1)
    }

    /// Width, in columns, of the line-number margin (0 when `linenumbers`
    /// is off) — the text area proper is `screen_cols - gutter_width()`
    /// wide.
    pub fn gutter_width(&self) -> usize {
        if !self.options.linenumbers {
            return 0;
        }
        let digits = self.buf().line_count().to_string().len();
        digits + 1
    }

    pub fn scroll_to_cursor(&mut self) {
        let rows = self.text_rows();
        let buf = self.buf_mut();
        if buf.cursor.line < buf.top_line {
            buf.top_line = buf.cursor.line;
        } else if buf.cursor.line >= buf.top_line + rows {
            buf.top_line = buf.cursor.line + 1 - rows;
        }
        self.scroll_horizontal_to_cursor();
    }

    /// Horizontal counterpart of `scroll_to_cursor`, for lines too long to
    /// fit the screen (relevant only when `softwrap` is off, since a
    /// soft-wrapped line never needs sideways scrolling). Adjusts
    /// `buf.left_col` — the display-column offset applied when rendering
    /// just the cursor's current line — using the same "cushion" scheme
    /// nano uses when not soft-wrapping (its `united_sidescroll`, see
    /// `get_page_start()` in nano's src/utils.c): scrolling only kicks in
    /// within a few columns of either edge, and then jumps just enough to
    /// restore that margin, rather than moving one column at a time.
    fn scroll_horizontal_to_cursor(&mut self) {
        const CUSHION: usize = 3;
        let tabsize = self.options.tabsize as usize;
        let width = self.screen_cols.saturating_sub(self.gutter_width());
        let buf = self.buf_mut();
        let cursor_col = crate::buffer::display_width(&buf.line(buf.cursor.line), buf.cursor.col, tabsize);
        let left = buf.left_col;
        buf.left_col = if width <= 2 * CUSHION + 1 {
            // Too narrow for a cushioned scroll; just keep the cursor in
            // view.
            cursor_col.saturating_sub(width.saturating_sub(1))
        } else if cursor_col < CUSHION {
            0
        } else if cursor_col < left + CUSHION {
            cursor_col - CUSHION
        } else if cursor_col > left + width - CUSHION - 1 {
            cursor_col + CUSHION + 1 - width
        } else {
            left
        };
    }

    /// Like `scroll_to_cursor`, but when the cursor is off-screen, centers
    /// it in the viewport instead of scrolling just enough to reveal it.
    /// Matches nano's `edit_redraw(..., CENTERING)`, which it uses
    /// specifically for search/find-next/find-previous and replace jumps
    /// (confirmed directly against the installed nano for both) — ordinary
    /// cursor movement (arrows, page up/down, ...) keeps the minimal-scroll
    /// behavior of plain `scroll_to_cursor`.
    pub fn scroll_to_cursor_centered(&mut self) {
        let rows = self.text_rows();
        let buf = self.buf_mut();
        if buf.cursor.line < buf.top_line || buf.cursor.line >= buf.top_line + rows {
            buf.top_line = buf.cursor.line.saturating_sub(rows / 2);
        }
        self.scroll_horizontal_to_cursor();
    }

    /// Dispatch one editing action. Returns true if the caller should
    /// re-render (essentially always, but kept for future use).
    pub fn execute(&mut self, action: Action) {
        use Action::*;
        // Any action other than Cut clears nano's "consecutive cuts append
        // to the same cutbuffer" chain.
        if !matches!(action, Cut | CutRestOfFile) {
            self.cut_was_consecutive = false;
        }
        match action {
            Help => {
                let lines = crate::help::build(Menu::Main, &self.keymap, self.screen_cols);
                self.mode = Mode::Help { lines, top: 0, return_to: None };
            }
            Cancel => {
                self.mode = Mode::Editing;
            }
            Exit => self.begin_exit(),
            WriteOut => self.begin_writeout(false),
            SaveFile => self.quick_save(),
            Insert => self.set_status("insert-file: not yet implemented"),
            WhereIs => self.begin_search(),
            WhereWas => {
                self.search.backwards = true;
                self.begin_search();
            }
            FindNext => self.repeat_search(false),
            FindPrevious => self.repeat_search(true),
            Replace => self.begin_replace(),
            Cut => self.do_cut(),
            CutRestOfFile => self.do_cut_rest_of_file(),
            Copy => self.do_copy(),
            Paste => self.do_paste(),
            Mark => self.toggle_mark(),
            Location => self.report_location(),
            WordCount => self.report_word_count(),
            Undo => {
                if self.buf_mut().undo() {
                    self.set_status("Undo");
                } else {
                    self.set_status("Nothing to undo");
                }
            }
            Redo => {
                if self.buf_mut().redo() {
                    self.set_status("Redo");
                } else {
                    self.set_status("Nothing to redo");
                }
            }
            Left => self.buf_mut().move_left(),
            Right => self.buf_mut().move_right(),
            Up => self.buf_mut().move_up(),
            Down => self.buf_mut().move_down(),
            Home => self.buf_mut().move_home(),
            End => self.buf_mut().move_end(),
            PrevWord => self.move_prev_word(),
            NextWord => self.move_next_word(),
            PageUp => self.page_up(),
            PageDown => self.page_down(),
            FirstLine => {
                self.buf_mut().cursor = Pos::new(0, 0);
            }
            LastLine => {
                let last = self.buf().line_count().saturating_sub(1);
                self.buf_mut().cursor = Pos::new(last, 0);
            }
            GotoLine => self.begin_goto_line(),
            Tab => self.buf_mut().insert_char('\t'),
            Enter => self.do_enter(),
            Delete => self.buf_mut().delete_forward(),
            Backspace => self.buf_mut().backspace(),
            Zap => self.do_zap(),
            ChopWordLeft => self.chop_word_left(),
            ChopWordRight => self.chop_word_right(),
            Complete => self.set_status("complete: not yet implemented"),
            Justify | FullJustify => self.set_status("justify: not yet implemented"),
            Indent => self.set_status("indent: not yet implemented"),
            Unindent => self.set_status("unindent: not yet implemented"),
            Comment => self.set_status("comment: not yet implemented"),
            Center => {}
            Cycle => {}
            ScrollUp => self.scroll_view(-1),
            ScrollDown => self.scroll_view(1),
            ScrollLeft | ScrollRight => {}
            BeginPara | EndPara | PrevBlock | NextBlock | TopRow | BottomRow => {
                self.set_status("paragraph/block navigation: not yet implemented");
            }
            FindBracket => self.set_status("find-bracket: not yet implemented"),
            Anchor | PrevAnchor | NextAnchor => self.set_status("anchors: not yet implemented"),
            PrevBuf => self.switch_buffer(-1),
            NextBuf => self.switch_buffer(1),
            Verbatim => {}
            RecordMacro | RunMacro => self.set_status("macros: not yet implemented"),
            Refresh => {}
            Suspend => self.set_status("suspend: not supported in this build"),
            Speller | Formatter | Linter | Execute => {
                self.set_status("external command integration: not yet implemented");
            }
            NoHelp => self.options.nohelp = !self.options.nohelp,
            Zero => self.options.zero = !self.options.zero,
            ConstantShow => self.options.constantshow = !self.options.constantshow,
            SoftWrap => self.options.softwrap = !self.options.softwrap,
            LineNumbers => self.options.linenumbers = !self.options.linenumbers,
            WhitespaceDisplay => {}
            NoSyntax => self.options.syntax_highlighting = !self.options.syntax_highlighting,
            SmartHome => self.options.smarthome = !self.options.smarthome,
            AutoIndent => self.options.autoindent = !self.options.autoindent,
            CutFromCursor => self.options.cutfromcursor = !self.options.cutfromcursor,
            BreakLongLines => self.options.breaklonglines = !self.options.breaklonglines,
            TabsToSpaces => self.options.tabstospaces = !self.options.tabstospaces,
            Mouse => self.options.mouse = !self.options.mouse,
            CaseSens => self.search.case_sensitive = !self.search.case_sensitive,
            Regexp => self.search.use_regex = !self.search.use_regex,
            Backwards => self.search.backwards = !self.search.backwards,
            _ => {}
        }
        // An action (e.g. Exit with no unsaved changes) may have just
        // closed the last buffer and set Mode::Quit; nothing left to
        // scroll in that case.
        if !self.buffers.is_empty() {
            self.scroll_to_cursor();
        }
    }

    /// Rewrite this buffer's lock file (if any) with the "modified" flag
    /// set, the first time it becomes modified in this session — matching
    /// nano's `set_modified()`, which does the same only on the
    /// false->true transition rather than on every keystroke. Called once
    /// per keystroke handled in the main edit window, since edits happen
    /// via several different paths (`execute()`'s actions, plain character
    /// self-insertion, ...).
    pub fn maybe_update_lock_modified_flag(&mut self) {
        if self.buffers.is_empty() {
            // The action just closed the last buffer (e.g. Exit with no
            // unsaved changes), which already set Mode::Quit; nothing left
            // to update.
            return;
        }
        let target = self.buf().path.as_ref().map(|p| p.display().to_string());
        let buf = self.buf_mut();
        if buf.modified && !buf.lock_modified_written {
            if let (Some(lock), Some(target)) = (&buf.lock_filename, target) {
                let _ = crate::lockfile::write_lock(lock, &target, true);
                buf.lock_modified_written = true;
            }
        }
    }

    /// Close the current buffer (deleting its lock file, if any) and, if it
    /// was the last one, quit — matching nano's normal `close_and_go()`.
    pub fn close_current_buffer(&mut self) {
        if let Some(lock) = self.buf_mut().lock_filename.take() {
            crate::lockfile::delete_lock(&lock);
        }
        self.buffers.remove(self.current);
        if self.buffers.is_empty() {
            self.mode = Mode::Quit;
        } else if self.current >= self.buffers.len() {
            self.current = self.buffers.len() - 1;
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.do_enter();
            return;
        }
        if c == '\t' && self.options.tabstospaces {
            let n = self.options.tabsize as usize;
            for _ in 0..n {
                self.buf_mut().insert_char(' ');
            }
        } else {
            self.buf_mut().insert_char(c);
        }
    }

    fn do_enter(&mut self) {
        let indent = if self.options.autoindent {
            let line = self.buf().line(self.buf().cursor.line);
            line.chars().take_while(|c| *c == ' ' || *c == '\t').collect::<String>()
        } else {
            String::new()
        };
        self.buf_mut().insert_char('\n');
        if !indent.is_empty() {
            self.buf_mut().insert_str(&indent);
        }
    }

    fn selection_range(&self) -> Option<(Pos, Pos)> {
        let buf = self.buf();
        buf.mark.map(|m| {
            if (m.line, m.col) <= (buf.cursor.line, buf.cursor.col) {
                (m, buf.cursor)
            } else {
                (buf.cursor, m)
            }
        })
    }

    fn toggle_mark(&mut self) {
        let cur = self.buf().cursor;
        let buf = self.buf_mut();
        if buf.mark.is_some() {
            buf.mark = None;
        } else {
            buf.mark = Some(cur);
        }
    }

    fn do_cut(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            let text = self.buf_mut().delete_range(start, end);
            self.cutbuffer = text;
            self.buf_mut().mark = None;
        } else {
            let line = self.buf().cursor.line;
            let line_len = self.buf().line(line).chars().count();
            let has_next = line + 1 < self.buf().line_count();
            let end = if has_next { Pos::new(line + 1, 0) } else { Pos::new(line, line_len) };
            let start = Pos::new(line, 0);
            let text = self.buf_mut().delete_range(start, end);
            if self.cut_was_consecutive {
                self.cutbuffer.push_str(&text);
            } else {
                self.cutbuffer = text;
            }
        }
        self.cut_was_consecutive = true;
        self.set_status("Cut");
    }

    fn do_cut_rest_of_file(&mut self) {
        let start = self.buf().cursor;
        let last_line = self.buf().line_count().saturating_sub(1);
        let end = Pos::new(last_line, self.buf().line(last_line).chars().count());
        self.cutbuffer = self.buf_mut().delete_range(start, end);
        self.set_status("Cut to end of file");
    }

    fn do_copy(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            self.cutbuffer = self.buf().text_range(start, end);
        } else {
            let line = self.buf().cursor.line;
            self.cutbuffer = format!("{}\n", self.buf().line(line));
        }
        self.set_status("Copied");
    }

    fn do_paste(&mut self) {
        if self.cutbuffer.is_empty() {
            return;
        }
        let text = self.cutbuffer.clone();
        self.buf_mut().insert_str(&text);
        self.set_status("Pasted");
    }

    fn do_zap(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            self.buf_mut().delete_range(start, end);
            self.buf_mut().mark = None;
            self.set_status("Zapped");
        } else {
            let line = self.buf().cursor.line;
            let has_next = line + 1 < self.buf().line_count();
            let start = Pos::new(line, 0);
            let end = if has_next {
                Pos::new(line + 1, 0)
            } else {
                Pos::new(line, self.buf().line(line).chars().count())
            };
            self.buf_mut().delete_range(start, end);
        }
    }

    fn chop_word_left(&mut self) {
        let end = self.buf().cursor;
        let start = word_left_pos(self.buf(), end);
        self.buf_mut().delete_range(start, end);
    }

    fn chop_word_right(&mut self) {
        let start = self.buf().cursor;
        let end = word_right_pos(self.buf(), start);
        self.buf_mut().delete_range(start, end);
    }

    fn move_prev_word(&mut self) {
        let pos = word_left_pos(self.buf(), self.buf().cursor);
        self.buf_mut().cursor = pos;
    }

    fn move_next_word(&mut self) {
        let pos = word_right_pos(self.buf(), self.buf().cursor);
        self.buf_mut().cursor = pos;
    }

    fn page_up(&mut self) {
        let rows = self.text_rows();
        for _ in 0..rows {
            self.buf_mut().move_up();
        }
    }

    fn page_down(&mut self) {
        let rows = self.text_rows();
        for _ in 0..rows {
            self.buf_mut().move_down();
        }
    }

    fn scroll_view(&mut self, delta: isize) {
        let buf = self.buf_mut();
        if delta < 0 {
            buf.top_line = buf.top_line.saturating_sub((-delta) as usize);
        } else {
            buf.top_line = buf.top_line.saturating_add(delta as usize);
        }
    }

    fn switch_buffer(&mut self, delta: isize) {
        let n = self.buffers.len() as isize;
        if n <= 1 {
            return;
        }
        let cur = self.current as isize;
        self.current = ((cur + delta).rem_euclid(n)) as usize;
    }

    fn report_location(&mut self) {
        let buf = self.buf();
        self.set_status(format!(
            "line {}/{}, col {}",
            buf.cursor.line + 1,
            buf.line_count(),
            buf.cursor.col + 1
        ));
    }

    fn report_word_count(&mut self) {
        let text = self.buf().to_string();
        let words = text.split_whitespace().count();
        let lines = self.buf().line_count();
        let chars = text.chars().count();
        self.set_status(format!("{lines} lines, {words} words, {chars} characters"));
    }

    fn begin_search(&mut self) {
        // The last search term is shown as a bracketed default in the
        // label (see search_prompt_label), not pre-filled into the input -
        // confirmed against the installed nano, which leaves the field
        // empty and reuses the bracketed default only if Enter is pressed
        // with nothing typed.
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WhereIs,
            menu: Menu::Search,
            label: search_prompt_label("Search", "", &self.search),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    fn begin_replace(&mut self) {
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::Replace1,
            menu: Menu::Replace,
            label: search_prompt_label("Search", " (to replace)", &self.search),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    fn begin_goto_line(&mut self) {
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::GotoLine,
            menu: Menu::GotoLine,
            label: "Enter line number, column number".to_string(),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    fn begin_writeout(&mut self, exiting: bool) {
        let default = self
            .buf()
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WriteOut { exiting },
            menu: Menu::WriteOut,
            label: "File Name to Write".to_string(),
            cursor: default.chars().count(),
            input: default,
            history_pos: None,
            saved_input: None,
        });
    }

    fn quick_save(&mut self) {
        if let Some(path) = self.buf().path.clone() {
            match crate::fileio::save_file(self.buf_mut(), &path) {
                Ok(()) => self.set_status(format!("Wrote {}", path.display())),
                Err(e) => self.set_status(format!("Error writing {}: {e}", path.display())),
            }
        } else {
            self.begin_writeout(false);
        }
    }

    fn begin_exit(&mut self) {
        if self.buf().modified {
            self.mode = Mode::Prompt(Prompt {
                kind: PromptKind::Exit { discard_and_quit: false },
                menu: Menu::YesNo,
                label: format!(
                    "Save modified buffer{}? ",
                    self.buf().path.as_ref().map(|p| format!(" ({})", p.display())).unwrap_or_default()
                ),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        } else {
            self.close_current_buffer();
        }
    }

    fn repeat_search(&mut self, backwards: bool) {
        let Some(pattern) = self.search.last_pattern.clone() else {
            self.set_status("No search pattern in memory");
            return;
        };
        self.run_search(&pattern, backwards);
    }

    pub fn begin_writeout_for_exit(&mut self) {
        self.begin_writeout(true);
    }

    /// Kick off a three-way-merge preview for the current buffer against
    /// its on-disk contents, using the buffer's originally-loaded content
    /// as the merge base.
    pub fn begin_merge_preview(&mut self) {
        let Some(path) = self.buf().path.clone() else {
            self.mode = Mode::Editing;
            return;
        };
        let Ok(theirs) = std::fs::read_to_string(&path) else {
            self.set_status("Could not re-read file from disk");
            self.mode = Mode::Editing;
            return;
        };
        let base = self.buf().original_content.clone();
        let ours = self.buf().to_string();
        match crate::fileio::three_way_merge(&base, &ours, &theirs) {
            crate::fileio::MergeResult::Clean { text, diff } => {
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::MergePreviewClean { merged_text: text },
                    menu: Menu::YesNo,
                    label: format!("{diff}\n[A]pply merge  [C]ancel"),
                    input: String::new(),
                    cursor: 0,
                    history_pos: None,
                    saved_input: None,
                });
            }
            crate::fileio::MergeResult::Conflict { diff } => {
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::MergeConflict,
                    menu: Menu::YesNo,
                    label: format!("{diff}\nCould not merge automatically (press any key)"),
                    input: String::new(),
                    cursor: 0,
                    history_pos: None,
                    saved_input: None,
                });
            }
        }
    }

    /// Kick off an interactive replace: find the first match (from the
    /// current cursor, wrapping once around the buffer) and, if found,
    /// open the "Replace this instance?" confirmation prompt. Matches
    /// nano's do_replace() / do_replace_loop().
    pub fn begin_replace_loop(&mut self, search: String, replacement: String) {
        if search.is_empty() {
            self.mode = Mode::Editing;
            return;
        }
        self.search.last_pattern = Some(search.clone());
        let session_start = self.buf().cursor;
        let found = find_next_match_for_replace(
            self.buf(),
            session_start,
            session_start,
            false,
            &search,
            self.search.case_sensitive,
            self.search.use_regex,
        );
        match found {
            Ok(Some((pos, len, wrapped))) => {
                self.buf_mut().cursor = pos;
                self.scroll_to_cursor_centered();
                self.set_spotlight_persistent(pos, len);
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::ReplaceConfirm(ReplaceLoopState {
                        search,
                        replacement,
                        match_pos: pos,
                        match_len: len,
                        session_start,
                        wrapped,
                        count: 0,
                    }),
                    menu: Menu::YesNo,
                    label: "Replace this instance?".to_string(),
                    input: String::new(),
                    cursor: 0,
                    history_pos: None,
                    saved_input: None,
                });
            }
            Ok(None) => {
                self.mode = Mode::Editing;
                self.clear_spotlight();
                self.set_status(format!("\"{search}\" not found"));
            }
            Err(e) => {
                self.mode = Mode::Editing;
                self.clear_spotlight();
                self.set_status(format!("Invalid regex: {e}"));
            }
        }
    }

    /// Act on the user's Yes/No/All/Cancel answer for the current match,
    /// then either advance to the next one (opening a fresh confirmation
    /// prompt) or finish the loop.
    pub fn replace_choice(&mut self, mut state: ReplaceLoopState, choice: ReplaceChoice) {
        if matches!(choice, ReplaceChoice::Cancel) {
            self.mode = Mode::Editing;
            self.clear_spotlight();
            self.report_replace_count(state.count);
            return;
        }
        let mut do_replace = matches!(choice, ReplaceChoice::Yes | ReplaceChoice::All);
        let replace_all = matches!(choice, ReplaceChoice::All);
        loop {
            let next_from = if do_replace {
                let expanded = self.expand_replacement(&state.search, &state.replacement, state.match_pos, state.match_len);
                let end = Pos::new(state.match_pos.line, state.match_pos.col + state.match_len);
                self.buf_mut().delete_range(state.match_pos, end);
                self.buf_mut().cursor = state.match_pos;
                let expanded_len = expanded.chars().count();
                self.buf_mut().insert_str(&expanded);
                state.count += 1;
                Pos::new(state.match_pos.line, state.match_pos.col + expanded_len)
            } else {
                // Skip past this match (at least one character, so a
                // zero-length regex match can't be found again forever).
                Pos::new(state.match_pos.line, state.match_pos.col + state.match_len.max(1))
            };
            let found = find_next_match_for_replace(
                self.buf(),
                next_from,
                state.session_start,
                state.wrapped,
                &state.search,
                self.search.case_sensitive,
                self.search.use_regex,
            );
            match found {
                Ok(Some((pos, len, wrapped))) => {
                    state.match_pos = pos;
                    state.match_len = len;
                    state.wrapped = wrapped;
                    if replace_all {
                        do_replace = true;
                        continue;
                    }
                    self.buf_mut().cursor = pos;
                    self.scroll_to_cursor_centered();
                    self.set_spotlight_persistent(pos, len);
                    self.mode = Mode::Prompt(Prompt {
                        kind: PromptKind::ReplaceConfirm(state),
                        menu: Menu::YesNo,
                        label: "Replace this instance?".to_string(),
                        input: String::new(),
                        cursor: 0,
                        history_pos: None,
                        saved_input: None,
                    });
                    return;
                }
                Ok(None) => {
                    self.mode = Mode::Editing;
                    self.clear_spotlight();
                    self.report_replace_count(state.count);
                    return;
                }
                Err(e) => {
                    self.mode = Mode::Editing;
                    self.clear_spotlight();
                    self.set_status(format!("Invalid regex: {e}"));
                    return;
                }
            }
        }
    }

    fn report_replace_count(&mut self, count: usize) {
        match count {
            0 => self.set_status("No replacements made"),
            1 => self.set_status("Replaced 1 occurrence"),
            n => self.set_status(format!("Replaced {n} occurrences")),
        }
    }

    /// Build the literal text to insert for one match: for a regex search,
    /// expands nano-style `\1`-`\9` backreferences (verified against
    /// nano's `replace_regexp()` in src/search.c); a literal-string search
    /// uses the replacement text as-is, with no backreference processing —
    /// nano does the same (`replace_line()` only calls `replace_regexp()`
    /// when `ISSET(USE_REGEXP)`).
    fn expand_replacement(&self, search: &str, replacement: &str, match_pos: Pos, match_len: usize) -> String {
        if !self.search.use_regex {
            return replacement.to_string();
        }
        let pat = if self.search.case_sensitive { search.to_string() } else { format!("(?i){search}") };
        let Ok(re) = regex::Regex::new(&pat) else { return replacement.to_string() };
        let line_chars: Vec<char> = self.buf().line(match_pos.line).chars().collect();
        let end = (match_pos.col + match_len).min(line_chars.len());
        let matched_text: String = line_chars[match_pos.col..end].iter().collect();
        let Some(caps) = re.captures(&matched_text) else { return replacement.to_string() };
        expand_backreferences(replacement, &caps)
    }

    pub fn run_search(&mut self, pattern: &str, backwards: bool) {
        if pattern.is_empty() {
            return;
        }
        let text = self.buf().to_string();
        let hay: Vec<&str> = text.split_inclusive('\n').collect();
        let found = find_in_lines(&hay, self.buf().cursor, pattern, backwards, self.search.case_sensitive, self.search.use_regex);
        match found {
            Some((pos, len)) => {
                self.buf_mut().cursor = pos;
                self.scroll_to_cursor_centered();
                self.search.last_pattern = Some(pattern.to_string());
                self.set_spotlight_timed(pos, len);
            }
            None => self.set_status(format!("\"{pattern}\" not found")),
        }
    }
}

fn word_left_pos(buf: &Buffer, from: Pos) -> Pos {
    let mut line = from.line;
    let mut chars: Vec<char> = buf.line(line).chars().collect();
    let mut col = from.col;
    loop {
        while col > 0 && !chars[col - 1].is_alphanumeric() && chars[col - 1] != '_' {
            col -= 1;
        }
        while col > 0 && (chars[col - 1].is_alphanumeric() || chars[col - 1] == '_') {
            col -= 1;
        }
        if col > 0 || line == 0 {
            return Pos::new(line, col);
        }
        line -= 1;
        chars = buf.line(line).chars().collect();
        col = chars.len();
        if col == 0 {
            return Pos::new(line, 0);
        }
    }
}

fn word_right_pos(buf: &Buffer, from: Pos) -> Pos {
    let mut line = from.line;
    let mut chars: Vec<char> = buf.line(line).chars().collect();
    let mut col = from.col;
    loop {
        while col < chars.len() && (chars[col].is_alphanumeric() || chars[col] == '_') {
            col += 1;
        }
        while col < chars.len() && !chars[col].is_alphanumeric() && chars[col] != '_' {
            col += 1;
        }
        if col < chars.len() || line + 1 >= buf.line_count() {
            return Pos::new(line, col);
        }
        line += 1;
        chars = buf.line(line).chars().collect();
        col = 0;
        if chars.is_empty() {
            return Pos::new(line, 0);
        }
    }
}

/// Build a Search/Replace prompt's label the way nano does: the base text,
/// then a bracketed flag for each active toggle in this exact order —
/// `[Case Sensitive]`, `[Regexp]`, `[Backwards]` — then an optional suffix
/// like `" (to replace)"`. Confirmed against the installed nano's actual
/// prompt text (e.g. `Search [Case Sensitive] [Regexp] (to replace):`).
pub fn search_prompt_label(base: &str, suffix: &str, search: &SearchState) -> String {
    let mut label = base.to_string();
    if search.case_sensitive {
        label.push_str(" [Case Sensitive]");
    }
    if search.use_regex {
        label.push_str(" [Regexp]");
    }
    if search.backwards {
        label.push_str(" [Backwards]");
    }
    label.push_str(suffix);
    // The remembered last search term is shown in brackets at the very
    // end, after any suffix (e.g. "Search [Case Sensitive] (to replace)
    // [apple]:") - confirmed against the installed nano's exact wording.
    // Pressing Enter with nothing typed reuses this as the search text.
    if let Some(default) = &search.last_pattern {
        if !default.is_empty() {
            label.push_str(&format!(" [{default}]"));
        }
    }
    label
}

/// Expand nano-style `\1`-`\9` backreferences in `template` using `caps`
/// (a valid group number that didn't participate in the match expands to
/// nothing; a `\` followed by anything else — including a digit that isn't
/// a valid group number for this pattern — is copied through literally).
fn expand_backreferences(template: &str, caps: &regex::Captures) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() && chars[i + 1] != '0' {
            let n = chars[i + 1].to_digit(10).unwrap() as usize;
            if n < caps.len() {
                if let Some(m) = caps.get(n) {
                    out.push_str(m.as_str());
                }
                i += 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Find the next match of `pattern` at or after `from`, wrapping around the
/// buffer once (but not past `session_start`, if already wrapped) —
/// matches nano's search wraparound ("came_full_circle") so a replace loop
/// can't repeat forever. Returns (match position, match length in chars,
/// whether the search has now wrapped).
#[allow(clippy::too_many_arguments)]
fn find_next_match_for_replace(
    buf: &Buffer,
    from: Pos,
    session_start: Pos,
    already_wrapped: bool,
    pattern: &str,
    case_sensitive: bool,
    use_regex: bool,
) -> Result<Option<(Pos, usize, bool)>, String> {
    let re = if use_regex {
        let pat = if case_sensitive { pattern.to_string() } else { format!("(?i){pattern}") };
        Some(regex::Regex::new(&pat).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let matches_at = |line: &str| -> Vec<(usize, usize)> {
        if let Some(re) = &re {
            re.find_iter(line)
                .map(|m| (line[..m.start()].chars().count(), line[m.start()..m.end()].chars().count()))
                .collect()
        } else if case_sensitive {
            line.char_indices()
                .filter(|(i, _)| line[*i..].starts_with(pattern))
                .map(|(i, _)| (line[..i].chars().count(), pattern.chars().count()))
                .collect()
        } else {
            let lower_line = line.to_lowercase();
            let lower_needle = pattern.to_lowercase();
            lower_line
                .char_indices()
                .filter(|(i, _)| lower_line[*i..].starts_with(&lower_needle))
                .map(|(i, _)| (lower_line[..i].chars().count(), lower_needle.chars().count()))
                .collect()
        }
    };

    let n = buf.line_count();
    for line_idx in from.line..n {
        let raw = buf.line(line_idx);
        for (c, len) in matches_at(&raw) {
            if line_idx != from.line || c >= from.col {
                return Ok(Some((Pos::new(line_idx, c), len, already_wrapped)));
            }
        }
    }
    if already_wrapped {
        return Ok(None);
    }
    for line_idx in 0..=session_start.line.min(n.saturating_sub(1)) {
        let raw = buf.line(line_idx);
        for (c, len) in matches_at(&raw) {
            if line_idx < session_start.line || c < session_start.col {
                return Ok(Some((Pos::new(line_idx, c), len, true)));
            }
        }
    }
    Ok(None)
}

fn find_in_lines(
    lines: &[&str],
    from: Pos,
    pattern: &str,
    backwards: bool,
    case_sensitive: bool,
    use_regex: bool,
) -> Option<(Pos, usize)> {
    let re = if use_regex {
        let pat = if case_sensitive { pattern.to_string() } else { format!("(?i){pattern}") };
        regex::Regex::new(&pat).ok()
    } else {
        None
    };
    let matches_at = |line: &str, needle: &str| -> Vec<(usize, usize)> {
        if let Some(re) = &re {
            re.find_iter(line)
                .map(|m| (line[..m.start()].chars().count(), line[m.start()..m.end()].chars().count()))
                .collect()
        } else if case_sensitive {
            line.char_indices()
                .filter(|(i, _)| line[*i..].starts_with(needle))
                .map(|(i, _)| (line[..i].chars().count(), needle.chars().count()))
                .collect()
        } else {
            let lower_line = line.to_lowercase();
            let lower_needle = needle.to_lowercase();
            lower_line
                .char_indices()
                .filter(|(i, _)| lower_line[*i..].starts_with(&lower_needle))
                .map(|(i, _)| (lower_line[..i].chars().count(), lower_needle.chars().count()))
                .collect()
        }
    };

    let n = lines.len();
    let order: Vec<usize> = if backwards {
        (0..n).rev().collect()
    } else {
        (0..n).collect()
    };
    // Rotate so we start searching from the current line, wrapping around.
    let start_idx = order.iter().position(|&l| l == from.line).unwrap_or(0);
    let rotated = order[start_idx..].iter().chain(order[..start_idx].iter());

    for &line_idx in rotated {
        let raw = lines[line_idx].trim_end_matches(['\n', '\r']);
        let mut cols = matches_at(raw, pattern);
        if !backwards {
            cols.retain(|&(c, _)| line_idx != from.line || c > from.col);
        } else {
            cols.reverse();
            cols.retain(|&(c, _)| line_idx != from.line || c < from.col);
        }
        if let Some(&(c, len)) = cols.first() {
            return Some((Pos::new(line_idx, c), len));
        }
    }
    None
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyMap;
    use crate::options::Options;

    fn test_editor(text: &str) -> Editor {
        let mut ed = Editor::new(Options::default(), KeyMap::new());
        ed.buffers[0] = Buffer::from_text(text, None);
        ed
    }

    #[test]
    fn exiting_the_last_unmodified_buffer_does_not_panic() {
        // execute() used to end with an unconditional scroll_to_cursor(),
        // which - like a couple of other post-action steps - assumed there
        // was always still a buffer to look at. Exiting with nothing to
        // save closes the last buffer and sets Mode::Quit in the same
        // call, leaving `buffers` empty.
        let mut ed = test_editor("hello");
        ed.execute(Action::Exit);
        assert!(matches!(ed.mode, Mode::Quit));
        assert!(ed.buffers.is_empty());
    }

    #[test]
    fn lock_flag_update_on_empty_buffers_does_not_panic() {
        let mut ed = test_editor("hello");
        ed.buffers.clear();
        ed.maybe_update_lock_modified_flag(); // must not panic
    }

    #[test]
    fn backreference_expansion_basic() {
        let re = regex::Regex::new(r"(\w+)@(\w+)").unwrap();
        let caps = re.captures("alice@example").unwrap();
        assert_eq!(expand_backreferences(r"\2:\1", &caps), "example:alice");
    }

    #[test]
    fn backreference_nonparticipating_group_is_empty() {
        let re = regex::Regex::new(r"(a)|(b)").unwrap();
        let caps = re.captures("b").unwrap();
        assert_eq!(expand_backreferences(r"[\1][\2]", &caps), "[][b]");
    }

    #[test]
    fn backreference_out_of_range_is_literal() {
        let re = regex::Regex::new(r"(a)").unwrap();
        let caps = re.captures("a").unwrap();
        // Only group 1 exists; \5 isn't a valid group so stays literal.
        assert_eq!(expand_backreferences(r"\5-\1", &caps), r"\5-a");
    }

    #[test]
    fn search_scrolls_offscreen_match_into_view() {
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24; // text_rows() ~= 20 with default title/status/help rows
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.run_search("line45".to_string().as_str(), false);
        assert_eq!(ed.buf().cursor.line, 45);
        let rows = ed.text_rows();
        assert!(
            ed.buf().top_line <= 45 && 45 < ed.buf().top_line + rows,
            "match at line 45 should be within the scrolled viewport (top_line={}, rows={rows})",
            ed.buf().top_line
        );
    }

    #[test]
    fn search_centers_offscreen_match_like_nano() {
        // nano's edit_redraw(..., CENTERING) puts the match at row
        // editwinrows/2 rather than just barely scrolling it into view -
        // confirmed directly against the installed nano.
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24;
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.run_search("line45".to_string().as_str(), false);
        let rows = ed.text_rows();
        assert_eq!(ed.buf().top_line, 45 - rows / 2);
    }

    #[test]
    fn search_does_not_scroll_when_match_already_visible() {
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24;
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.run_search("line3".to_string().as_str(), false); // well within the first screenful
        assert_eq!(ed.buf().top_line, 0, "already-visible match shouldn't move the viewport");
    }

    #[test]
    fn config_options_seed_initial_search_state() {
        let mut opts = Options::default();
        opts.casesensitive = true;
        opts.regexp = true;
        let ed = Editor::new(opts, KeyMap::new());
        assert!(ed.search.case_sensitive);
        assert!(ed.search.use_regex);
    }

    #[test]
    fn replace_all_literal_case_insensitive() {
        let mut ed = test_editor("Foo bar foo BAR foo");
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.begin_replace_loop("foo".to_string(), "X".to_string());
        // First match should be found and awaiting confirmation.
        let Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(state), .. }) = &ed.mode else {
            panic!("expected a replace-confirm prompt");
        };
        assert_eq!(state.match_pos, Pos::new(0, 0));
        let state = state.clone();
        ed.replace_choice(state, ReplaceChoice::All);
        assert_eq!(ed.buf().to_string(), "X bar X BAR X");
        assert!(matches!(ed.mode, Mode::Editing));
    }

    #[test]
    fn replace_yes_no_skips_and_replaces_selectively() {
        let mut ed = test_editor("cat cat cat");
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.begin_replace_loop("cat".to_string(), "dog".to_string());
        let state = match &ed.mode {
            Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(s), .. }) => s.clone(),
            _ => panic!("expected prompt"),
        };
        ed.replace_choice(state, ReplaceChoice::No); // skip first "cat"
        let state = match &ed.mode {
            Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(s), .. }) => s.clone(),
            _ => panic!("expected prompt after No"),
        };
        ed.replace_choice(state, ReplaceChoice::Yes); // replace second "cat"
        assert_eq!(ed.buf().to_string(), "cat dog cat");
        let state = match &ed.mode {
            Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(s), .. }) => s.clone(),
            _ => panic!("expected prompt after Yes"),
        };
        ed.replace_choice(state, ReplaceChoice::Cancel);
        assert_eq!(ed.buf().to_string(), "cat dog cat");
        assert!(matches!(ed.mode, Mode::Editing));
    }

    #[test]
    fn replace_regex_with_backreferences() {
        let mut ed = test_editor("2024-01-15");
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.search.use_regex = true;
        ed.begin_replace_loop(r"(\d+)-(\d+)-(\d+)".to_string(), r"\3/\2/\1".to_string());
        let state = match &ed.mode {
            Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(s), .. }) => s.clone(),
            _ => panic!("expected prompt"),
        };
        ed.replace_choice(state, ReplaceChoice::Yes);
        assert_eq!(ed.buf().to_string(), "15/01/2024");
    }

    #[test]
    fn replace_wraps_around_once_then_stops() {
        // Cursor starts on the second "x"; with wraparound both should be
        // found (in order: second, then first on wrap), but not forever.
        let mut ed = test_editor("x y x");
        ed.buf_mut().cursor = Pos::new(0, 4); // at the second 'x'
        ed.begin_replace_loop("x".to_string(), "Z".to_string());
        let state = match &ed.mode {
            Mode::Prompt(Prompt { kind: PromptKind::ReplaceConfirm(s), .. }) => s.clone(),
            _ => panic!("expected prompt"),
        };
        assert_eq!(state.match_pos, Pos::new(0, 4));
        ed.replace_choice(state, ReplaceChoice::All);
        assert_eq!(ed.buf().to_string(), "Z y Z");
        assert!(matches!(ed.mode, Mode::Editing));
    }

    #[test]
    fn replace_not_found_reports_status() {
        let mut ed = test_editor("hello world");
        ed.begin_replace_loop("zzz".to_string(), "y".to_string());
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(ed.status.as_deref(), Some("\"zzz\" not found"));
    }

    #[test]
    fn invalid_regex_reports_status_not_panic() {
        let mut ed = test_editor("hello world");
        ed.search.use_regex = true;
        ed.begin_replace_loop("(unclosed".to_string(), "y".to_string());
        assert!(matches!(ed.mode, Mode::Editing));
        assert!(ed.status.as_deref().unwrap_or("").starts_with("Invalid regex"));
    }

    #[test]
    fn short_line_never_scrolls_horizontally() {
        let mut ed = test_editor("short");
        ed.screen_cols = 60;
        ed.buf_mut().cursor.col = 5;
        ed.scroll_to_cursor();
        assert_eq!(ed.buf().left_col, 0);
    }

    #[test]
    fn long_line_scrolls_to_keep_cursor_visible() {
        // Regression test: the first cut of this formula underflowed
        // (`cursor_col - width + CUSHION + 1` computed left-to-right in
        // usize) for exactly this kind of case, panicking in a debug
        // build the moment the cursor crossed the scroll threshold.
        let mut ed = test_editor(&"x".repeat(200));
        ed.screen_cols = 60;
        for _ in 0..65 {
            ed.execute(Action::Right);
        }
        assert_eq!(ed.buf().cursor.col, 65);
        assert!(ed.buf().left_col > 0, "cursor at column 65 in a 60-wide view should have scrolled");
        // The cursor itself must still land within the visible window
        // (leaving room for the '<' marker its scroll implies).
        assert!(
            ed.buf().cursor.col > ed.buf().left_col && ed.buf().cursor.col < ed.buf().left_col + ed.screen_cols,
            "cursor (col={}) should be inside the scrolled window (left_col={})",
            ed.buf().cursor.col,
            ed.buf().left_col
        );
    }

    #[test]
    fn scroll_resets_when_cursor_returns_near_start() {
        let mut ed = test_editor(&"x".repeat(200));
        ed.screen_cols = 60;
        for _ in 0..65 {
            ed.execute(Action::Right);
        }
        assert!(ed.buf().left_col > 0);
        ed.buf_mut().cursor.col = 0;
        ed.scroll_to_cursor();
        assert_eq!(ed.buf().left_col, 0);
    }

    #[test]
    fn narrow_screen_does_not_panic() {
        // width <= 2*CUSHION+1 takes the "too narrow to cushion" fallback;
        // make sure it doesn't underflow either.
        let mut ed = test_editor(&"x".repeat(50));
        ed.screen_cols = 3;
        for _ in 0..20 {
            ed.execute(Action::Right); // must not panic
        }
    }
}

