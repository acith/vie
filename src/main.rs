mod app;
mod fs;
mod fs_ops;
mod image_loader;
mod input;
mod media;
mod ui;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::{App, PendingAction};
use crate::image_loader::ImageLoader;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliProtocol {
    Auto,
    Kitty,
    Sixel,
    Iterm2,
    Halfblocks,
}

#[derive(Parser, Debug)]
#[command(
    name = "vie",
    version = "0.12",
    about = "Fast Yazi-style terminal image viewer (supports Ghostty/Kitty, Foot/Sixel, iTerm2, Halfblocks)"
)]
struct Cli {
    /// Starting directory or image file
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Force a specific terminal graphics protocol
    #[arg(short, long, value_enum, default_value_t = CliProtocol::Auto)]
    protocol: CliProtocol,
}

fn clear_kitty_graphics() {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x1b_Ga=d,d=A\x1b\\");
    let _ = stdout.flush();
}

fn restore_terminal(protocol_type: Option<ProtocolType>) {
    if protocol_type == Some(ProtocolType::Kitty) {
        clear_kitty_graphics();
    }
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

fn edit_file_with_vi(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    protocol_type: ProtocolType,
    path: &Path,
) -> Result<()> {
    restore_terminal(Some(protocol_type));

    let status = Command::new("vi").arg(path).status();

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    enable_raw_mode()?;
    terminal.clear()?;

    // Drain any buffered key events left over after returning from vi
    while event::poll(Duration::from_millis(0))? {
        let _ = event::read()?;
    }

    if let Err(err) = status {
        return Err(anyhow::anyhow!("Failed to launch vi: {err}"));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(None);
        default_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // Query terminal graphics protocol and font size (after EnterAlternateScreen, before reading events)
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    match cli.protocol {
        CliProtocol::Auto => {}
        CliProtocol::Kitty => picker.set_protocol_type(ProtocolType::Kitty),
        CliProtocol::Sixel => picker.set_protocol_type(ProtocolType::Sixel),
        CliProtocol::Iterm2 => picker.set_protocol_type(ProtocolType::Iterm2),
        CliProtocol::Halfblocks => picker.set_protocol_type(ProtocolType::Halfblocks),
    }

    let protocol_type = picker.protocol_type();
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let loader = ImageLoader::new(picker.clone());
    let mut app = App::new(cli.path, loader, protocol_type);

    let res = run_loop(&mut terminal, &mut app, &picker);

    restore_terminal(Some(protocol_type));
    res
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    picker: &Picker,
) -> Result<()> {
    let mut needs_redraw = true;

    loop {
        if let Some(action) = app.pending_action.take() {
            if app.protocol_type == ProtocolType::Kitty {
                clear_kitty_graphics();
            }
            match action {
                PendingAction::EditText(path) => {
                    if let Err(err) = edit_file_with_vi(terminal, app.protocol_type, &path) {
                        app.set_error(err.to_string());
                    } else {
                        app.reload_directories(Some(&path));
                    }
                }
                PendingAction::PlayAudio(path) => {
                    if let Err(err) = media::play_audio(terminal, &path) {
                        app.set_error(err);
                    }
                }
                PendingAction::PlayVideo(path) => {
                    if let Err(err) = media::play_video(terminal, picker, &path) {
                        app.set_error(err);
                    }
                }
            }
            app.requested_key = None;
            app.needs_terminal_clear = true;
            needs_redraw = true;
        }

        if app.needs_terminal_clear {
            if app.protocol_type == ProtocolType::Kitty {
                clear_kitty_graphics();
            }
            terminal.clear()?;
            app.needs_terminal_clear = false;
            needs_redraw = true;
        }

        if needs_redraw {
            terminal.draw(|frame| ui::render(frame, app))?;
            needs_redraw = false;
        }

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => {
                    input::handle_key(app, key);
                    needs_redraw = true;
                }
                Event::Resize(_, _) => {
                    app.requested_key = None;
                    app.needs_terminal_clear = true;
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        if app.poll_loader() {
            needs_redraw = true;
        }
    }

    Ok(())
}
