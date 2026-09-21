//! Loading/saving files, detecting when the on-disk file has changed
//! underneath us, and three-way merging local edits with external changes.

use crate::buffer::{Buffer, DiskState};
use std::hash::{Hash, Hasher};
use std::path::Path;

/// True if the current process can actually write to `path` (a file or a
/// directory), matching nano's own `access(path, W_OK)` checks in
/// src/files.c. This is deliberately not the same thing as "some write bit
/// is set in the mode" (`std::fs::Permissions::readonly()`): a file like
/// `/etc/passwd`, owned by root with mode 644, has *a* write bit set (the
/// owner's) but a non-root user still can't write to it, and
/// `Permissions::readonly()` would wrongly say it's writable.
#[cfg(unix)]
pub fn path_writable(path: &Path) -> bool {
    rustix::fs::access(path, rustix::fs::Access::WRITE_OK).is_ok()
}

#[cfg(not(unix))]
pub fn path_writable(path: &Path) -> bool {
    // No portable equivalent of access(W_OK) wired up yet for non-Unix
    // targets; fall back to the mode-bit check, which at least catches the
    // common "no write bit at all" case.
    std::fs::metadata(path)
        .map(|m| !m.permissions().readonly())
        .unwrap_or(true)
}

fn hash_content(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn stat_disk_state(path: &Path) -> Option<DiskState> {
    let meta = std::fs::metadata(path).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    Some(DiskState {
        mtime: meta.modified().ok(),
        len: meta.len(),
        content_hash: hash_content(&content),
    })
}

/// A human-readable summary of a freshly loaded file's size, matching
/// nano's post-load status message (`Read %zu line(s)` in src/files.c).
/// nano counts actual lines of content, not ropey's `len_lines()` (which
/// counts a trailing empty line after a final newline).
pub fn describe_read(text: &str) -> String {
    let n = nano_style_line_count(text);
    if n == 1 {
        "Read 1 line".to_string()
    } else {
        format!("Read {n} lines")
    }
}

fn nano_style_line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let newlines = text.matches('\n').count();
    if text.ends_with('\n') {
        newlines
    } else {
        newlines + 1
    }
}

pub fn load_file(path: &Path) -> std::io::Result<Buffer> {
    let text = std::fs::read_to_string(path)?;
    let mut buf = Buffer::from_text(&text, Some(path.to_path_buf()));
    buf.disk_state = stat_disk_state(path);
    Ok(buf)
}

pub fn save_file(buffer: &mut Buffer, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, buffer.to_string())?;
    buffer.path = Some(path.to_path_buf());
    buffer.modified = false;
    buffer.disk_state = stat_disk_state(path);
    buffer.original_content = buffer.to_string();
    Ok(())
}

/// The result of checking whether the file changed on disk since we loaded
/// or last saved it.
pub enum ExternalChange {
    Unchanged,
    /// Disk changed, buffer has no local edits: safe to silently reload.
    ChangedNoLocalEdits,
    /// Disk changed *and* we have local edits: caller must prompt.
    ChangedWithLocalEdits,
}

pub fn check_external_change(buffer: &Buffer) -> ExternalChange {
    let Some(path) = &buffer.path else {
        return ExternalChange::Unchanged;
    };
    let Some(known) = &buffer.disk_state else {
        return ExternalChange::Unchanged;
    };
    let Some(current) = stat_disk_state(path) else {
        return ExternalChange::Unchanged;
    };
    if current.content_hash == known.content_hash {
        return ExternalChange::Unchanged;
    }
    if buffer.modified {
        ExternalChange::ChangedWithLocalEdits
    } else {
        ExternalChange::ChangedNoLocalEdits
    }
}

/// Reload a buffer from disk in place, discarding any (already-established
/// to be nonexistent-or-ignorable) local state. Preserves cursor position
/// where possible by clamping.
pub fn reload(buffer: &mut Buffer) -> std::io::Result<()> {
    let Some(path) = buffer.path.clone() else {
        return Ok(());
    };
    let text = std::fs::read_to_string(&path)?;
    let cursor = buffer.cursor;
    buffer.rope = ropey::Rope::from_str(&text);
    buffer.original_content = text;
    buffer.modified = false;
    buffer.undo_stack.clear();
    buffer.redo_stack.clear();
    buffer.disk_state = stat_disk_state(&path);
    let max_line = buffer.line_count().saturating_sub(1);
    buffer.cursor.line = cursor.line.min(max_line);
    let line_len = buffer.line(buffer.cursor.line).chars().count();
    buffer.cursor.col = cursor.col.min(line_len);
    Ok(())
}

/// The outcome of attempting a three-way merge of local edits against a
/// changed-on-disk file.
pub enum MergeResult {
    /// Merged cleanly; `text` is the merged content, `diff` a human-readable
    /// preview of what changed, to show before applying.
    Clean { text: String, diff: String },
    /// Could not merge automatically (overlapping conflicting edits).
    Conflict { diff: String },
}

