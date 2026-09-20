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
        for fa in &file_args {
            let path = std::path::PathBuf::from(&fa.path);
            let mut buf = if path.exists() {
                fileio::load_file(&path).unwrap_or_else(|_| buffer::Buffer::from_text("", Some(path.clone())))
            } else {
                buffer::Buffer::from_text("", Some(path.clone()))
            };
            if let Some(line) = fa.line {
                let target = (line.max(1) as usize) - 1;
                buf.cursor.line = target.min(buf.line_count().saturating_sub(1));
                if let Some(col) = fa.column {
                    buf.cursor.col = (col.max(1) as usize) - 1;
                }
            }
            editor.buffers.push(buf);
        }
    }
    editor.current = 0;

    ui::run(&mut editor)?;
    Ok(())
}
