//! Binary entry point: tracing + config + auth + terminal lifecycle + the TEA
//! loop. All domain logic lives in the `oxidone` library crate.
//!
//! Data flow (ADR-0001): the UI reads from the local SQLite cache for instant,
//! offline-capable startup. When BYO credentials are configured, background
//! workers refresh from Google over `RestClient`, mirror into the cache, and
//! feed the results back as `Message`s. With no credentials the app runs purely
//! against the cache.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::NaiveDate;
use crossterm::event::{self, Event, KeyEventKind};
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing_subscriber::EnvFilter;

use oxidone::api::{RestClient, TasksApi};
use oxidone::app::{update, Command, Message, Model};
use oxidone::auth::{self, FileTokenStore, TokenStore, YupTokenProvider};
use oxidone::cache::Cache;
use oxidone::config::{self, Config};
use oxidone::domain::{List, ListId, TaskId};
use oxidone::sync;
use oxidone::ui::{self, theme::Theme};

/// The live Google client, shared across background workers. `None` when no BYO
/// credentials are configured (offline, cache-only).
type Api = Option<Arc<dyn TasksApi>>;
/// The cache, shared between the reducer loop and background workers.
type SharedCache = Arc<Mutex<Cache>>;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::load();
    let theme = Theme::from_flavor(&config.theme);

    // Build the live client (and run first-run auth) BEFORE entering the TUI, so
    // the consent browser flow isn't hidden behind the alternate screen.
    let api = build_api(&config).await;

    let cache: SharedCache = Arc::new(Mutex::new(open_cache()));
    let (initial_lists, load_error) = {
        let cache = cache.lock().unwrap();
        match cache.lists() {
            Ok(lists) => (lists, None),
            Err(e) => {
                tracing::warn!(error = %e, "failed to read cached lists");
                (
                    Vec::new(),
                    Some(format!("failed to read cached lists: {e}")),
                )
            }
        }
    };

    // `ratatui::init` enters the alternate screen + raw mode and installs a
    // panic hook that restores the terminal. `restore` reverses it.
    let mut terminal = ratatui::init();
    let result = run(
        &mut terminal,
        &theme,
        config.ascii_fallback,
        api,
        cache,
        initial_lists,
        load_error,
    )
    .await;
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    theme: &Theme,
    ascii: bool,
    api: Api,
    cache: SharedCache,
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

    // Seed the panes from cache for an instant first frame — Lists and, via the
    // emitted commands, their Tasks read straight from cache even when online.
    // Then a background refresh (if online) updates both from Google.
    let mut model = Model::new();
    let seed = update(&mut model, Message::ListsLoaded(initial_lists));
    seed_tasks_from_cache(&mut model, seed, &cache);
    if let Some(reason) = load_error {
        update(&mut model, Message::LoadFailed(reason));
    }
    if let Some(api) = &api {
        spawn_refresh_lists(api.clone(), cache.clone(), tx.clone());
    }

    loop {
        terminal.draw(|frame| ui::view(&model, theme, ascii, frame))?;
        match rx.recv().await {
            Some(msg) => {
                // Stamp the clock at the impure edge so the reducer stays pure
                // yet can resolve relative due dates (ADR-0005).
                model.now = chrono::Local::now();
                dispatch(update(&mut model, msg), &api, &cache, &tx);
                if model.should_quit {
                    break;
                }
            }
            None => break,
        }
    }
    Ok(())
}

