# oxidone

A single-user terminal UI for [Google Tasks](https://tasks.google.com), written in Rust.
A keyboard-driven daily-driver for triaging tasks across multiple lists — styled in
[btop](https://github.com/aristocratos/btop)'s visual language (rounded panels, braille
meters) with a [Catppuccin](https://catppuccin.com) palette.

> Status: **early**. The domain model and architecture are settled (see
> [`CONTEXT.md`](CONTEXT.md) and [`docs/adr/`](docs/adr)); the implementation is in progress.

## Why

Google Tasks is a deliberately thin model — no priorities, no tags, no due *times*,
no recurrence in the API. oxidone leans into that: it is a fast, honest **mirror** of
your tasks (what you see is what Google has, everywhere), not a heavier todo app wearing
a Tasks costume.

## Features (target v1)

- Multiple lists in a two-pane layout (list sidebar + task pane)
- Modeless single-key keybindings (`lazygit`-style), with a `?` cheatsheet
- Create / complete / edit / delete tasks; un-complete; clear completed
- Subtasks (one level, matching Google), indent/outdent, manual reorder
- Natural-language due dates (`tomorrow`, `mon`, `+3d`) with ISO fallback
- Notes edited in your `$EDITOR`
- Full list management (create / rename / delete)
- Local-order-preserving, plus throwaway sort views (by due, by title)
- Braille completion meters and a due-load histogram
- Instant startup from a local SQLite cache; works offline for *viewing*

Not planned: local-only priorities/tags/times (they wouldn't round-trip to Google),
recurrence and reminders (not exposed by the API).

## Install

Requires a Rust toolchain (stable). From source:

```sh
git clone https://github.com/erwins-enkel/oxidone
cd oxidone
cargo build --release
# binary at target/release/oxidone
```

Prebuilt binaries and `cargo install oxidone` will follow at the first tagged release.

## First-run setup (bring your own Google credentials)

oxidone does **not** ship a shared OAuth client — you use your own Google Cloud
project. This keeps the app out of Google's verification process and puts you fully in
control of your credentials. It's a one-time, ~10-minute setup.

1. **Create/select a project** in the [Google Cloud Console](https://console.cloud.google.com).
2. **Enable the Tasks API**: *APIs & Services → Library →* search "Google Tasks API" *→ Enable*.
3. **Configure the OAuth consent screen**: *APIs & Services → OAuth consent screen*.
   - User type **External** (or **Internal** on Workspace).
   - Fill in an app name and your email.
   - Add the scope `https://www.googleapis.com/auth/tasks`.
   - Add your Google account under **Test users**.
4. **Create credentials**: *APIs & Services → Credentials → Create credentials → OAuth client ID*.
   - Application type: **Desktop app**.
   - Download the JSON (`client_secret_*.json`).
5. **Point oxidone at it** — set the path in the config file (see below).
6. **Run `oxidone`**. It opens your browser to Google's consent screen; because the app is
   unverified and you're a test user, choose *Advanced → proceed*. A local `localhost`
   listener catches the redirect, and the refresh token is saved to your config dir.

## Configuration

Config lives at the platform config dir (`directories`):

- macOS: `~/Library/Application Support/oxidone/config.toml`
- Linux: `~/.config/oxidone/config.toml`

```toml
# Path to your downloaded OAuth client (step 4 above)
client_secret_path = "~/.config/oxidone/client_secret.json"

# Catppuccin flavor: "latte" | "frappe" | "macchiato" | "mocha"
theme = "mocha"

# Render ASCII block bars instead of braille (for terminals/fonts without braille glyphs)
ascii_fallback = false
```

The refresh token is stored `chmod 600` in the config dir. See
[ADR-0002](docs/adr/0002-byo-oauth-plaintext-token.md) for the security rationale.

## Development

```sh
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The core (TEA reducer, `TasksApi` trait, cache, sync) is testable with no terminal and
no live Google account — logic tests run against an in-memory fake API.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you shall be dual-licensed as above, without any
additional terms or conditions.
