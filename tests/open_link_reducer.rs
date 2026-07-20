//! Reducer tests for opening a Task's links (`u`): the four press outcomes, the
//! picker's keys, and the failure report. `Command::OpenUrl` is the seam — no
//! test here launches a browser.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Focus, Message, Model, Overlay};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

/// A single Task carrying `notes`, with the task pane focused.
async fn model_with_notes(notes: &str) -> Model {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "alpha".to_string(),
            notes: (!notes.is_empty()).then(|| notes.to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    m
}

fn opened(cmds: &[Command]) -> &str {
    match cmds {
        [Command::OpenUrl(url)] => url.as_str(),
        other => panic!("expected a single OpenUrl, got {other:?}"),
    }
}

#[tokio::test]
async fn u_on_the_sidebar_does_nothing_at_all() {
    let mut m = model_with_notes("see https://a.dev/1").await;
    m.focus = Focus::Sidebar;

    let cmds = update(&mut m, ch('u'));

    assert!(cmds.is_empty());
    assert_eq!(
        m.status_line, None,
        "the sidebar has no link verb to report"
    );
    assert!(m.overlay.is_none());
}

#[tokio::test]
async fn u_with_no_links_says_so() {
    let mut m = model_with_notes("just a plain note").await;

    let cmds = update(&mut m, ch('u'));

    assert!(cmds.is_empty());
    assert_eq!(m.status_line.as_deref(), Some("no links in these notes"));
}

#[tokio::test]
async fn u_with_only_unopenable_links_distinguishes_itself_from_finding_none() {
    let mut m = model_with_notes("backup file:///srv/dump and share smb://nas/vol").await;

    let cmds = update(&mut m, ch('u'));

    assert!(cmds.is_empty(), "nothing openable, so nothing to open");
    assert_eq!(
        m.status_line.as_deref(),
        Some("2 links found, none openable (http/https only)"),
    );
    // The whole point of scanning every scheme: this must not read "no links".
    let mut none = model_with_notes("just a plain note").await;
    update(&mut none, ch('u'));
    assert_ne!(m.status_line, none.status_line);
}

#[tokio::test]
async fn a_lone_unopenable_link_is_reported_in_the_singular() {
    let mut m = model_with_notes("backup file:///srv/dump").await;

    update(&mut m, ch('u'));

    assert_eq!(
        m.status_line.as_deref(),
        Some("1 link found, none openable (http/https only)"),
    );
}

#[tokio::test]
async fn u_with_one_link_opens_it_and_says_which() {
    let mut m = model_with_notes("ticket https://a.dev/1, thanks").await;

    let cmds = update(&mut m, ch('u'));

    assert_eq!(opened(&cmds), "https://a.dev/1");
    // Detached and silent: without this the working case looks like a dead key.
    assert_eq!(m.status_line.as_deref(), Some("opening https://a.dev/1"));
    assert!(m.overlay.is_none(), "one link needs no picker");
}

#[tokio::test]
async fn u_with_several_links_raises_the_picker_instead_of_guessing() {
    let mut m = model_with_notes("https://a.dev/1 and https://b.dev/2").await;

    let cmds = update(&mut m, ch('u'));

    assert!(cmds.is_empty(), "nothing opens until one is chosen");
    match &m.overlay {
        Some(Overlay::OpenLink { urls, selected }) => {
            assert_eq!(urls.len(), 2);
            assert_eq!(*selected, 0);
        }
        other => panic!("expected OpenLink overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn the_picker_only_counts_openable_links() {
    let mut m = model_with_notes("file:///x https://a.dev/1 smb://n/v https://b.dev/2").await;

    update(&mut m, ch('u'));

    match &m.overlay {
        Some(Overlay::OpenLink { urls, .. }) => {
            let shown: Vec<&str> = urls.iter().map(|u| u.as_str()).collect();
            assert_eq!(shown, vec!["https://a.dev/1", "https://b.dev/2"]);
        }
        other => panic!("expected OpenLink overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn the_picker_moves_with_j_and_k_and_clamps_at_both_ends() {
    let mut m = model_with_notes("https://a.dev/1 https://b.dev/2").await;
    update(&mut m, ch('u'));

    let selected = |m: &Model| match &m.overlay {
        Some(Overlay::OpenLink { selected, .. }) => *selected,
        other => panic!("expected OpenLink overlay, got {other:?}"),
    };

    update(&mut m, ch('k')); // already at the top
    assert_eq!(selected(&m), 0, "no wrap upward");
    update(&mut m, ch('j'));
    assert_eq!(selected(&m), 1);
    update(&mut m, ch('j')); // already at the bottom
    assert_eq!(selected(&m), 1, "no wrap downward");
    update(&mut m, key(KeyCode::Up));
    assert_eq!(selected(&m), 0, "arrows move too");
}

#[tokio::test]
async fn enter_opens_the_selected_link_and_closes_the_picker() {
    let mut m = model_with_notes("https://a.dev/1 https://b.dev/2").await;
    update(&mut m, ch('u'));
    update(&mut m, ch('j'));

    let cmds = update(&mut m, key(KeyCode::Enter));

    assert_eq!(opened(&cmds), "https://b.dev/2");
    assert_eq!(m.status_line.as_deref(), Some("opening https://b.dev/2"));
    assert!(m.overlay.is_none());
}

#[tokio::test]
async fn esc_closes_the_picker_without_opening_anything() {
    let mut m = model_with_notes("https://a.dev/1 https://b.dev/2").await;
    update(&mut m, ch('u'));

    let cmds = update(&mut m, key(KeyCode::Esc));

    assert!(cmds.is_empty());
    assert!(m.overlay.is_none());
}

#[tokio::test]
async fn q_does_not_quit_while_the_picker_is_open() {
    let mut m = model_with_notes("https://a.dev/1 https://b.dev/2").await;
    update(&mut m, ch('u'));

    update(&mut m, ch('q'));

    assert!(!m.should_quit, "a modal must swallow the quit key");
    assert!(m.overlay.is_some());
}

#[tokio::test]
async fn y_does_not_dismiss_the_picker() {
    // The picker has no text buffer, so a two-way overlay router would have sent
    // this to the Confirm arm and closed it.
    let mut m = model_with_notes("https://a.dev/1 https://b.dev/2").await;
    update(&mut m, ch('u'));

    let cmds = update(&mut m, ch('y'));

    assert!(cmds.is_empty());
    assert!(m.overlay.is_some(), "y is not a picker key");
}

#[tokio::test]
async fn a_failed_open_is_reported_rather_than_swallowed() {
    let mut m = model_with_notes("https://a.dev/1").await;

    let cmds = update(
        &mut m,
        Message::LinkOpenFailed {
            reason: "no browser configured".to_string(),
        },
    );

    assert!(cmds.is_empty());
    assert_eq!(
        m.status_line.as_deref(),
        Some("could not open link: no browser configured"),
    );
}
