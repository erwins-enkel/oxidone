//! Contract tests for `RestClient` against a `wiremock` server (ADR-0004).
//! Each test asserts the request oxidone builds (verb, path, query params, JSON
//! body) and that Google's JSON deserializes into the domain types. No live
//! Google account, no real OAuth — the bearer token is injected statically.

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use oxidone::api::{ApiError, NewTask, RestClient, TaskPatch, TasksApi};
use oxidone::auth::StaticTokenProvider;
use oxidone::domain::{ListId, Status, TaskId};

const TOKEN: &str = "test-bearer-token";

/// Build a `RestClient` pointed at the mock server, with a static bearer token.
fn client(server: &MockServer) -> RestClient {
    RestClient::with_base(
        server.uri(),
        Arc::new(StaticTokenProvider(TOKEN.to_string())),
    )
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

// ---- Lists ----

#[tokio::test]
async fn list_lists_gets_users_me_lists_and_deserializes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .and(header("authorization", bearer().as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                { "id": "L1", "title": "Work", "etag": "e1", "updated": "2023-11-14T22:13:20.000Z" },
                { "id": "L2", "title": "Home", "etag": "e2", "updated": "2023-11-14T22:13:21.000Z" }
            ]
        })))
        .mount(&server)
        .await;

    let lists = client(&server).list_lists().await.unwrap();
    let titles: Vec<_> = lists.iter().map(|l| l.title.as_str()).collect();
    assert_eq!(titles, ["Work", "Home"]);
    assert_eq!(lists[0].id, ListId("L1".into()));
    assert_eq!(lists[0].etag, "e1");
    assert_eq!(
        lists[0].updated,
        Utc.with_ymd_and_hms(2023, 11, 14, 22, 13, 20).unwrap()
    );
}

#[tokio::test]
async fn default_list_gets_the_default_alias_and_deserializes_to_the_concrete_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me/lists/@default"))
        .and(header("authorization", bearer().as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "MDExMjIzMzQ0NTU", "title": "My Tasks", "etag": "e9",
            "updated": "2023-11-14T22:13:20.000Z"
        })))
        .mount(&server)
        .await;

    let list = client(&server).default_list().await.unwrap();
    // The alias resolves to the real id we then use everywhere (ADR-0003).
    assert_eq!(list.id, ListId("MDExMjIzMzQ0NTU".into()));
    assert_eq!(list.title, "My Tasks");
}

#[tokio::test]
async fn insert_list_posts_title() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users/@me/lists"))
        .and(body_partial_json(json!({ "title": "New" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "L9", "title": "New", "etag": "e9", "updated": "2023-11-14T22:13:20.000Z"
        })))
        .mount(&server)
        .await;

    let list = client(&server).insert_list("New").await.unwrap();
    assert_eq!(list.id, ListId("L9".into()));
    assert_eq!(list.title, "New");
}

#[tokio::test]
async fn patch_list_patches_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/users/@me/lists/L1"))
        .and(body_partial_json(json!({ "title": "Renamed" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "L1", "title": "Renamed", "etag": "e2", "updated": "2023-11-14T22:13:20.000Z"
        })))
        .mount(&server)
        .await;

    let list = client(&server)
        .patch_list(&ListId("L1".into()), "Renamed")
        .await
        .unwrap();
    assert_eq!(list.title, "Renamed");
}

#[tokio::test]
async fn delete_list_deletes_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/users/@me/lists/L1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    client(&server)
        .delete_list(&ListId("L1".into()))
        .await
        .unwrap();
}

// ---- Tasks ----

