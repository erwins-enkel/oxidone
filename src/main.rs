//! Binary entry point: tracing + config + terminal lifecycle + the TEA loop.
//! All domain logic lives in the `oxidone` library crate.

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing_subscriber::EnvFilter;

use oxidone::app::{update, Command, Message, Model};
use oxidone::cache::Cache;
use oxidone::config::{self, Config};
use oxidone::domain::List;
use oxidone::ui::{self, theme::Theme};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::load();
    let theme = Theme::from_flavor(&config.theme);

    // Reads come from the local cache (ADR-0001). The live refresh over a real
    // `TasksApi` is wired once the REST client lands (#15); for now the sidebar
    // shows whatever the cache already holds.
    let cache = open_cache();
    let (initial_lists, load_error) = match cache.lists() {
        Ok(lists) => (lists, None),
        Err(e) => {
            tracing::warn!(error = %e, "failed to read cached lists");
            (
                Vec::new(),
                Some(format!("failed to read cached lists: {e}")),
            )
        }
    };

    // `ratatui::init` enters the alternate screen + raw mode and installs a
    // panic hook that restores the terminal. `restore` reverses it.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &theme, cache, initial_lists, load_error).await;
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    theme: &Theme,
    cache: Cache,
    initial_lists: Vec<List>,
    load_error: Option<String>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Blocking terminal-event reader on its own thread; feeds key presses into
    // the reducer loop. Exits when the receiver is dropped (quit) or on error.
    let reader_tx = tx.clone();
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if reader_tx.send(Message::Key(key)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    let mut model = Model::new();
    dispatch(
        update(&mut model, Message::ListsLoaded(initial_lists)),
        &cache,
        &tx,
    );
    if let Some(reason) = load_error {
        update(&mut model, Message::LoadFailed(reason));
    }

    loop {
        terminal.draw(|frame| ui::view(&model, theme, frame))?;
        match rx.recv().await {
            Some(msg) => {
                dispatch(update(&mut model, msg), &cache, &tx);
                if model.should_quit {
                    break;
                }
            }
            None => break,
        }
    }
    Ok(())
}

/// Execute the reducer's side-effect `Command`s. Reads come from the local
/// cache (ADR-0001); a live refresh over a real `TasksApi` is wired with #15.
fn dispatch(commands: Vec<Command>, cache: &Cache, tx: &UnboundedSender<Message>) {
    for command in commands {
        match command {
            Command::LoadTasks(list) => {
                let message = match cache.tasks(&list) {
                    Ok(tasks) => Message::TasksLoaded(list, tasks),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to read cached tasks");
                        Message::LoadFailed(format!("failed to read tasks: {e}"))
                    }
                };
                let _ = tx.send(message);
            }
        }
    }
}

/// Open the on-disk cache, falling back to an in-memory one if the data dir or
/// database can't be prepared — the app runs either way.
fn open_cache() -> Cache {
    if let Some(path) = config::db_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match Cache::open(&path) {
            Ok(cache) => return cache,
            Err(e) => tracing::warn!(error = %e, "failed to open cache db; using in-memory"),
        }
    }
    Cache::open_in_memory().expect("in-memory sqlite cache")
}

/// Logs go to a daily-rotating file in the platform log dir — never stdout,
/// which would corrupt the TUI. Best-effort.
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
