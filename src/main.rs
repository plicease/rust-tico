mod app;
mod buffer;
mod cli;
mod config;
mod fileio;
mod keymap;
mod options;
mod ui;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    if cli.listsyntaxes {
        println!("(syntax listing not yet implemented)");
        return Ok(());
    }

    let ignore_rcfiles = cli.ignorercfiles;
    let explicit_rcfile = cli.rcfile.as_deref();
    let loaded = config::load(explicit_rcfile, ignore_rcfiles);
    let mut options = loaded.options;
    cli.apply(&mut options);

    for w in &loaded.warnings {
        eprintln!("tico: {w}");
    }

    let file_args = cli::parse_file_args(&cli.files);
    let mut editor = app::Editor::new(options, loaded.keymap);
    editor.buffers.clear();

    if file_args.is_empty() {
        editor.buffers.push(buffer::Buffer::empty());
    } else {
        for (i, fa) in file_args.iter().enumerate() {
            let path = std::path::PathBuf::from(&fa.path);
            let (mut buf, message, level) = open_one(&path, editor.options.locking);
            if let Some(line) = fa.line {
                let target = (line.max(1) as usize) - 1;
                buf.cursor.line = target.min(buf.line_count().saturating_sub(1));
                if let Some(col) = fa.column {
                    buf.cursor.col = (col.max(1) as usize) - 1;
                }
            }
            editor.buffers.push(buf);
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
        }
    }
    editor.current = 0;

    ui::run(&mut editor)?;
    Ok(())
}

/// Load one file argument as nano would: refuse to open directories (an
/// empty "New Buffer" is used instead), warn (but still load) files that
/// exist but aren't writable, and — when `locking` is on, matching nano's
/// `ISSET(LOCKING)`-gated check in `has_valid_path()` — warn when a new
/// file's containing directory isn't writable either. Returns (buffer,
/// status message, message severity).
fn open_one(path: &std::path::Path, locking: bool) -> (buffer::Buffer, String, app::StatusLevel) {
    if path.is_dir() {
        return (
            buffer::Buffer::empty(),
            format!("'{}' is a directory", path.display()),
            app::StatusLevel::Alert,
        );
    }
    if !path.exists() {
        if locking {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
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
                (buf, format!("File '{}' is unwritable", path.display()), app::StatusLevel::Alert)
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