#[tokio::test]
async fn list_tasks_sends_show_flags_and_updated_min() {
    let server = MockServer::start().await;
    let updated_min = Utc.with_ymd_and_hms(2023, 11, 14, 22, 13, 20).unwrap();
    Mock::given(method("GET"))
        .and(path("/lists/L1/tasks"))
        .and(query_param("showCompleted", "true"))
        .and(query_param("showHidden", "false"))
        .and(query_param("maxResults", "100"))
        .and(query_param("updatedMin", "2023-11-14T22:13:20Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": "T1", "title": "buy milk", "etag": "e1",
                    "updated": "2023-11-14T22:13:20.000Z",
                    "status": "needsAction", "position": "00000000000000000000",
                    "due": "2023-12-25T00:00:00.000Z", "notes": "2%",
                    "links": [
                        {
                            "type": "email",
                            "description": "Re: milk run",
                            "link": "https://mail.google.com/mail/u/0/#inbox/abc"
                        }
                    ]
                },
                {
                    "id": "T2", "title": "done thing", "etag": "e2",
                    "updated": "2023-11-14T22:13:21.000Z",
                    "status": "completed", "position": "00000000000000000001",
                    "completed": "2023-11-14T22:13:21.000Z", "parent": "T1"
                }
            ]
        })))
        .mount(&server)
        .await;

    let tasks = client(&server)
        .list_tasks(&ListId("L1".into()), true, false, Some(updated_min))
        .await
        .unwrap();
    assert_eq!(tasks.len(), 2);

    let t1 = &tasks[0];
    assert_eq!(t1.id, TaskId("T1".into()));
    assert_eq!(t1.title, "buy milk");
    assert_eq!(t1.list, ListId("L1".into()));
    assert_eq!(t1.status, Status::NeedsAction);
    assert_eq!(t1.due, Some(NaiveDate::from_ymd_opt(2023, 12, 25).unwrap()));
    assert_eq!(t1.notes.as_deref(), Some("2%"));
    assert!(t1.parent.is_none());
    // The output-only `links[]` mirror: `type` maps to `kind`, kept as a String.
    assert_eq!(t1.links.len(), 1);
    let l = &t1.links[0];
    assert_eq!(l.url, "https://mail.google.com/mail/u/0/#inbox/abc");
    assert_eq!(l.description.as_deref(), Some("Re: milk run"));
    assert_eq!(l.kind.as_deref(), Some("email"));

    let t2 = &tasks[1];
    assert_eq!(t2.status, Status::Completed);
    assert_eq!(t2.parent, Some(TaskId("T1".into())));
    assert!(t2.completed_at.is_some());
    // No `links` key in the JSON: `#[serde(default)]` yields an empty vec, not a
    // parse failure for the whole page.
    assert!(t2.links.is_empty());
}

