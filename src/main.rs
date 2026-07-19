//! Binary entry point: terminal setup + tokio runtime + event loop wiring.
//! All domain logic lives in the `oxidone` library crate.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Init tracing -> rotating file in the platform log dir (never stdout —
    //    it would corrupt the TUI).
    // 2. Load config (TOML); resolve BYO client_secret path + token store.
    // 3. Ensure auth: if no token, run the browser + loopback flow (auth::login).
    // 4. Open the SQLite cache; run migrations.
    // 5. Enter raw mode / alternate screen (crossterm); build ratatui Terminal.
    // 6. Spawn workers (api/sync) + the terminal-event reader; run the TEA loop:
    //    drain Messages -> update(model) -> view(&model) each tick.
    // 7. On exit: restore the terminal unconditionally (even on panic).
    todo!("wire runtime; see docs/adr/0005 for the loop shape")
}
