# A machine-readable JSON CLI is a public contract

`oxidone json` is a second entry point onto the same core the TUI drives: read
subcommands print JSON to stdout, and every write goes through one `apply` subcommand
that reads its command as JSON on stdin. It exists so another process — first the
Omarchy bar plugin — can read and change tasks without a terminal, and unlike the
`--version`/`--help`/`--print-config-path` flags beside it, its output shape is something
callers depend on and we therefore promise.

ADR-0005 already bought this: the TEA core, the `TasksApi` trait, auth and the domain
types have no dependency on `ratatui`, and 86 integration test files already drive them
with no terminal and no Google account. The CLI adds an entry point, not an architecture.

Writes take their payload on stdin rather than in flags because argv is not private:
`/proc/<pid>/cmdline` is readable by every process running as the same user, so a
`--title` flag would publish task titles and notes to anything else on the machine.
Reads stay ordinary subcommands — they carry ids and list names, not content, and a
contract nobody can run by hand is a contract nobody can debug.

Two shapes were rejected. A long-lived `--serve` daemon speaking JSON lines would make
actions instant and could push changes, but it buys that with supervision, restart and
backoff policy, and a far larger protocol — for a caller that polls every five minutes.
And the CLI deliberately does **not** open the SQLite cache: a second writer would end
the single-writer design ADR-0001 rests on, and would need WAL and busy-timeout handling
to buy a consistency the caller can get by keeping its own last-known-good copy.

## Consequences

- The JSON shapes are versioned by oxidone's own release number, and callers gate on it.
  Changing a field is a breaking change with a visible cost, which is the point of writing
  this down.
- `oxidone json` is network-only. It has no offline read path, and a change made through it
  does not appear in a running TUI until the next Refresh — the same way a change made on a
  phone does not. The **pure mirror** stays pure.
- Two oxidone processes may now want to refresh the same grant, and `SingleFlight` only
  coalesces within one process. The refresh exchange (ADR-0009) takes a file lock so the TUI
  and a CLI call cannot both POST `grant_type=refresh_token` and race to rewrite
  `token.json`.
- Due-date parsing is reachable from outside the TUI, so a caller can offer the same
  `tomorrow` / `mon` / `+3d` vocabulary the `d` key does instead of inventing a second one.