/// Execute the reducer's side-effect `Command`s. Online, each becomes a
/// background worker that refreshes from Google and mirrors into the cache;
/// offline, it reads straight from the cache (ADR-0001).
fn dispatch(commands: Vec<Command>, api: &Api, cache: &SharedCache, tx: &UnboundedSender<Message>) {
    for command in commands {
        match command {
            Command::LoadTasks(list) => match api {
                Some(api) => spawn_load_tasks(api.clone(), cache.clone(), tx.clone(), list),
                None => {
                    let message = match cache.lock().unwrap().tasks(&list) {
                        Ok(tasks) => Message::TasksLoaded(list, tasks),
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to read cached tasks");
                            Message::LoadFailed(format!("failed to read tasks: {e}"))
                        }
                    };
                    let _ = tx.send(message);
                }
            },
            Command::SetCompleted {
                list,
                task,
                completed,
            } => match api {
                Some(api) => spawn_write_completed(
                    api.clone(),
                    cache.clone(),
                    tx.clone(),
                    list,
                    task,
                    completed,
                ),
                None => {
                    // No offline editing in v1 (ADR-0001): roll the optimistic
                    // change back with an explanation.
                    let _ = tx.send(Message::TaskWriteFailed {
                        task,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::SetTitle { list, task, title } => match api {
                Some(api) => {
                    spawn_write_title(api.clone(), cache.clone(), tx.clone(), list, task, title)
                }
                None => {
                    let _ = tx.send(Message::TaskWriteFailed {
                        task,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::SetDue { list, task, due } => match api {
                Some(api) => {
                    spawn_write_due(api.clone(), cache.clone(), tx.clone(), list, task, due)
                }
                None => {
                    let _ = tx.send(Message::TaskWriteFailed {
                        task,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::DeleteTask { list, task } => match api {
                Some(api) => spawn_delete_task(api.clone(), cache.clone(), tx.clone(), list, task),
                None => {
                    let _ = tx.send(Message::TaskDeleteFailed {
                        task,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::AddTask { list, temp, title } => match api {
                Some(api) => {
                    spawn_add_task(api.clone(), cache.clone(), tx.clone(), list, temp, title)
                }
                None => {
                    let _ = tx.send(Message::TaskAddFailed {
                        temp,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::AddList { temp, title } => match api {
                Some(api) => spawn_add_list(api.clone(), cache.clone(), tx.clone(), temp, title),
                None => {
                    let _ = tx.send(Message::ListAddFailed {
                        temp,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::RenameList { list, title } => match api {
                Some(api) => spawn_rename_list(api.clone(), cache.clone(), tx.clone(), list, title),
                None => {
                    let _ = tx.send(Message::ListWriteFailed {
                        list,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::DeleteList { list } => match api {
                Some(api) => spawn_delete_list(api.clone(), cache.clone(), tx.clone(), list),
                None => {
                    let _ = tx.send(Message::ListDeleteFailed {
                        list,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
        }
    }
}

/// Shown when a write is attempted with no live connection (ADR-0001: no offline
/// editing in v1).
const OFFLINE: &str = "not connected to Google";

/// Write-through a completion toggle: patch on Google (retry-once), mirror into
/// the cache, and report the server Task back (or a rollback on failure).
fn spawn_write_completed(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    task: TaskId,
    completed: bool,
) {
    tokio::spawn(async move {
        let message = match sync::patch_completed(api.as_ref(), &list, &task, completed).await {
            Ok(updated) => {
                if let Err(e) = cache.lock().unwrap().upsert_task(&updated) {
                    tracing::warn!(error = %e, "failed to cache task write");
                }
                Message::TaskUpdated(updated)
            }
            Err(e) => Message::TaskWriteFailed {
                task,
                reason: format!("failed to update task: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Write-through a title edit: patch on Google, mirror into the cache, report
/// the server Task back (or a rollback on failure).
fn spawn_write_title(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    task: TaskId,
    title: String,
) {
    tokio::spawn(async move {
        let message = match sync::patch_title(api.as_ref(), &list, &task, &title).await {
            Ok(updated) => {
                if let Err(e) = cache.lock().unwrap().upsert_task(&updated) {
                    tracing::warn!(error = %e, "failed to cache task write");
                }
                Message::TaskUpdated(updated)
            }
            Err(e) => Message::TaskWriteFailed {
                task,
                reason: format!("failed to update task: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Insert a Task on Google, mirror into the cache, and reconcile the optimistic
/// placeholder (by `temp` id) with the server Task — or drop it on failure.
fn spawn_add_task(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    temp: TaskId,
    title: String,
) {
    tokio::spawn(async move {
        let new = oxidone::api::NewTask {
            title,
            ..Default::default()
        };
        let message = match api.insert_task(&list, new).await {
            Ok(task) => {
                if let Err(e) = cache.lock().unwrap().upsert_task(&task) {
                    tracing::warn!(error = %e, "failed to cache inserted task");
                }
                Message::TaskInserted { temp, task }
            }
            Err(e) => Message::TaskAddFailed {
                temp,
                reason: format!("failed to add task: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Write-through a due-date change: patch on Google, mirror into the cache,
/// report the server Task back (or a rollback on failure). `due = None` clears.
fn spawn_write_due(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    task: TaskId,
    due: Option<NaiveDate>,
) {
    tokio::spawn(async move {
        let message = match sync::patch_due(api.as_ref(), &list, &task, due).await {
            Ok(updated) => {
                if let Err(e) = cache.lock().unwrap().upsert_task(&updated) {
                    tracing::warn!(error = %e, "failed to cache task write");
                }
                Message::TaskUpdated(updated)
            }
            Err(e) => Message::TaskWriteFailed {
                task,
                reason: format!("failed to update task: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Delete a Task on Google, mirror the removal into the cache, and report back
/// (or roll the optimistic delete back on failure).
fn spawn_delete_task(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    task: TaskId,
) {
    tokio::spawn(async move {
        let message = match api.delete_task(&list, &task).await {
            Ok(()) => {
                if let Err(e) = cache.lock().unwrap().delete_task(&task) {
                    tracing::warn!(error = %e, "failed to remove task from cache");
                }
                Message::TaskDeleted(task)
            }
            Err(e) => Message::TaskDeleteFailed {
                task,
                reason: format!("failed to delete task: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Insert a List on Google, mirror into the cache, and reconcile the optimistic
/// placeholder (by `temp` id) with the server List — or drop it on failure.
fn spawn_add_list(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    temp: ListId,
    title: String,
) {
    tokio::spawn(async move {
        let message = match api.insert_list(&title).await {
            Ok(list) => {
                if let Err(e) = cache.lock().unwrap().upsert_list(&list) {
                    tracing::warn!(error = %e, "failed to cache inserted list");
                }
                Message::ListInserted { temp, list }
            }
            Err(e) => Message::ListAddFailed {
                temp,
                reason: format!("failed to add list: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Write-through a List rename: patch on Google, mirror into the cache, report
/// the server List back (or a rollback on failure).
fn spawn_rename_list(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    title: String,
) {
    tokio::spawn(async move {
        let message = match sync::patch_list_title(api.as_ref(), &list, &title).await {
            Ok(updated) => {
                if let Err(e) = cache.lock().unwrap().upsert_list(&updated) {
                    tracing::warn!(error = %e, "failed to cache list rename");
                }
                Message::ListUpdated(updated)
            }
            Err(e) => Message::ListWriteFailed {
                list,
                reason: format!("failed to rename list: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Delete a List on Google, mirror the removal into the cache, and report back
/// (or roll the optimistic delete back on failure — e.g. Google refuses to
/// delete the undeletable default List).
fn spawn_delete_list(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
) {
    tokio::spawn(async move {
        let message = match api.delete_list(&list).await {
            Ok(()) => {
                if let Err(e) = cache.lock().unwrap().delete_list(&list) {
                    tracing::warn!(error = %e, "failed to remove list from cache");
                }
                Message::ListDeleted(list)
            }
            Err(e) => Message::ListDeleteFailed {
                list,
                reason: format!("failed to delete list: {e}"),
            },
        };
        let _ = tx.send(message);
    });
}

/// Seed the task pane from cache for the first frame. Each `LoadTasks` the seed
/// emits is served from the cache (instant, offline-capable) rather than the
/// network — the live refresh, dispatched later, does the network round-trip.
fn seed_tasks_from_cache(model: &mut Model, commands: Vec<Command>, cache: &SharedCache) {
    for command in commands {
        // The seed only emits LoadTasks (from ListsLoaded); ignore anything else.
        if let Command::LoadTasks(list) = command {
            match cache.lock().unwrap().tasks(&list) {
                Ok(tasks) => {
                    update(model, Message::TasksLoaded(list, tasks));
                }
                Err(e) => tracing::warn!(error = %e, "failed to read cached tasks"),
            }
        }
    }
}

/// Refresh all Lists from Google, mirror into the cache, and report back the
/// cached view (ADR-0001: the UI renders the cache projection, not raw API data).
fn spawn_refresh_lists(api: Arc<dyn TasksApi>, cache: SharedCache, tx: UnboundedSender<Message>) {
    tokio::spawn(async move {
        let message = match api.list_lists().await {
            // Lock only for the sync mirror+read, never across the await above.
            Ok(lists) => match sync::mirror_lists(&cache.lock().unwrap(), &lists) {
                Ok(cached) => Message::ListsLoaded(cached),
                Err(e) => Message::LoadFailed(format!("failed to cache lists: {e}")),
            },
            Err(e) => Message::LoadFailed(format!("failed to load lists: {e}")),
        };
        let _ = tx.send(message);
    });
}

/// Refresh one List's Tasks from Google, mirror, and report back the cached view.
fn spawn_load_tasks(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
) {
    tokio::spawn(async move {
        let message = match sync::fetch_active_tasks(api.as_ref(), &list).await {
            Ok(tasks) => match sync::mirror_tasks(&cache.lock().unwrap(), &list, &tasks) {
                Ok(cached) => Message::TasksLoaded(list, cached),
                Err(e) => Message::LoadFailed(format!("failed to cache tasks: {e}")),
            },
            Err(e) => Message::LoadFailed(format!("failed to load tasks: {e}")),
        };
        let _ = tx.send(message);
    });
}

/// Build the live Google client from BYO credentials, running the first-run
/// browser authorization if no token is cached yet. Returns `None` (offline) if
/// no credentials are configured or auth setup fails.
async fn build_api(config: &Config) -> Api {
    let secret = config.client_secret_path.as_ref()?;
    let store = FileTokenStore::in_config_dir()?;
    let has_token = match store.load() {
        Ok(token) => token.is_some(),
        Err(e) => {
            tracing::warn!(error = %e, "reading cached token failed; will re-authenticate");
            false
        }
    };
    let store: Arc<dyn TokenStore> = Arc::new(store);

    if !has_token {
        eprintln!("oxidone: authorizing with Google — a browser window will open…");
        if let Err(e) = auth::login(secret, store.clone()).await {
            tracing::error!(error = %e, "google authorization failed");
            eprintln!("oxidone: authorization failed ({e}); starting offline.");
            return None;
        }
    }

    match YupTokenProvider::new(secret, store).await {
        Ok(provider) => Some(Arc::new(RestClient::new(Arc::new(provider)))),
        Err(e) => {
            tracing::warn!(error = %e, "auth setup failed; starting offline");
            None
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
