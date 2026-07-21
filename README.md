# oxidone

A single-user terminal UI for [Google Tasks](https://tasks.google.com), written in Rust.
A keyboard-driven daily-driver for triaging tasks across multiple lists — styled in
[btop](https://github.com/aristocratos/btop)'s visual language (rounded panels, braille
meters) with a [Catppuccin](https://catppuccin.com) palette.

> Status: **0.1.0** — the v1 scope ([#1](https://github.com/erwins-enkel/oxidone/issues/1))
> is implemented. The domain model and architecture are settled (see
> [`CONTEXT.md`](CONTEXT.md) and [`docs/adr/`](docs/adr)).

## Why

Google Tasks is a deliberately thin model — no priorities, no tags, no due *times*,
no recurrence in the API. oxidone leans into that: it is a fast, honest **mirror** of
your tasks (what you see is what Google has, everywhere), not a heavier todo app wearing
a Tasks costume.

## Features (target v1)

- Multiple lists in a two-pane layout (list sidebar + task pane)
- Modeless single-key keybindings (`lazygit`-style), with a `?` cheatsheet and
  an always-visible legend of the most common keys for the focused pane
- Create / complete / edit / delete tasks; un-complete; clear completed
- Subtasks (one level, matching Google), indent/outdent, manual reorder
  (from a sort view, the first reorder key switches back to "my order")
- Move a task to another list (`M`), from a list or from Today
- Natural-language due dates (`tomorrow`, `mon`, `+3d`) with ISO fallback, shown
  relative to today (`tomorrow`, `in 3d`, `2d ago`) until a week out
- Bullet Journal entry types — Task, Event (`○`), Note (`—`) — flipped with
  `t`/`T`, and migration (`m`) to push an entry forward a day
- Notes edited in your `$EDITOR`
- Full list management (create / rename / delete)
- Opens in due order, Subtasks still grouped under their parent; `s` cycles
  due → title → "my order" (Google's), the only view a reorder writes to
- Braille completion meters and a due-load histogram
- Instant startup from a local SQLite cache; works offline for *viewing*

Not planned: local-only priorities/tags/times (they wouldn't round-trip to Google),
recurrence and reminders (not exposed by the API).

## Install

oxidone supports terminals of 80x24 and up; the `?` cheatsheet is laid out to fit
that in full, and says so on its bottom border if it ever cannot.

### Prebuilt binaries

Each [release](https://github.com/erwins-enkel/oxidone/releases) ships prebuilt
binaries for Linux (x86-64), macOS (Apple Silicon), and Windows (x86-64).
Download the archive for your platform, extract, and put `oxidone` on your `PATH`:

```sh
# Linux / macOS example (adjust tag + target)
tar xzf oxidone-v0.1.0-aarch64-apple-darwin.tar.gz
install oxidone-v0.1.0-aarch64-apple-darwin/oxidone /usr/local/bin/
```

### From source

Requires a Rust toolchain (stable). With `make`:

```sh
git clone https://github.com/erwins-enkel/oxidone
cd oxidone
make install          # builds and installs to ~/.local/bin (override: PREFIX=/usr/local)
```

Later, to update to the latest commit and reinstall in one step:

```sh
make update           # git pull --ff-only, then make install
```

`make install` installs to `$PREFIX/bin` (default `~/.local/bin`); make sure that
directory is on your `PATH`. Plain `cargo build --release` still works too (binary
at `target/release/oxidone`). `cargo install oxidone` (crates.io) is planned.

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

Run `make config` to drop a starter file (a copy of
[`config.example.toml`](config.example.toml)) there if none exists; it never
overwrites an existing config. The fields:

```toml
# Absolute path to your downloaded OAuth client (step 4 above).
# Must be absolute — `~` is NOT expanded.
client_secret_path = "/home/you/.config/oxidone/client_secret.json"

# Catppuccin flavor: "latte" | "frappe" | "macchiato" | "mocha"
theme = "mocha"

# Render ASCII block bars instead of braille (for terminals/fonts without braille glyphs)
ascii_fallback = false

# Hide entries due far out from the task pane (`w` toggles it live). Entries due
# more than `horizon_days` days out are hidden; undated entries always stay.
hide_distant = false
horizon_days = 14
```

The refresh token is stored `chmod 600` in the config dir. See
[ADR-0002](docs/adr/0002-byo-oauth-plaintext-token.md) for the security rationale.

## Development

Once per clone, point Git at the committed hooks:

```sh
git config core.hooksPath .githooks
```

That enables a `pre-push` hook mirroring CI (fmt · clippy · test · unused deps) and a
`commit-msg` hook enforcing [Conventional Commits](https://www.conventionalcommits.org/).

The gate lives in one place — the `Makefile` — and both the hook and CI run it:

```sh
make gate         # fmt · clippy · test · unused deps — the full gate
make check        # cargo check --all-targets --all-features — fast inner loop
make dev-tools    # once, installs cargo-machete (needed by the gate)
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
