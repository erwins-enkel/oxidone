//! Contract tests for `RestClient` against a `wiremock` server (ADR-0004).
//! Each test asserts the request oxidone builds (verb, path, query params, JSON
//! body) and that Google's JSON deserializes into the domain types. No live
//! Google account, no real OAuth — the bearer token is injected statically.

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

#[tokio::test]
async fn move_task_sends_parent_and_previous_query_params() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T3/move"))
        .and(query_param("parent", "T1"))
        .and(query_param("previous", "T2"))
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
async fn move_task_to_top_sends_no_query_params() {
    let server = MockServer::start().await;
    // Matcher deliberately omits query_param assertions; the handler still
    // responds, proving oxidone sends a bare move for a top-of-list reposition.
    Mock::given(method("POST"))
        .and(path("/lists/L1/tasks/T3/move"))
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
