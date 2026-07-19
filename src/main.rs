//! Binary entry point: tracing + config + terminal lifecycle + the TEA loop.
//! All domain logic lives in the `oxidone` library crate.

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use oxidone::app::{update, Message, Model};
use oxidone::config::{self, Config};
use oxidone::ui::{self, theme::Theme};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::load();
    let theme = Theme::from_flavor(&config.theme);

    // `ratatui::init` enters the alternate screen + raw mode and installs a
    // panic hook that restores the terminal — so a panic can't leave the tty
    // wedged. `restore` reverses it on the normal path.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &theme).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal, theme: &Theme) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Blocking terminal-event reader on its own thread; it feeds key presses
    // into the reducer loop over the channel. The thread exits when the receiver
    // is dropped (quit) or on a read error.
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if tx.send(Message::Key(key)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    let mut model = Model::new();
    loop {
        terminal.draw(|frame| ui::view(&model, theme, frame))?;
        match rx.recv().await {
            Some(msg) => {
                // Later slices dispatch the returned Commands to workers here;
                // the shell emits none (`Command` is uninhabited).
                let _commands = update(&mut model, msg);
                if model.should_quit {
                    break;
                }
            }
            None => break,
        }
    }
    Ok(())
}

/// Logs go to a daily-rotating file in the platform log dir — never stdout,
/// which would corrupt the TUI. Best-effort: if the dir can't be prepared,
/// the app runs without file logging rather than failing to start.
fn init_tracing() {
    let Some(dir) = config::log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let file_appender = tracing_appender::rolling::daily(&dir, "oxidone.log");
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init();
}
