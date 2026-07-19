//! Unit tests for `FileTokenStore` (ADR-0002): save/load/clear round-trip
//! against a temp file, and `0600` permissions on unix. No network.

use oxidone::auth::{FileTokenStore, TokenStore};

fn temp_store() -> (tempfile::TempDir, FileTokenStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(dir.path().join("token.json"));
    (dir, store)
}

#[test]
fn load_missing_is_none() {
    let (_dir, store) = temp_store();
    assert!(store.load().unwrap().is_none());
}

#[test]
fn save_then_load_round_trips() {
    let (_dir, store) = temp_store();
    store.save("the-token-blob").unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("the-token-blob"));
}

#[test]
fn save_overwrites_previous() {
    let (_dir, store) = temp_store();
    store.save("first").unwrap();
    store.save("second").unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("second"));
}

#[test]
fn clear_removes_the_file() {
    let (_dir, store) = temp_store();
    store.save("x").unwrap();
    store.clear().unwrap();
    assert!(store.load().unwrap().is_none());
}

#[test]
fn clear_missing_is_ok() {
    let (_dir, store) = temp_store();
    // Clearing a never-written store is a no-op, not an error.
    store.clear().unwrap();
}

#[test]
fn save_creates_missing_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("does/not/exist/token.json");
    let store = FileTokenStore::new(&nested);
    store.save("x").unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("x"));
}

#[cfg(unix)]
#[test]
fn saved_file_is_chmod_600() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, store) = temp_store();
    store.save("secret").unwrap();
    let mode = std::fs::metadata(store.path())
        .unwrap()
        .permissions()
        .mode();
    // Only the low 9 permission bits matter.
    assert_eq!(mode & 0o777, 0o600, "token file must be owner-only rw");
}
