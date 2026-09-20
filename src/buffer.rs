//! A single edited file: its text (as a rope), cursor/mark, undo/redo
//! history, and the on-disk metadata needed to detect external changes.

use ropey::Rope;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize, // character offset within the line (not display width)
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }
}

/// One undoable edit: replacing the text in `[start, end)` (in the buffer
/// *before* the edit) with `inserted`. Undo restores `removed` at `start`;
/// redo re-applies `inserted`.
#[derive(Debug, Clone)]
pub struct Edit {
    pub start_char: usize,
    pub removed: String,
    pub inserted: String,
    pub cursor_before: Pos,
    pub cursor_after: Pos,
}

/// Snapshot of a file's on-disk state at the time it was last loaded or
/// saved by us, used to detect concurrent external modification.
#[derive(Debug, Clone)]
pub struct DiskState {
    pub mtime: Option<SystemTime>,
    pub len: u64,
    pub content_hash: u64,
}

pub struct Buffer {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub cursor: Pos,
    pub mark: Option<Pos>,
    pub modified: bool,
    pub undo_stack: Vec<Edit>,
    pub redo_stack: Vec<Edit>,
    pub disk_state: Option<DiskState>,
    /// Original content as loaded, used as the merge base for three-way
    /// merges when the file changes on disk while we have local edits.
    pub original_content: String,
    pub top_line: usize,
    pub language: Option<String>,
    /// Remembered column for consecutive up/down movement through shorter
    /// lines, reset by any horizontal movement or edit (as in nano).
    goal_col: Option<usize>,
}

impl Buffer {
    pub fn empty() -> Buffer {
        Buffer {
            rope: Rope::new(),
            path: None,
            cursor: Pos::new(0, 0),
            mark: None,
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            disk_state: None,
            original_content: String::new(),
            top_line: 0,
            language: None,
            goal_col: None,
        }
    }

    pub fn from_text(text: &str, path: Option<PathBuf>) -> Buffer {
        let mut b = Buffer::empty();
        b.rope = Rope::from_str(text);
        b.original_content = text.to_string();
        b.path = path;
        b
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line(&self, idx: usize) -> String {
        if idx >= self.rope.len_lines() {
            return String::new();
        }
        let s = self.rope.line(idx).to_string();
        s.trim_end_matches(['\n', '\r']).to_string()
    }

    fn char_idx(&self, pos: Pos) -> usize {
        let line_start = self.rope.line_to_char(pos.line.min(self.rope.len_lines().saturating_sub(1)));
        line_start + pos.col
    }

    fn clamp_pos(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.rope.len_lines().saturating_sub(1));
        let line_len = self.line(line).chars().count();
        Pos::new(line, pos.col.min(line_len))
    }

