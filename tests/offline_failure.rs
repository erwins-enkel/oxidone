//! `offline_failure_for` is the runtime's single decision for the ADR-0001
//! write-through rule: with no live Google client, a write `Command` rolls its
//! optimistic change back as the matching `*Failed` variant with `OFFLINE` as
//! the reason, and a read `Command` maps to `None` (it serves from the cache).
//!
//! This is the binary-side offline path that `main.rs::dispatch` relies on, so
//! it lives in the library and is tested here without a terminal or network.

use chrono::NaiveDate;
use oxidone::app::{offline_failure_for, Command, Message, OFFLINE};
use oxidone::domain::{ListId, TaskId};
use oxidone::links::OpenableUrl;

fn list(id: &str) -> ListId {
    ListId(id.into())
}
fn task(id: &str) -> TaskId {
    TaskId(id.into())
}
fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

#[test]
fn every_write_rolls_back_with_the_offline_reason() {
    // Each write Command carries the id its matching *Failed variant echoes back
    // to the reducer; the rest of the Command's fields are irrelevant offline,
    // so `offline_failure_for` must drop them (not, say, require the title).
    let cases: Vec<(Command, &str)> = vec![
        (
            Command::SetCompleted {
                list: list("w"),
                task: task("t1"),
                completed: true,
            },
            "t1",
        ),
        (
            Command::SetTitle {
                list: list("w"),
                task: task("t2"),
                title: "ignored".into(),
            },
            "t2",
        ),
        (
            Command::SetDue {
                list: list("w"),
                task: task("t3"),
                due: Some(date(2026, 1, 1)),
            },
            "t3",
        ),
        (
            Command::SetNotes {
                list: list("w"),
                task: task("t4"),
                notes: Some("ignored".into()),
            },
            "t4",
        ),
        (
            Command::DeleteTask {
                list: list("w"),
                task: task("t5"),
            },
            "t5",
        ),
        (
            Command::AddTask {
                list: list("w"),
                temp: task("t6"),
                title: "ignored".into(),
                parent: None,
                due: None,
            },
            "t6",
        ),
        // A same-List Move echoes the List, not the Task.
        (
            Command::Move {
                list: list("m1"),
                task: task("t7"),
                parent: None,
                previous: None,
            },
            "m1",
        ),
        // A cross-List Move echoes the Task, not the Lists.
        (
            Command::MoveToList {
                source: list("s"),
                task: task("t8"),
                destination: list("d"),
            },
            "t8",
        ),
        (
            Command::AddList {
                temp: list("l1"),
                title: "ignored".into(),
            },
            "l1",
        ),
        (
            Command::RenameList {
                list: list("l2"),
                title: "ignored".into(),
            },
            "l2",
        ),
        (Command::DeleteList { list: list("l3") }, "l3"),
        (Command::ClearCompleted { list: list("l4") }, "l4"),
    ];

    for (command, expected_id) in cases {
        let Some(failed) = offline_failure_for(&command) else {
            panic!(
                "{:?} should fail offline, not fall through to the cache",
                command
            );
        };
        // Each write surfaces as its own `*Failed` variant, echoing the id the
        // reducer needs to roll the optimistic change back. `LoadFailed` (the
        // id-less Refresh surface) is the wrong shape for a write and is
        // rejected here — its own case is below.
        let reason = match &failed {
            Message::TaskWriteFailed { task, reason }
            | Message::TaskDeleteFailed { task, reason }
            | Message::MoveToListFailed { task, reason } => {
                assert_eq!(
                    task.0, expected_id,
                    "{:?} echoed the wrong Task id",
                    command
                );
                reason
            }
            Message::TaskAddFailed { temp, reason } => {
                assert_eq!(
                    temp.0, expected_id,
                    "{:?} echoed the wrong temp id",
                    command
                );
                reason
            }
            Message::ListAddFailed { temp, reason } => {
                assert_eq!(
                    temp.0, expected_id,
                    "{:?} echoed the wrong temp id",
                    command
                );
                reason
            }
            Message::MoveFailed { list, reason }
            | Message::ListWriteFailed { list, reason }
            | Message::ListDeleteFailed { list, reason }
            | Message::ClearCompletedFailed { list, reason } => {
                assert_eq!(
                    list.0, expected_id,
                    "{:?} echoed the wrong List id",
                    command
                );
                reason
            }
            other => panic!(
                "{:?} produced a non-write-failure message {:?}",
                command, other
            ),
        };
        assert_eq!(reason, OFFLINE, "{:?} reason must be OFFLINE", command);
    }
}

#[test]
fn refresh_lists_fails_closed_as_load_failed_offline() {
    // A Refresh has no optimistic change and no per-row id, so it surfaces as the
    // id-less `LoadFailed` — the same surface a failed `list_lists` uses.
    let Some(Message::LoadFailed(reason)) = offline_failure_for(&Command::RefreshLists) else {
        panic!("RefreshLists offline should be LoadFailed");
    };
    assert_eq!(reason, OFFLINE);
}

#[test]
fn reads_and_non_google_commands_stay_online_optional() {
    // Reads serve from the cache whether online or not, so they map to `None`
    // and fall through to the cache paths in `dispatch`. URL open and the
    // external editor need no Google client either.
    let url = OpenableUrl::parse("https://example.com/a").expect("https is openable");
    let reads: Vec<Command> = vec![
        Command::LoadTasks(list("w")),
        Command::LoadToday {
            lists: Vec::new(),
            today: date(2026, 1, 1),
        },
        Command::LoadSearch { lists: Vec::new() },
        Command::LoadWeek { lists: Vec::new() },
        Command::OpenUrl(url),
        Command::SpawnEditor {
            task: task("t"),
            notes: None,
        },
    ];
    for command in reads {
        assert!(
            offline_failure_for(&command).is_none(),
            "{:?} must not fail offline — it is cache-served or Google-independent",
            command,
        );
    }
}