#[tokio::test]
async fn insert_task_posts_body_with_due_date_only() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks"))
        .and(body_partial_json(json!({
            "title": "wrap gifts",
            "notes": "before the 24th",
            "due": "2023-12-25T00:00:00.000Z"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T5", "title": "wrap gifts", "etag": "e5",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction",
            "position": "00000000000000000000"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .insert_task(
            &ListId("L1".into()),
            NewTask {
                title: "wrap gifts".into(),
                notes: Some("before the 24th".into()),
                due: Some(NaiveDate::from_ymd_opt(2023, 12, 25).unwrap()),
                parent: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(task.id, TaskId("T5".into()));
    assert_eq!(task.status, Status::NeedsAction);
}

#[tokio::test]
async fn insert_subtask_sends_parent_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks"))
        .and(query_param("parent", "T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T6", "title": "sub", "etag": "e6",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction",
            "position": "00000000000000000000", "parent": "T1"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .insert_task(
            &ListId("L1".into()),
            NewTask {
                title: "sub".into(),
                parent: Some(TaskId("T1".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(task.parent, Some(TaskId("T1".into())));
}

#[tokio::test]
async fn patch_task_completing_sets_status() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/lists/L1/tasks/T1"))
        .and(body_partial_json(json!({ "status": "completed" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1", "title": "x", "etag": "e2",
            "updated": "2023-11-14T22:13:20.000Z", "status": "completed",
            "position": "0", "completed": "2023-11-14T22:13:20.000Z"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .patch_task(
            &ListId("L1".into()),
            &TaskId("T1".into()),
            TaskPatch {
                completed: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(task.status, Status::Completed);
    assert!(task.completed_at.is_some());
}

#[tokio::test]
async fn patch_task_reopening_clears_status_and_completed() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/lists/L1/tasks/T1"))
        .and(body_partial_json(
            json!({ "status": "needsAction", "completed": null }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1", "title": "x", "etag": "e3",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction", "position": "0"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .patch_task(
            &ListId("L1".into()),
            &TaskId("T1".into()),
            TaskPatch {
                completed: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(task.status, Status::NeedsAction);
    assert!(task.completed_at.is_none());
}

#[tokio::test]
async fn patch_task_can_clear_notes_and_due_with_null() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/lists/L1/tasks/T1"))
        .and(body_partial_json(json!({ "notes": null, "due": null })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1", "title": "x", "etag": "e4",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction", "position": "0"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .patch_task(
            &ListId("L1".into()),
            &TaskId("T1".into()),
            TaskPatch {
                notes: Some(None),
                due: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(task.notes.is_none());
    assert!(task.due.is_none());
}

#[tokio::test]
async fn delete_task_deletes_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    client(&server)
        .delete_task(&ListId("L1".into()), &TaskId("T1".into()))
        .await
        .unwrap();
}

// Every bodyless POST below asserts `content-length: 0`. reqwest omits the
// header entirely for a request with no body, and Google's HTTP/1.1 frontend
// answers 411 without it — never reaching the Tasks API. Observed live: the same
// POST is 411 without the header and 401 (i.e. past framing) with it.

#[tokio::test]
async fn move_task_sends_parent_and_previous_query_params() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T3/move"))
        .and(query_param("parent", "T1"))
        .and(query_param("previous", "T2"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T3", "title": "moved", "etag": "e5",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction",
            "position": "00000000000000000002", "parent": "T1"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .move_task(
            &ListId("L1".into()),
            &TaskId("T3".into()),
            Some(&TaskId("T1".into())),
            Some(&TaskId("T2".into())),
        )
        .await
        .unwrap();
    assert_eq!(task.parent, Some(TaskId("T1".into())));
}

#[tokio::test]
async fn move_task_to_list_sends_destination_tasklist_and_clears_the_echoed_parent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T3/move"))
        .and(query_param("destinationTasklist", "L2"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T3", "title": "moved", "etag": "e5",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction",
            // Google echoing the *source* List's parent. Left as-is it would name
            // a Task absent from L2, which `Model::groups` draws as an orphan.
            "position": "00000000000000000000", "parent": "T1"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .move_task_to_list(
            &ListId("L1".into()),
            &TaskId("T3".into()),
            &ListId("L2".into()),
        )
        .await
        .unwrap();
    // The destination is stamped from the argument, not read off the wire…
    assert_eq!(task.list, ListId("L2".into()));
    // …and the stale parent is dropped: the move asked for top-level.
    assert_eq!(task.parent, None);
}

#[tokio::test]
async fn move_task_to_top_sends_no_query_params() {
    let server = MockServer::start().await;
    // Matcher deliberately omits query_param assertions; the handler still
    // responds, proving oxidone sends a bare move for a top-of-list reposition.
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T3/move"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T3", "title": "moved", "etag": "e5",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction", "position": "0"
        })))
        .mount(&server)
        .await;

    let task = client(&server)
        .move_task(&ListId("L1".into()), &TaskId("T3".into()), None, None)
        .await
        .unwrap();
    assert!(task.parent.is_none());
}

#[tokio::test]
async fn clear_completed_posts_to_clear_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/clear"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    client(&server)
        .clear_completed(&ListId("L1".into()))
        .await
        .unwrap();
}

// ---- Error mapping ----

#[tokio::test]
async fn unauthorized_maps_to_auth_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "code": 401, "message": "Invalid Credentials", "status": "UNAUTHENTICATED" }
        })))
        .mount(&server)
        .await;

    let err = client(&server).list_lists().await.unwrap_err();
    assert_eq!(err, oxidone::api::ApiError::AuthExpired);
}

#[tokio::test]
async fn not_found_maps_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/users/@me/lists/GONE"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": { "code": 404, "message": "Not Found", "status": "NOT_FOUND" }
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .patch_list(&ListId("GONE".into()), "x")
        .await
        .unwrap_err();
    assert_eq!(err, oxidone::api::ApiError::NotFound);
}

#[tokio::test]
async fn other_error_maps_to_rejected_with_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "code": 400, "message": "Invalid task", "status": "INVALID_ARGUMENT" }
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .insert_task(
            &ListId("L1".into()),
            NewTask {
                title: "x".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        oxidone::api::ApiError::Rejected {
            status: 400,
            message: "Invalid task".into()
        }
    );
}

// ---- Pagination (issue #87) ----
//
// Google caps `tasks.list` at 100 rows a page and hands back a `nextPageToken`;
// a client that ignores it shows a truncated List that looks complete. Pages are
// told apart with the `up_to_n_times(1)` + `with_priority` idiom used by the
// 401-retry tests below: the earlier stub retires after one hit, so the next
// request falls through to the stub that asserts the cursor.

/// One `tasks.list` row, so a multi-page body stays readable.
fn task_json(id: &str, position: &str) -> serde_json::Value {
    json!({
        "id": id, "title": id, "etag": "e1", "status": "needsAction",
        "updated": "2023-11-14T22:13:20.000Z", "position": position
    })
}

/// Every filter has to be re-sent with the cursor — Google does not remember a
/// request's parameters across a `pageToken`. Each one asserted here is a
/// distinct silent corruption if dropped: `updatedMin` would widen the
/// incremental sync window so later pages carry Tasks the caller filtered out,
/// and `maxResults` would fall back to Google's default of 20, quintupling the
/// round trips on exactly the path the Move Subtask check walks.
#[tokio::test]
async fn list_tasks_follows_next_page_token() {
    let server = MockServer::start().await;
    let updated_min = Utc.with_ymd_and_hms(2023, 11, 14, 22, 13, 20).unwrap();
    let filters = |mock: wiremock::MockBuilder| {
        mock.and(query_param("showCompleted", "true"))
            .and(query_param("showHidden", "true"))
            .and(query_param("maxResults", "100"))
            .and(query_param("updatedMin", "2023-11-14T22:13:20Z"))
    };

    filters(Mock::given(method("GET")).and(path("/lists/L1/tasks")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T1", "00000000000000000000")],
            "nextPageToken": "P2"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    filters(Mock::given(method("GET")).and(path("/lists/L1/tasks")))
        .and(query_param("pageToken", "P2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T2", "00000000000000000001")]
        })))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;

    let tasks = client(&server)
        .list_tasks(&ListId("L1".into()), true, true, Some(updated_min))
        .await
        .unwrap();
    // Both pages, concatenated in the order Google served them.
    let ids: Vec<_> = tasks.iter().map(|t| t.id.0.as_str()).collect();
    assert_eq!(ids, ["T1", "T2"]);
}

/// The regression test for cursor accumulation, which needs a *third* page to
/// be visible at all: `reqwest`'s `RequestBuilder::query` merges rather than
/// replaces, so a client that clones one builder and appends `pageToken` per
/// page sends `pageToken=P2&pageToken=P3` from here on — and a plain
/// `query_param("pageToken", "P3")` matcher is satisfied by that request. Only
/// counting the pairs catches it.
#[tokio::test]
async fn list_tasks_sends_the_cursor_exactly_once_on_the_third_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/lists/L1/tasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T1", "00000000000000000000")],
            "nextPageToken": "P2"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lists/L1/tasks"))
        .and(query_param("pageToken", "P2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T2", "00000000000000000001")],
            "nextPageToken": "P3"
        })))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lists/L1/tasks"))
        .and(|req: &Request| {
            let cursors: Vec<String> = req
                .url
                .query_pairs()
                .filter(|(key, _)| key == "pageToken")
                .map(|(_, value)| value.into_owned())
                .collect();
            cursors == ["P3"]
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T3", "00000000000000000002")]
        })))
        .with_priority(3)
        .expect(1)
        .mount(&server)
        .await;

    let tasks = client(&server)
        .list_tasks(&ListId("L1".into()), true, false, None)
        .await
        .unwrap();
    let ids: Vec<_> = tasks.iter().map(|t| t.id.0.as_str()).collect();
    assert_eq!(ids, ["T1", "T2", "T3"]);
}

/// A cursor Google repeats verbatim is going nowhere. Fail closed: the two
/// Tasks already gathered are discarded rather than returned as if they were the
/// whole List — a short `Vec` here is indistinguishable from a short List.
#[tokio::test]
async fn list_tasks_rejects_a_repeated_page_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/lists/L1/tasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T1", "00000000000000000000")],
            "nextPageToken": "P2"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // Same cursor back again.
    Mock::given(method("GET"))
        .and(path("/lists/L1/tasks"))
        .and(query_param("pageToken", "P2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [task_json("T2", "00000000000000000001")],
            "nextPageToken": "P2"
        })))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;

    let err = client(&server)
        .list_tasks(&ListId("L1".into()), true, false, None)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Pagination(_)), "got {err:?}");
}

