//! oxidone — a single-user TUI for Google Tasks.
//!
//! The crate is split so the core (TEA model, `TasksApi` trait, cache, sync) is
//! testable without a terminal or a live Google account (see `docs/adr/0005`).
//! `main.rs` owns only terminal setup and the tokio runtime.

pub mod api; // TasksApi trait + reqwest impl + in-memory fake
pub mod app; // The Elm Architecture: Model, Message, update, view wiring
pub mod auth; // OAuth loopback flow + TokenStore trait
pub mod cache; // SQLite pure-mirror cache + completion_log
pub mod config; // TOML config + platform paths
pub mod dateparse; // pure natural-language + ISO due-date parser (local TZ)
pub mod domain; // Task, List, Subtask, Status, DueDate — the ubiquitous language
pub mod keymap; // keymap-as-data (context + key -> Action)
pub mod sync; // write-through, manual refresh, reconciliation
pub mod ui; // ratatui view + theme + braille widgets
