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
    ReplaceConfirm { search: String, replacement: String, regex: bool },
    GotoLine,
    WriteOut { exiting: bool },
    Exit { discard_and_quit: bool },
    ExternalChangeConflict,
    MergeConflict,
    MergePreviewClean { merged_text: String },
    Help,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub menu: Menu,
    pub label: String,
    pub input: String,
    pub cursor: usize,
}

pub enum Mode {
    Editing,
    Prompt(Prompt),
    Help(Vec<String>),
    Quit,
}

/// Severity of a status-bar message, matching the subset of nano's message
/// importance levels (src/prototypes.h: HUSH/REMARK/NOTICE/MILD/AHEM/ALERT)
/// that affect rendering here: most messages are `Normal` (nano's default
/// STATUS_BAR color, reverse video); errors like "is a directory" or "is
/// unwritable" are `Alert` (nano's ERROR_MESSAGE color, bold white-on-red,
/// plus a bell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusLevel {
    #[default]
    Normal,
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
    pub mode: Mode,
    pub screen_rows: usize,
    pub screen_cols: usize,
}

impl Editor {
    pub fn new(options: Options, keymap: crate::keymap::KeyMap) -> Editor {
        Editor {
            buffers: vec![Buffer::empty()],
            current: 0,
            options,
            keymap,
            cutbuffer: String::new(),
            cut_was_consecutive: false,
            search: SearchState::default(),
            status: None,
            status_level: StatusLevel::Normal,
            status_countdown: 0,
            bell_pending: false,
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

    pub fn scroll_to_cursor(&mut self) {
        let rows = self.text_rows();
        let buf = self.buf_mut();
        if buf.cursor.line < buf.top_line {
            buf.top_line = buf.cursor.line;
        } else if buf.cursor.line >= buf.top_line + rows {
            buf.top_line = buf.cursor.line + 1 - rows;
        }
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
                self.mode = Mode::Help(help_text());
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
        self.scroll_to_cursor();
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
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WhereIs,
            menu: Menu::Search,
            label: if self.search.backwards { "Search backward" } else { "Search" }.to_string(),
            input: self.search.last_pattern.clone().unwrap_or_default(),
            cursor: 0,
        });
    }

    fn begin_replace(&mut self) {
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::Replace1,
            menu: Menu::Replace,
            label: "Search (to replace)".to_string(),
            input: String::new(),
            cursor: 0,
        });
    }

    fn begin_goto_line(&mut self) {
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::GotoLine,
            menu: Menu::GotoLine,
            label: "Enter line number, column number".to_string(),
            input: String::new(),
            cursor: 0,
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
            });
        } else if self.buffers.len() > 1 {
            self.buffers.remove(self.current);
            if self.current >= self.buffers.len() {
                self.current = self.buffers.len() - 1;
            }
        } else {
            self.mode = Mode::Quit;
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
                });
            }
            crate::fileio::MergeResult::Conflict { diff } => {
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::MergeConflict,
                    menu: Menu::YesNo,
                    label: format!("{diff}\nCould not merge automatically (press any key)"),
                    input: String::new(),
                    cursor: 0,
                });
            }
        }
    }

    /// Replace every occurrence of `search` with `replacement` in the
    /// buffer, respecting the active case-sensitivity/regex search options.
    pub fn do_replace_all(&mut self, search: &str, replacement: &str) {
        if search.is_empty() {
            return;
        }
        let text = self.buf().to_string();
        let new_text = if self.search.use_regex {
            let pat = if self.search.case_sensitive { search.to_string() } else { format!("(?i){search}") };
            match regex::Regex::new(&pat) {
                Ok(re) => re.replace_all(&text, replacement).into_owned(),
                Err(e) => {
                    self.set_status(format!("Invalid regex: {e}"));
                    return;
                }
            }
        } else if self.search.case_sensitive {
            text.replace(search, replacement)
        } else {
            replace_case_insensitive(&text, search, replacement)
        };
        let count = if self.search.use_regex {
            0 // exact count not tracked for the regex path; message kept generic below
        } else {
            text.matches(search).count()
        };
        let cursor = self.buf().cursor;
        self.buf_mut().rope = ropey::Rope::from_str(&new_text);
        self.buf_mut().modified = true;
        self.buf_mut().cursor = cursor;
        let max_line = self.buf().line_count().saturating_sub(1);
        self.buf_mut().cursor.line = self.buf().cursor.line.min(max_line);
        if count > 0 {
            self.set_status(format!("Replaced {count} occurrence(s)"));
        } else {
            self.set_status("Replacement complete");
        }
        self.search.last_pattern = Some(search.to_string());
    }

    pub fn run_search(&mut self, pattern: &str, backwards: bool) {
        if pattern.is_empty() {
            return;
        }
        let text = self.buf().to_string();
        let hay: Vec<&str> = text.split_inclusive('\n').collect();
        let found = find_in_lines(&hay, self.buf().cursor, pattern, backwards, self.search.case_sensitive, self.search.use_regex);
        match found {
            Some(pos) => {
                self.buf_mut().cursor = pos;
                self.search.last_pattern = Some(pattern.to_string());
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

fn find_in_lines(
    lines: &[&str],
    from: Pos,
    pattern: &str,
    backwards: bool,
    case_sensitive: bool,
    use_regex: bool,
) -> Option<Pos> {
    let re = if use_regex {
        let pat = if case_sensitive { pattern.to_string() } else { format!("(?i){pattern}") };
        regex::Regex::new(&pat).ok()
    } else {
        None
    };
    let matches_at = |line: &str, needle: &str| -> Vec<usize> {
        if let Some(re) = &re {
            re.find_iter(line).map(|m| line[..m.start()].chars().count()).collect()
        } else if case_sensitive {
            line.char_indices()
                .filter(|(i, _)| line[*i..].starts_with(needle))
                .map(|(i, _)| line[..i].chars().count())
                .collect()
        } else {
            let lower_line = line.to_lowercase();
            let lower_needle = needle.to_lowercase();
            lower_line
                .char_indices()
                .filter(|(i, _)| lower_line[*i..].starts_with(&lower_needle))
                .map(|(i, _)| lower_line[..i].chars().count())
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
            cols.retain(|&c| line_idx != from.line || c > from.col);
        } else {
            cols.reverse();
            cols.retain(|&c| line_idx != from.line || c < from.col);
        }
        if let Some(&c) = cols.first() {
            return Some(Pos::new(line_idx, c));
        }
    }
    None
}

/// Case-insensitive replace-all, implemented char-by-char (rather than by
/// slicing a separately-lowercased copy) so it can't panic on the rare
/// characters whose lowercase form has a different UTF-8 byte length.
fn replace_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let needle_lower: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    let hay: Vec<char> = haystack.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < hay.len() {
        let mut hi = i;
        let mut matched = true;
        for &nc in &needle_lower {
            if hi >= hay.len() {
                matched = false;
                break;
            }
            let mut hc_lower = hay[hi].to_lowercase();
            if hc_lower.next() != Some(nc) || hc_lower.next().is_some() {
                matched = false;
                break;
            }
            hi += 1;
        }
        if matched {
            out.push_str(replacement);
            i = hi;
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn help_text() -> Vec<String> {
    vec![
        "tico help".to_string(),
        "".to_string(),
        "tico is a nano-compatible text editor.".to_string(),
        "Press ^X to close this help.".to_string(),
    ]
}
