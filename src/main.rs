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
            let (mut buf, message) = if path.exists() {
                match fileio::load_file(&path) {
                    Ok(buf) => {
                        let msg = fileio::describe_read(&buf.to_string());
                        (buf, msg)
                    }
                    Err(e) => (buffer::Buffer::from_text("", Some(path.clone())), format!("Error reading {}: {e}", path.display())),
                }
            } else {
                (buffer::Buffer::from_text("", Some(path.clone())), "New File".to_string())
            };
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
                editor.set_status(message);
            }
        }
    }
    editor.current = 0;

    ui::run(&mut editor)?;
    Ok(())
}
