# House rules for AI agents

oxidone is a single-user TUI for Google Tasks, in Rust. Read `CONTEXT.md` before
writing code — it defines the domain vocabulary (List, Task, Status, the four
exits) and the words to avoid. Architectural decisions live in `docs/adr/`;
don't relitigate one in a feature PR.

## The gate

One command decides whether the work is done. Run it; paste the output.

```sh
git config core.hooksPath .githooks   # once per clone — enables the hooks below

make gate                             # fmt · clippy · test · unused deps
make dev-tools                        # once, if `cargo machete` is missing
```

`make gate` is the single source of truth for the gate commands. `.githooks/pre-push`
and `.github/workflows/ci.yml` both run exactly `make gate`, so a green push is a
green CI — there is one definition to keep honest, not three copies to sync.

`.githooks/commit-msg` enforces Conventional Commits
(`feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert`), optionally via
cocogitto (`cargo install cocogitto`) when `cog` is on PATH.

**Never** reach for `--no-verify`, `#[allow(...)]`, or a
`[package.metadata.cargo-machete] ignored` entry to get past a failing gate. Fix
the underlying defect. If a lint is genuinely wrong, say so in the PR and scope
the allow to the single item with a comment explaining why.

## Engineering posture (surgical & mechanical)

- **Simplicity first.** Write the minimum code that solves the stated problem —
  no speculative features, abstractions for single use, or config nobody asked for.
- **Surgical changes.** Every changed line traces to the request. Don't refactor,
  reformat, or polish adjacent code; match existing style. Delete only what your
  change orphaned; surface pre-existing dead code rather than silently expanding
  the diff.
- **Fail closed.** Render error/unreachable paths as explicit failures; never let
  a swallowed error, empty result, or zero count masquerade as success. In this
  codebase that means: no `let _ =` on a `Result`, no `unwrap_or_default()` that
  turns a failed sync into "you have no Tasks."
- **Single source of truth.** Derive counts, totals, and dimensions from the data
  (slice length, column count) — never hardcode a magic number that silently drifts.
  The Completion meter and Due-load histogram compute from the Task set, always.
- **Keep names and comments honest.** When you change a function's behavior, update
  its name, doc comment, and inline comments to match — no stale comment left behind.
- **No dead code.** Delete unreachable branches and unused fields/params, or wire
  them to a real path, before opening a PR. Don't add a dependency until the code
  that uses it lands in the same change — `cargo machete` fails the gate on unused deps.
- **Verify before claiming done.** Run the gate; show the output. Evidence before
  assertions. "Should work" is not a result.

## Project-specific invariants

- **Pure mirror.** The live-Task cache models exactly what Google stores — no
  local-only fields, no augmentation (ADR-0003). The Completion log is the
  separate, per-machine, non-authoritative exception (ADR-0007).
- **Due dates are dates, never times.** Google discards the time portion; oxidone
  never stores or shows one.
- **Sort views are local and read-only.** They never mutate Manual order and never
  write to Google. Only a Move writes `position` or `parent`.
- **Subtasks nest one level.** A Subtask cannot have Subtasks.
- **Writes are optimistic with rollback.** On API failure, roll the local state back.
- **Core stays terminal-free.** The TEA reducer, `TasksApi` trait, cache, and sync
  are testable with no terminal and no live Google account — test against the
  in-memory fake API, not the network.

## Git

- Conventional Commits, imperative, concise: `feat(sync): pull Cleared Tasks on refresh`.
- Never commit to `main`; branch and open a PR.
- Use the `gh` CLI for GitHub operations.
