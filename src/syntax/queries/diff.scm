; Copied from the Helix editor's runtime/queries/diff/highlights.scm
; (https://github.com/helix-editor/helix, at commit 737ab17), which is
; licensed under the Mozilla Public License 2.0. Unlike the rest of tico
; (MIT), this file remains under MPL-2.0; see LICENSE-MPL-2.0 in the
; repository root.
;
; It replaces tree-sitter-diff's own highlights.scm, which uses arbitrary
; @string/@keyword buckets; this one names the real diff.* scopes every
; Helix theme defines, so additions and deletions come out green and red
; whatever theme is in use.

[(addition) (new_file)] @diff.plus
[(deletion) (old_file)] @diff.minus

(commit) @constant
(location) @attribute
(command) @markup.bold

(comment) @comment
; Rename/copy similarity percentage (`similarity index 95%`).
(score) @constant.numeric
