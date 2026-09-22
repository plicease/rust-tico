mod app;
mod buffer;
mod cli;
mod config;
mod fileio;
mod help;
mod history;
mod justify;
mod keymap;
mod lockfile;
mod options;
mod syntax;
mod theme;
mod ui;
mod watch;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    if cli.listsyntaxes {
        print_syntax_names();
        return Ok(());
    }
    if cli.listthemes {
        for name in theme::available_names(&theme::Loader::with_default_dirs()) {
            println!("{name}");
        }
        return Ok(());
    }

    let ignore_rcfiles = cli.ignorercfiles;
    let explicit_rcfile = cli.rcfile.as_deref();
    let loaded = config::load(explicit_rcfile, ignore_rcfiles, cli.modernbindings);
    let mut options = loaded.options;
    cli.apply(&mut options);

    let mut warnings = loaded.warnings;
    // A theme that can't be loaded falls back to the built-in default with
    // a warning, like any other bad config item -- never a refusal to start.
    let theme = match options.theme.as_deref() {
        Some(name) => theme::Loader::with_default_dirs()
            .load(name, &mut warnings)
            .unwrap_or_else(theme::Theme::builtin_default),
        None => theme::Theme::builtin_default(),
    };
    for w in &warnings {
        eprintln!("tico: {w}");
    }

    let file_args = cli::parse_file_args(&cli.files);
    let mut editor = app::Editor::new(options, loaded.keymap);
    editor.theme = theme;
    editor.buffers.clear();

    let syntax_override = editor.options.syntax_name.clone();
    if file_args.is_empty() {
        let mut buf = buffer::Buffer::empty();
        buf.language = syntax::detect_with_override(None, "", syntax_override.as_deref());
        editor.buffers.push(buf);
    } else {
        for (i, fa) in file_args.iter().enumerate() {
            // A bare `-` reads standard input into an unnamed buffer,
            // matching nano: `echo foo | nano -`.
            let (mut buf, message, level) = if fa.path == "-" {
                open_stdin(syntax_override.as_deref())
            } else {
                let path = std::path::PathBuf::from(&fa.path);
                open_one(&path, editor.options.locking, syntax_override.as_deref())
            };
            if let Some(line) = fa.line {
                let target = (line.max(1) as usize) - 1;
                buf.cursor.line = target.min(buf.line_count().saturating_sub(1));
                if let Some(col) = fa.column {
                    buf.cursor.col = (col.max(1) as usize) - 1;
                }
            }
            // The status message reflects whichever buffer ends up focused
            // (the first one), matching nano showing the "Read N lines"
            // blurb for the file that lands in the active edit window.
            if i == 0 {
                match level {
                    app::StatusLevel::Alert => editor.set_status_alert(message),
                    app::StatusLevel::Mild => editor.set_status_mild(message),
                    app::StatusLevel::Normal => editor.set_status(message),
                }
            }
            // Only the focused buffer gets an interactive "someone else is
            // editing this" prompt; for any others opened at the same time,
            // this mirrors nano's non-interactive ask_the_user=false path
            // (warn-lessly take over the lock) rather than chaining several
            // blocking prompts before the UI has even started.
            if let Some(prompt) = acquire_lock(&mut editor, &mut buf, i == 0) {
                editor.mode = app::Mode::Prompt(prompt);
            }
            editor.buffers.push(buf);
        }
    }
    editor.current = 0;

    ui::run(&mut editor)?;

    if editor.options.historylog {
        editor.history.save();
    }
    Ok(())
}

/// Print the names of tico's built-in syntax-highlighting languages, for
/// `-z`/`--listsyntaxes`. Unlike nano, these aren't read from nanorc `syntax`
/// definitions (tico intentionally ignores those; see the syntax module),
/// so this lists the fixed, compiled-in language registry instead, wrapped
/// to the real terminal width rather than nano's hardcoded 45 columns.
fn print_syntax_names() {
    println!("Available syntaxes:");
    // nano wraps this listing at a hardcoded 45 columns regardless of the
    // actual terminal size; wrap to the real width instead (falling back
    // to 80 when it can't be queried, e.g. output is piped to a file).
    let width = crossterm::terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(80)
        .max(10);
    let mut line = String::new();
    for name in syntax::names() {
        let extra = 1 + name.chars().count(); // leading space + the name itself
        if !line.is_empty() && line.chars().count() + extra > width {
            println!("{line}");
            line.clear();
        }
        line.push(' ');
        line.push_str(name);
    }
    if !line.is_empty() {
        println!("{line}");
    }
}