    /// Replace `[start,end)` with `text`, recording an undo entry. Returns
    /// the new cursor position (end of the inserted text).
    fn replace_range(&mut self, start: Pos, end: Pos, text: &str, cursor_after: Pos) {
        let start_c = self.char_idx(start);
        let end_c = self.char_idx(end);
        let removed: String = self.rope.slice(start_c..end_c).to_string();
        self.rope.remove(start_c..end_c);
        self.rope.insert(start_c, text);
        self.undo_stack.push(Edit {
            start_char: start_c,
            removed,
            inserted: text.to_string(),
            cursor_before: self.cursor,
            cursor_after,
        });
        self.redo_stack.clear();
        self.modified = true;
        self.cursor = cursor_after;
        self.goal_col = None;
    }

    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor;
        let mut s = String::new();
        s.push(c);
        let after = if c == '\n' {
            Pos::new(pos.line + 1, 0)
        } else {
            Pos::new(pos.line, pos.col + 1)
        };
        self.replace_range(pos, pos, &s, after);
    }

    pub fn insert_str(&mut self, text: &str) {
        let pos = self.cursor;
        let newlines = text.matches('\n').count();
        let after = if newlines == 0 {
            Pos::new(pos.line, pos.col + text.chars().count())
        } else {
            let last_line_len = text.rsplit('\n').next().unwrap_or("").chars().count();
            Pos::new(pos.line + newlines, last_line_len)
        };
        self.replace_range(pos, pos, text, after);
    }

    pub fn backspace(&mut self) {
        let pos = self.cursor;
        if pos.col == 0 && pos.line == 0 {
            return;
        }
        let before = if pos.col == 0 {
            let prev_len = self.line(pos.line - 1).chars().count();
            Pos::new(pos.line - 1, prev_len)
        } else {
            Pos::new(pos.line, pos.col - 1)
        };
        self.replace_range(before, pos, "", before);
    }

    pub fn delete_forward(&mut self) {
        let pos = self.cursor;
        let line_len = self.line(pos.line).chars().count();
        let after_pos = if pos.col >= line_len {
            if pos.line + 1 >= self.rope.len_lines() {
                return;
            }
            Pos::new(pos.line + 1, 0)
        } else {
            Pos::new(pos.line, pos.col + 1)
        };
        self.replace_range(pos, after_pos, "", pos);
    }

    /// Cut and return the text of a line range (used by cut-line and
    /// cut-marked-region).
    pub fn delete_range(&mut self, start: Pos, end: Pos) -> String {
        let (start, end) = if (start.line, start.col) <= (end.line, end.col) {
            (start, end)
        } else {
            (end, start)
        };
        let start_c = self.char_idx(start);
        let end_c = self.char_idx(end);
        let removed = self.rope.slice(start_c..end_c).to_string();
        self.replace_range(start, end, "", start);
        removed
    }

    pub fn text_range(&self, start: Pos, end: Pos) -> String {
        let (start, end) = if (start.line, start.col) <= (end.line, end.col) {
            (start, end)
        } else {
            (end, start)
        };
        let start_c = self.char_idx(start);
        let end_c = self.char_idx(end);
        self.rope.slice(start_c..end_c).to_string()
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo_stack.pop() else { return false };
        let inserted_len = edit.inserted.chars().count();
        self.rope.remove(edit.start_char..edit.start_char + inserted_len);
        self.rope.insert(edit.start_char, &edit.removed);
        self.cursor = edit.cursor_before;
        self.redo_stack.push(edit);
        self.modified = !self.undo_stack.is_empty();
        self.goal_col = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo_stack.pop() else { return false };
        let removed_len = edit.removed.chars().count();
        self.rope.remove(edit.start_char..edit.start_char + removed_len);
        self.rope.insert(edit.start_char, &edit.inserted);
        self.cursor = edit.cursor_after;
        self.undo_stack.push(edit.clone());
        self.modified = true;
        self.goal_col = None;
        true
    }

    pub fn move_left(&mut self) {
        self.goal_col = None;
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line(self.cursor.line).chars().count();
        }
    }

    pub fn move_right(&mut self) {
        self.goal_col = None;
        let len = self.line(self.cursor.line).chars().count();
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.line > 0 {
            let goal = self.goal_col.get_or_insert(self.cursor.col);
            let goal = *goal;
            self.cursor.line -= 1;
            self.cursor = self.clamp_pos(Pos::new(self.cursor.line, goal));
            self.goal_col = Some(goal);
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.line + 1 < self.rope.len_lines() {
            let goal = self.goal_col.get_or_insert(self.cursor.col);
            let goal = *goal;
            self.cursor.line += 1;
            self.cursor = self.clamp_pos(Pos::new(self.cursor.line, goal));
            self.goal_col = Some(goal);
        }
    }

    pub fn move_home(&mut self) {
        self.goal_col = None;
        self.cursor.col = 0;
    }

    pub fn move_end(&mut self) {
        self.goal_col = None;
        self.cursor.col = self.line(self.cursor.line).chars().count();
    }

    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace() {
        let mut b = Buffer::from_text("hello\nworld", None);
        b.cursor = Pos::new(0, 5);
        b.insert_str(" there");
        assert_eq!(b.to_string(), "hello there\nworld");
        b.backspace();
        assert_eq!(b.to_string(), "hello ther\nworld");
        assert_eq!(b.cursor, Pos::new(0, 10));
    }

    #[test]
    fn insert_newline_moves_to_next_line_col0() {
        let mut b = Buffer::from_text("ab", None);
        b.cursor = Pos::new(0, 1);
        b.insert_char('\n');
        assert_eq!(b.to_string(), "a\nb");
        assert_eq!(b.cursor, Pos::new(1, 0));
    }

    #[test]
    fn backspace_at_line_start_joins_lines() {
        let mut b = Buffer::from_text("foo\nbar", None);
        b.cursor = Pos::new(1, 0);
        b.backspace();
        assert_eq!(b.to_string(), "foobar");
        assert_eq!(b.cursor, Pos::new(0, 3));
    }

    #[test]
    fn delete_forward_at_eol_joins_next_line() {
        let mut b = Buffer::from_text("foo\nbar", None);
        b.cursor = Pos::new(0, 3);
        b.delete_forward();
        assert_eq!(b.to_string(), "foobar");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut b = Buffer::from_text("abc", None);
        b.cursor = Pos::new(0, 3);
        b.insert_str("def");
        assert_eq!(b.to_string(), "abcdef");
        assert!(b.undo());
        assert_eq!(b.to_string(), "abc");
        assert_eq!(b.cursor, Pos::new(0, 3));
        assert!(b.redo());
        assert_eq!(b.to_string(), "abcdef");
    }

    #[test]
    fn delete_range_marked_region() {
        let mut b = Buffer::from_text("hello world", None);
        let removed = b.delete_range(Pos::new(0, 0), Pos::new(0, 6));
        assert_eq!(removed, "hello ");
        assert_eq!(b.to_string(), "world");
    }

    #[test]
    fn move_up_down_clamps_column() {
        let mut b = Buffer::from_text("longline\nhi", None);
        b.cursor = Pos::new(0, 8);
        b.move_down();
        assert_eq!(b.cursor, Pos::new(1, 2));
        b.move_up();
        assert_eq!(b.cursor, Pos::new(0, 8));
    }
}