/// `tasklists.list` paginates too — Google's page default there is 1000 (which
/// is also its maximum, so oxidone sends no `maxResults`), but an account may
/// hold 2000 Lists.
#[tokio::test]
async fn list_lists_follows_next_page_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                { "id": "L1", "title": "Work", "etag": "e1",
                  "updated": "2023-11-14T22:13:20.000Z" }
            ],
            "nextPageToken": "P2"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .and(query_param("pageToken", "P2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                { "id": "L2", "title": "Home", "etag": "e2",
                  "updated": "2023-11-14T22:13:21.000Z" }
            ]
        })))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;

    let lists = client(&server).list_lists().await.unwrap();
    let titles: Vec<_> = lists.iter().map(|l| l.title.as_str()).collect();
    assert_eq!(titles, ["Work", "Home"]);
}

// ---- Auth-expiry retry (ADR-0002) ----

#[tokio::test]
async fn a_401_forces_a_refresh_and_retries_once() {
    let server = MockServer::start().await;
    // First attempt: 401 (served once, higher priority).
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // The forced-refresh retry sees 200.
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
        .with_priority(2)
        .mount(&server)
        .await;

    let lists = client(&server).list_lists().await.unwrap();
    assert!(lists.is_empty());
}

/// The 401 replay is a `try_clone` of the original builder, so anything set on
/// the request by hand — notably the explicit `Content-Length: 0` a bodyless
/// POST needs — has to survive onto the retry. Asserted on both mocks: drop the
/// header on either attempt and that mock stops matching.
#[tokio::test]
async fn a_401_retry_of_a_bodyless_post_keeps_content_length() {
    let server = MockServer::start().await;
    // First attempt: 401 (served once, higher priority).
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T1/move"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // The forced-refresh retry sees 200 — and still carries the header.
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T1/move"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1", "title": "moved", "etag": "e6",
            "updated": "2023-11-14T22:13:20.000Z", "status": "needsAction",
            "position": "00000000000000000000"
        })))
        .with_priority(2)
        .mount(&server)
        .await;

    let task = client(&server)
        .move_task_to_list(
            &ListId("L1".into()),
            &TaskId("T1".into()),
            &ListId("L2".into()),
        )
        .await
        .unwrap();
    assert_eq!(task.list, ListId("L2".into()));
}

#[tokio::test]
async fn a_persistent_401_surfaces_auth_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me/lists"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = client(&server).list_lists().await.unwrap_err();
    assert_eq!(err, ApiError::AuthExpired);
}
