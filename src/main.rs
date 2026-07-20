//! Binary entry point: tracing + config + auth + terminal lifecycle + the TEA
//! loop. All domain logic lives in the `oxidone` library crate.
//!
//! Data flow (ADR-0001): the UI reads from the local SQLite cache for instant,
//! offline-capable startup. When BYO credentials are configured, background
//! workers refresh from Google over `RestClient`, mirror into the cache, and
//! feed the results back as `Message`s. With no credentials the app runs purely
//! against the cache.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use chrono::NaiveDate;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing_subscriber::EnvFilter;

use oxidone::api::{RestClient, TasksApi};
use oxidone::app::{update, Command, Message, Model, OFFLINE};
use oxidone::auth::{self, FileTokenStore, TokenStore, YupTokenProvider};
use oxidone::cache::Cache;
use oxidone::config::{self, Config};
use oxidone::domain::{List, ListId, TaskId};
use oxidone::links::OpenableUrl;
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

    // Editor for the notes feature (`$VISUAL`/`$EDITOR`). Resolved once: its
    // presence decides the reducer's notes path, and the tokens drive the spawn.
    let editor = resolve_editor();

    // While the external notes editor owns the terminal, the reader thread must
    // not also read from it (it would steal the editor's keystrokes). The gate
    // coordinates that hand-off.
    let gate = ReaderGate {
        paused: Arc::new(AtomicBool::new(false)),
        parked: Arc::new(AtomicBool::new(false)),
    };

    // Terminal-event reader on its own thread; feeds key presses into the reducer
    // loop. It polls (rather than blocking in `read`) so it can observe the gate
    // between reads. Exits when the receiver is dropped (quit) or on error.
    let reader_tx = tx.clone();
    let reader_paused = gate.paused.clone();
    let reader_parked = gate.parked.clone();
    std::thread::spawn(move || loop {
        if reader_paused.load(Ordering::Acquire) {
            reader_parked.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        reader_parked.store(false, Ordering::Release);
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if reader_tx.send(Message::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    });

    // Seed the panes from cache for an instant first frame — Lists and, via the
    // emitted commands, their Tasks read straight from cache even when online.
    // Then a background refresh (if online) updates both from Google.
    let mut model = Model::new();
    model.editor_available = editor.is_some();
    model.api_available = api.is_some();
    let seed = update(&mut model, Message::ListsLoaded(initial_lists));
    seed_tasks_from_cache(&mut model, seed, &cache);
    // Sidebar meters for every cached List, before anything touches the network.
    recount(&cache, &mut model);
    if let Some(reason) = load_error {
        update(&mut model, Message::LoadFailed(reason));
    }
    if let Some(api) = &api {
        spawn_refresh_lists(api.clone(), cache.clone(), tx.clone());
    }

    loop {
        // Stamp before drawing: the view reads `now` to render due dates
        // relative (`ui::render_task_pane`) and to bucket the due-load strip
        // (`ui::due_load_counts`), so the first frame — seeded from cache,
        // before any event has arrived — must not read the placeholder epoch.
        // Every repaint re-stamps, so a long-lived session that crosses midnight
        // corrects itself on its next redraw. Note it only redraws on an event:
        // an idle session shows a stale "today" until something wakes it, which
        // would need a periodic tick to fix and is beyond this change.
        model.now = chrono::Local::now();
        terminal.draw(|frame| ui::view(&model, theme, ascii, frame))?;
        match rx.recv().await {
            Some(msg) => {
                // Re-stamp at the impure edge so the reducer stays pure yet can
                // resolve relative due dates (ADR-0005). Distinct from the draw
                // stamp above: `recv` blocks for an unbounded time, so the
                // reducer must not resolve "tomorrow" against the clock as it
                // read when the frame was painted.
                model.now = chrono::Local::now();
                // Every worker mirrors into the cache *before* it sends, so one
                // recount here sees every write, in order — no per-write-site
                // list to keep in step. Keys are skipped because a keystroke
                // never writes the cache: it may change the Model optimistically
                // and emit a Command, but the write lands later in a worker,
                // whose reply is not a Key and so recounts then. Some worker
                // replies (a failure, say) wrote nothing either — recounting
                // anyway costs one indexed query and keeps the rule simple.
                let cache_may_have_changed = !matches!(msg, Message::Key(_));
                let commands = update(&mut model, msg);
                if cache_may_have_changed {
                    recount(&cache, &mut model);
                }
                // `SpawnEditor` owns the terminal, so it runs here synchronously
                // rather than in a background worker; its follow-up write joins the
                // rest of the commands for the normal worker dispatch.
                let mut deferred = Vec::with_capacity(commands.len());
                for command in commands {
                    match command {
                        Command::SpawnEditor { task, notes } => deferred.extend(run_notes_editor(
                            terminal, &editor, &gate, &mut model, task, notes,
                        )),
                        other => deferred.push(other),
                    }
                }
                dispatch(deferred, &api, &cache, &tx);
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
            Command::SetNotes { list, task, notes } => match api {
                Some(api) => {
                    spawn_write_notes(api.clone(), cache.clone(), tx.clone(), list, task, notes)
                }
                None => {
                    let _ = tx.send(Message::TaskWriteFailed {
                        task,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::OpenUrl(url) => spawn_open_url(tx.clone(), url),
            // `SpawnEditor` never reaches here: it owns the terminal and is
            // handled synchronously in the run loop, not by a background worker.
            Command::SpawnEditor { .. } => {
                tracing::error!("SpawnEditor reached the worker dispatch; ignoring");
            }
            Command::DeleteTask { list, task } => match api {
                Some(api) => spawn_delete_task(api.clone(), cache.clone(), tx.clone(), list, task),
                None => {
                    let _ = tx.send(Message::TaskDeleteFailed {
                        task,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::AddTask {
                list,
                temp,
                title,
                parent,
            } => match api {
                Some(api) => spawn_add_task(
                    api.clone(),
                    cache.clone(),
                    tx.clone(),
                    list,
                    temp,
                    title,
                    parent,
                ),
                None => {
                    let _ = tx.send(Message::TaskAddFailed {
                        temp,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            Command::Move {
                list,
                task,
                parent,
                previous,
            } => match api {
                Some(api) => spawn_move(
                    api.clone(),
                    cache.clone(),
                    tx.clone(),
                    list,
                    task,
                    parent,
                    previous,
                ),
                None => {
                    let _ = tx.send(Message::MoveFailed {
                        list,
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
            Command::ClearCompleted { list } => match api {
                Some(api) => spawn_clear_completed(api.clone(), cache.clone(), tx.clone(), list),
                None => {
                    let _ = tx.send(Message::ClearCompletedFailed {
                        list,
                        reason: OFFLINE.to_string(),
                    });
                }
            },
            // The reducer already gates this on `api_available`, so the `None`
            // arm is unreachable in practice; it fails closed rather than
            // silently dropping the Refresh (cf. `run_notes_editor`). Unlike
            // the write commands above, a Refresh has no optimistic change to
            // roll back, so it reports via the id-less `LoadFailed`.
            Command::RefreshLists => match api {
                Some(api) => spawn_refresh_lists(api.clone(), cache.clone(), tx.clone()),
                None => {
                    let _ = tx.send(Message::LoadFailed(OFFLINE.to_string()));
                }
            },
        }
    }
}

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
    parent: Option<TaskId>,
) {
    tokio::spawn(async move {
        let new = oxidone::api::NewTask {
            title,
            parent,
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

/// Reposition/reparent a Task on Google (indent, outdent, or reorder), then
/// re-mirror the active view so the cache reflects the renumbered positions.
/// Reports success (with the reconciled Tasks) or a rollback on failure.
fn spawn_move(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    task: TaskId,
    parent: Option<TaskId>,
    previous: Option<TaskId>,
) {
    tokio::spawn(async move {
        if let Err(e) = api
            .move_task(&list, &task, parent.as_ref(), previous.as_ref())
            .await
        {
            let _ = tx.send(Message::MoveFailed {
                list,
                reason: format!("failed to move task: {e}"),
            });
            return;
        }
        // A Move renumbers many positions; re-fetch the active view and mirror it
        // so the pane reconciles to Google's authoritative order.
        let message = match sync::fetch_active_tasks(api.as_ref(), &list).await {
            Ok(tasks) => match sync::mirror_tasks(&cache.lock().unwrap(), &list, &tasks) {
                Ok(cached) => Message::MoveSucceeded {
                    list,
                    tasks: cached,
                },
                Err(e) => Message::MoveFailed {
                    list,
                    reason: format!("failed to cache move: {e}"),
                },
            },
            Err(e) => Message::MoveFailed {
                list,
                reason: format!("failed to refresh after move: {e}"),
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

/// Hand a URL to the platform browser.
///
/// Detached on purpose: a blocking or inherited-stdio launch lets the opener
/// (`xdg-open` and friends) write over the alternate screen, and waiting on a
/// browser would stall the worker for as long as it stays open. The scheme was
/// already checked — [`OpenableUrl`] cannot exist otherwise — so there is
/// nothing left to decide here but success or failure.
///
/// Runs on a blocking thread: the spawn itself is synchronous work.
fn spawn_open_url(tx: UnboundedSender<Message>, url: OpenableUrl) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = open::that_detached(url.as_str()) {
            let _ = tx.send(Message::LinkOpenFailed {
                reason: e.to_string(),
            });
        }
    });
}

/// Write-through a notes change: patch on Google, mirror into the cache, report
/// the server Task back (or a rollback on failure). `notes = None` clears them.
fn spawn_write_notes(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
    task: TaskId,
    notes: Option<String>,
) {
    tokio::spawn(async move {
        let message = match sync::patch_notes(api.as_ref(), &list, &task, notes).await {
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

/// Sweep a List's Completed Tasks on Google (`clear_completed`), then re-mirror
/// the active view so the cache drops the now-hidden Tasks too. Reports success
/// (drops the optimistic-Clear snapshot) or a rollback on failure. The log keeps
/// the swept completions — the re-mirror only touches the pure `tasks` mirror.
fn spawn_clear_completed(
    api: Arc<dyn TasksApi>,
    cache: SharedCache,
    tx: UnboundedSender<Message>,
    list: ListId,
) {
    tokio::spawn(async move {
        if let Err(e) = api.clear_completed(&list).await {
            let _ = tx.send(Message::ClearCompletedFailed {
                list,
                reason: format!("failed to clear completed: {e}"),
            });
            return;
        }
        // Refresh the cache to match Google (the cleared Tasks are now hidden).
        // Best-effort: the Clear itself already succeeded, so a cache-refresh
        // failure is logged, not surfaced as a Clear failure (which would roll
        // the optimistic removal back and mislead).
        match sync::fetch_active_tasks(api.as_ref(), &list).await {
            Ok(tasks) => {
                if let Err(e) = sync::mirror_tasks(&cache.lock().unwrap(), &list, &tasks) {
                    tracing::warn!(error = %e, "failed to re-mirror after clear");
                }
            }
            Err(e) => tracing::warn!(error = %e, "failed to refetch after clear"),
        }
        let _ = tx.send(Message::ClearedCompleted(list));
    });
}

/// Re-derive the sidebar's per-List counts from the cache and fold them into the
/// Model. The cache is their only source, so this runs wherever it may have
/// changed — once, at the event edge, rather than at each write site.
///
/// Applied through `update`, never sent on the channel: a `CountsLoaded` arriving
/// as an event would itself be an event worth recounting, and the two would feed
/// each other forever. A read failure leaves the previous counts in place — stale
/// meters beat blank ones, and the error is logged rather than swallowed.
fn recount(cache: &SharedCache, model: &mut Model) {
    match cache.lock().unwrap().list_counts() {
        Ok(counts) => {
            let commands = update(model, Message::CountsLoaded(counts));
            debug_assert!(
                commands.is_empty(),
                "CountsLoaded must stay command-free; the recount drops its return"
            );
        }
        Err(e) => tracing::warn!(error = %e, "failed to recount list totals"),
    }
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

/// Handshake flags for pausing the terminal-event reader while the external
/// editor owns the terminal. `paused` asks the reader to stop; `parked` is its
/// acknowledgement that it is idle (not blocked inside a read), so the runtime
/// can hand the terminal over without racing it for the editor's keystrokes.
struct ReaderGate {
    paused: Arc<AtomicBool>,
    parked: Arc<AtomicBool>,
}

/// Resolve the user's editor from `$VISUAL` then `$EDITOR`, split into program +
/// leading args (e.g. `EDITOR="code -w"`). `None` when neither is set, which
/// selects the inline fallback overlay instead.
fn resolve_editor() -> Option<Vec<String>> {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var) {
            let tokens: Vec<String> = value.split_whitespace().map(String::from).collect();
            if !tokens.is_empty() {
                return Some(tokens);
            }
        }
    }
    None
}

/// Run the notes editor for `task` and feed the outcome back through the reducer:
/// a change becomes an optimistic write-through (`NotesEdited`), an unchanged
/// buffer is a no-op, and a failure lands on the status line.
fn run_notes_editor(
    terminal: &mut ratatui::DefaultTerminal,
    editor: &Option<Vec<String>>,
    gate: &ReaderGate,
    model: &mut Model,
    task: TaskId,
    notes: Option<String>,
) -> Vec<Command> {
    let Some(tokens) = editor else {
        // editor_available is stamped from the same source, so this is unreachable
        // in practice; fail closed rather than silently drop the edit.
        return update(
            model,
            Message::LoadFailed("no editor configured".to_string()),
        );
    };
    match edit_in_external_editor(terminal, gate, tokens, notes.as_deref()) {
        Ok(Some(text)) => {
            let notes = (!text.is_empty()).then_some(text);
            update(model, Message::NotesEdited { task, notes })
        }
        Ok(None) => Vec::new(), // unchanged: nothing to write
        Err(e) => update(
            model,
            Message::LoadFailed(format!("notes editor failed: {e}")),
        ),
    }
}

/// Suspend the TUI, open `current` notes in the external editor, and return the
/// edited text — or `None` when it is unchanged. The reader gate is released and
/// the temp file removed on every exit path, so a teardown failure can't leave
/// the reader parked or leak the file.
fn edit_in_external_editor(
    terminal: &mut ratatui::DefaultTerminal,
    gate: &ReaderGate,
    editor: &[String],
    current: Option<&str>,
) -> io::Result<Option<String>> {
    let original = current.unwrap_or("");
    let path = std::env::temp_dir().join(format!("oxidone-notes-{}.txt", std::process::id()));
    std::fs::write(&path, original)?;

    // Ask the reader thread to stand down and wait (bounded) for it to confirm it
    // is idle, so it can't consume the editor's keystrokes. Proceed anyway if it
    // doesn't ack in time (e.g. it was mid-read of an already-buffered key).
    gate.paused.store(true, Ordering::Release);
    for _ in 0..20 {
        if gate.parked.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let outcome = run_suspended_editor(terminal, editor, &path, original);

    // Release the reader and drop the temp file whatever happened above — an early
    // `?` inside the suspend/resume must not leave either dangling.
    gate.paused.store(false, Ordering::Release);
    let _ = std::fs::remove_file(&path);
    outcome
}

/// Leave the TUI, run the editor on `path`, re-enter the TUI, and read the result
/// back — `None` when unchanged. Re-entry (raw mode + alternate screen) runs
/// before the editor's exit status is propagated, so the terminal is restored on
/// both success and editor-failure paths.
fn run_suspended_editor(
    terminal: &mut ratatui::DefaultTerminal,
    editor: &[String],
    path: &std::path::Path,
    original: &str,
) -> io::Result<Option<String>> {
    // Leave the TUI so the editor gets a clean terminal.
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;

    let status = std::process::Command::new(&editor[0])
        .args(&editor[1..])
        .arg(path)
        .status();

    // Re-enter the TUI regardless of how the editor exited, then force a full
    // repaint (the alternate screen was cleared underneath us).
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    if !status?.success() {
        return Err(io::Error::other("editor exited with a non-zero status"));
    }

    // Editors habitually append a trailing newline; ignore it when comparing so
    // an untouched buffer reads as "no change".
    let edited = std::fs::read_to_string(path)?;
    let trimmed = edited.trim_end_matches('\n');
    if trimmed == original.trim_end_matches('\n') {
        return Ok(None);
    }
    Ok(Some(trimmed.to_string()))
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