/// Attempt a three-way merge: `base` is the content as originally loaded,
/// `ours` is the current (local, unsaved) buffer content, `theirs` is the
/// current on-disk content.
pub fn three_way_merge(base: &str, ours: &str, theirs: &str) -> MergeResult {
    use similar::{ChangeTag, TextDiff};

    // Diff base->ours and base->theirs, expressed as a common sequence of
    // base-line ranges so we can detect whether both sides touched the same
    // region of the base text.
    let ours_diff = TextDiff::from_lines(base, ours);
    let theirs_diff = TextDiff::from_lines(base, theirs);

    let ours_ops = ours_diff.ops();
    let theirs_ops = theirs_diff.ops();

    // Build, for each side, a map from base-line-index -> replacement lines
    // (only for ranges that actually differ from base).
    let base_lines: Vec<&str> = base.split_inclusive('\n').collect();
    let ours_lines: Vec<&str> = ours.split_inclusive('\n').collect();
    let theirs_lines: Vec<&str> = theirs.split_inclusive('\n').collect();

    #[derive(Clone)]
    struct Change {
        base_range: std::ops::Range<usize>,
        replacement: Vec<String>,
    }

    let extract_changes = |ops: &[similar::DiffOp], new_lines: &[&str]| -> Vec<Change> {
        let mut changes = Vec::new();
        for op in ops {
            match op {
                similar::DiffOp::Equal { .. } => {}
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                } => {
                    changes.push(Change {
                        base_range: *old_index..(*old_index + *old_len),
                        replacement: Vec::new(),
                    });
                }
                similar::DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                } => {
                    changes.push(Change {
                        base_range: *old_index..*old_index,
                        replacement: new_lines[*new_index..*new_index + *new_len]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    });
                }
                similar::DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    changes.push(Change {
                        base_range: *old_index..(*old_index + *old_len),
                        replacement: new_lines[*new_index..*new_index + *new_len]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    });
                }
            }
        }
        changes
    };

    let ours_changes = extract_changes(ours_ops, &ours_lines);
    let theirs_changes = extract_changes(theirs_ops, &theirs_lines);

    let ranges_overlap = |a: &std::ops::Range<usize>, b: &std::ops::Range<usize>| -> bool {
        if a.is_empty() && b.is_empty() {
            a.start == b.start
        } else {
            a.start < b.end && b.start < a.end
        }
    };

    let mut conflict = false;
    for oc in &ours_changes {
        for tc in &theirs_changes {
            if ranges_overlap(&oc.base_range, &tc.base_range) {
                conflict = true;
            }
        }
    }

    let diff_preview = |label: &str, text: &str| -> String {
        let d = TextDiff::from_lines(base, text);
        let mut out = format!("--- base\n+++ {label}\n");
        for change in d.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            out.push_str(sign);
            out.push_str(change.as_str().unwrap_or(""));
            if !change.as_str().unwrap_or("").ends_with('\n') {
                out.push('\n');
            }
        }
        out
    };

    if conflict {
        let mut diff = diff_preview("theirs (on disk)", theirs);
        diff.push('\n');
        diff.push_str(&diff_preview("ours (local edits)", ours));
        return MergeResult::Conflict { diff };
    }

    // No overlap: apply both sets of changes to base, in order.
    let mut all_changes: Vec<(Change, &str)> = ours_changes
        .iter()
        .map(|c| (c.clone(), "ours"))
        .chain(theirs_changes.iter().map(|c| (c.clone(), "theirs")))
        .collect();
    all_changes.sort_by_key(|(c, _)| (c.base_range.start, c.base_range.end));

    let mut merged = String::new();
    let mut pos = 0usize;
    for (change, _side) in &all_changes {
        while pos < change.base_range.start {
            merged.push_str(base_lines[pos]);
            pos += 1;
        }
        for line in &change.replacement {
            merged.push_str(line);
        }
        pos = change.base_range.end;
    }
    while pos < base_lines.len() {
        merged.push_str(base_lines[pos]);
        pos += 1;
    }

    let diff = diff_preview("merged", &merged);
    MergeResult::Clean { text: merged, diff }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_line_count_matches_nano() {
        assert_eq!(describe_read("a\nb\n"), "Read 2 lines");
        assert_eq!(describe_read("a\nb"), "Read 2 lines");
        assert_eq!(describe_read("a\nb\nc\n"), "Read 3 lines");
        assert_eq!(describe_read("onlyline"), "Read 1 line");
        assert_eq!(describe_read(""), "Read 0 lines");
    }

    #[test]
    fn merges_non_overlapping_edits() {
        let base = "one\ntwo\nthree\nfour\n";
        let ours = "one\nTWO\nthree\nfour\n"; // we changed line 2
        let theirs = "one\ntwo\nthree\nFOUR\n"; // they changed line 4
        match three_way_merge(base, ours, theirs) {
            MergeResult::Clean { text, .. } => {
                assert_eq!(text, "one\nTWO\nthree\nFOUR\n");
            }
            MergeResult::Conflict { diff } => panic!("expected clean merge, got conflict:\n{diff}"),
        }
    }

    #[test]
    fn detects_overlapping_conflict() {
        let base = "one\ntwo\nthree\n";
        let ours = "one\nTWO-ours\nthree\n";
        let theirs = "one\nTWO-theirs\nthree\n";
        match three_way_merge(base, ours, theirs) {
            MergeResult::Clean { text, .. } => panic!("expected conflict, got clean merge: {text}"),
            MergeResult::Conflict { .. } => {}
        }
    }

    #[test]
    fn unchanged_when_no_diffs() {
        let base = "same\n";
        match three_way_merge(base, base, base) {
            MergeResult::Clean { text, .. } => assert_eq!(text, base),
            MergeResult::Conflict { diff } => panic!("unexpected conflict:\n{diff}"),
        }
    }
}