/// When `locking` is on, check for (and take) a vim-style lock on `buf`'s
/// file, matching nano's `do_lockfile()`. If another lock is already held
/// and `interactive` is true, returns a confirmation prompt instead of
/// acquiring the lock immediately (the caller must set it as the editor's
/// mode); otherwise a conflicting lock is taken over anyway, as nano itself
/// does in its non-interactive (`ask_the_user = FALSE`) path.
fn acquire_lock(
    editor: &mut app::Editor,
    buf: &mut buffer::Buffer,
    interactive: bool,
) -> Option<app::Prompt> {
    if !editor.options.locking || editor.options.view {
        return None;
    }
    let path = buf.path.clone()?;
    let lock_path = lockfile::lock_path(&path);
    let target = path.display().to_string();
    match lockfile::check_lock(&lock_path) {
        lockfile::LockCheck::None => {
            let _ = lockfile::write_lock(&lock_path, &target, false);
            buf.lock_filename = Some(lock_path);
            None
        }
        lockfile::LockCheck::Bad => {
            // nano warns and leaves the file open without taking a lock,
            // rather than overwriting a lock file it doesn't understand.
            editor.set_status_alert(format!("Bad lock file is ignored: {}", lock_path.display()));
            None
        }
        lockfile::LockCheck::Held(info) if interactive => Some(app::Prompt {
            kind: app::PromptKind::LockConflict { lock_path, target },
            menu: keymap::Menu::YesNo,
            label: format!(
                "File {} is being edited by {} (with {}, PID {}); open anyway?",
                path.display(),
                info.user,
                info.program,
                info.pid
            ),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        }),
        lockfile::LockCheck::Held(_) => {
            let _ = lockfile::write_lock(&lock_path, &target, false);
            buf.lock_filename = Some(lock_path);
            None
        }
    }
}

/// Read standard input into an unnamed buffer (`nano -`, `echo foo | tico
/// -`). Matches nano's `scoop_stdin()`: no path is attached (so writing it
/// out needs a filename, same as any other new buffer), and the buffer is
/// marked modified whenever it got any content — read-but-unsaved input
/// isn't "clean" the way a file freshly loaded from disk is.
fn open_stdin(syntax_override: Option<&str>) -> (buffer::Buffer, String, app::StatusLevel) {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        eprintln!("Reading data from keyboard; type ^D or ^D^D to finish.");
    }
    let mut content = String::new();
    let (mut buf, msg, level) = match std::io::stdin().read_to_string(&mut content) {
        Ok(_) => {
            let msg = fileio::describe_read(&content);
            let mut buf = buffer::Buffer::from_text(&content, None);
            if !content.is_empty() {
                buf.modified = true;
            }
            (buf, msg, app::StatusLevel::Normal)
        }
        Err(e) => (
            buffer::Buffer::empty(),
            format!("Failed to open stdin: {e}"),
            app::StatusLevel::Alert,
        ),
    };
    buf.language = syntax::detect_with_override(None, &buf.to_string(), syntax_override);
    (buf, msg, level)
}

/// Load one file argument as nano would: refuse to open directories (an
/// empty "New Buffer" is used instead), warn (but still load) files that
/// exist but aren't writable, and — when `locking` is on, matching nano's
/// `ISSET(LOCKING)`-gated check in `has_valid_path()` — warn when a new
/// file's containing directory isn't writable either. Returns (buffer,
/// status message, message severity).
fn open_one(
    path: &std::path::Path,
    locking: bool,
    syntax_override: Option<&str>,
) -> (buffer::Buffer, String, app::StatusLevel) {
    let (mut buf, msg, level) = open_one_inner(path, locking);
    // Detected once at load time (extension/filename -> shebang -> modeline,
    // all on by default, or forced by -Y/--syntax); the on/off toggle (M-Y)
    // only controls whether rendering actually uses it, so toggling back on
    // doesn't need to re-detect.
    buf.language =
        syntax::detect_with_override(buf.path.as_deref(), &buf.to_string(), syntax_override);
    (buf, msg, level)
}

fn open_one_inner(
    path: &std::path::Path,
    locking: bool,
) -> (buffer::Buffer, String, app::StatusLevel) {
    if path.is_dir() {
        return (
            buffer::Buffer::empty(),
            format!("'{}' is a directory", path.display()),
            app::StatusLevel::Alert,
        );
    }
    if !path.exists() {
        if locking {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            if parent.is_dir() && !fileio::path_writable(parent) {
                return (
                    buffer::Buffer::from_text("", Some(path.to_path_buf())),
                    format!("Directory '{}' is not writable", parent.display()),
                    app::StatusLevel::Mild,
                );
            }
        }
        return (
            buffer::Buffer::from_text("", Some(path.to_path_buf())),
            "New File".to_string(),
            app::StatusLevel::Normal,
        );
    }
    match fileio::load_file(path) {
        Ok(buf) => {
            if !fileio::path_writable(path) {
                (
                    buf,
                    format!("File '{}' is unwritable", path.display()),
                    app::StatusLevel::Alert,
                )
            } else {
                let msg = fileio::describe_read(&buf.to_string());
                (buf, msg, app::StatusLevel::Normal)
            }
        }
        Err(e) => (
            buffer::Buffer::from_text("", Some(path.to_path_buf())),
            format!("Error reading {}: {e}", path.display()),
            app::StatusLevel::Alert,
        ),
    }
}
