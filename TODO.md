# TODO

Features nano 8.7.1 has that tico does not yet. Remove an item once it is
implemented (and, where it applies, its "not yet implemented" status
message and test in `src/app.rs` / `src/ui.rs`).

Syntax highlighting is intentionally *not* nano-compatible; see
`CLAUDE.md`. Nothing about nanorc `color`/`syntax` directives belongs here.

## Bound keys that report "not yet implemented"

- [ ] Comment toggle (`M-3`)
- [ ] Word completion (`^]`)
- [ ] Paragraph navigation: begin/end of paragraph (`M-(` / `M-)`, `M-9` / `M-0`)
- [ ] Block navigation: previous/next block (`^Up` / `^Down`, `M-7` / `M-8`)
- [ ] Top/bottom row of screen (`M-Home` / `M-End`)
- [ ] Find matching bracket (`M-]`)
- [ ] Anchors: set, previous, next (`M-Ins` / `M-"`, `M-PgUp`, `M-PgDn` / `M-'`)
- [ ] Macros: record and replay (`M-:` / `M-;`)
- [ ] File browser (`^T` from the Read File / Write Out prompts), including
      its Go To Directory, First File and Last File actions
- [ ] No Conversion (`M-N`) at the Insert prompt
- [ ] Pipe Text (`M-\`) at the Execute prompt
- [ ] Cut Till End (`^V`) from the Execute menu
- [ ] Full Justify (`^J`) from the Execute menu — main-menu `M-J` already
      works; this path is stubbed separately in `src/ui.rs`
- [ ] Suspend (`^Z`) — currently "not supported in this build"

## Bound keys that silently do nothing

- [ ] Verbatim input (`M-V`)
- [ ] Center (`^L`) / Cycle (`M-%`)
- [ ] Whitespace display toggle (`M-P`)
- [ ] Scroll left / right (`M-<` / `M->`)
- [ ] Write Out prompt toggles: DOS format (`M-D`), Mac format (`M-M`),
      Append (`M-A`), Prepend (`M-P`), Backup (`M-B`) — bound but no
      handler in `apply_prompt_action`

## Line endings

- [ ] Detect CR / CRLF on read and preserve on write (currently Unix-only
      end to end in `src/fileio.rs`); needed by the DOS/Mac toggles above
      and by `set unix` / `set noconvert`

## Options parsed but never consulted

### Display

- [ ] `softwrap` — toggle flips the flag, nothing wraps
- [ ] `constantshow`
- [ ] `matchbrackets`
- [ ] `indicator` (scrollbar)
- [ ] `stateflags`
- [ ] `guidestripe`
- [ ] `showcursor`
- [ ] `boldtext`
- [ ] `bookstyle`
- [ ] `jumpyscrolling`
- [ ] `emptyline`
- [ ] `rawsequences`
- [ ] `whitespace`
- [ ] `atblanks`
- [ ] `afterends`
- [ ] `minibar` — shrinks the layout but the minibar itself is never drawn

### Colors

Only `selectedcolor`, `spotlightcolor` and `promptcolor` are honored.

- [ ] `titlecolor`
- [ ] `statuscolor`
- [ ] `numbercolor`
- [ ] `keycolor`
- [ ] `functioncolor`
- [ ] `minicolor`
- [ ] `errorcolor`
- [ ] `stripecolor`
- [ ] `scrollercolor`

### Files and safety

- [ ] `backup` / `backupdir` (no backup-file machinery exists yet)
- [ ] `allow_insecure_backup` — not even recognized by the nanorc parser
- [ ] `positionlog` (no position-log machinery exists yet)
- [ ] `saveonexit`
- [ ] `restricted`
- [ ] `operatingdir`
- [ ] `unix`
- [ ] `noconvert`
- [ ] `nonewlines`

### Editing

- [ ] `wordchars` / `wordbounds`
- [ ] `zap` (the setting: Backspace/Delete erase the marked region)
- [ ] `colonparsing`
- [ ] `rebinddelete`
- [ ] `preserve`

### Input

- [ ] `mouse` — toggle exists but no mouse events are captured
