# Changelog

## 0.1.0 (2026-07-20)

First tagged release — the v1 scope from [#1](https://github.com/erwins-enkel/oxidone/issues/1).

### Features

- **Auth**: bring-your-own Google OAuth client, loopback consent flow, refresh
  token stored `chmod 600`, transparent refresh-and-retry-once on expiry.
- **Lists**: sidebar navigation; create, rename, delete (destructive-confirm
  gated; Google's undeletable default List handled gracefully).
- **Tasks**: add, edit title, set/clear due date, edit notes in `$EDITOR` (inline
  single-line fallback), complete/un-complete, delete (confirm gated).
- **Completed handling**: hidden by default with a reveal toggle (struck-through),
  Clear to sweep to hidden, and a local append-only completion log.
- **Subtasks & reorder**: one-level Subtasks (indented), create/indent/outdent,
  and manual up/down reorder — all via Google's Move operation.
- **Due dates**: natural-language entry (`tomorrow`, `mon`, `+3d`) with ISO
  fallback, rendered relative to today; date-only per Google.
- **Sort views**: throwaway by-due / by-title lenses that never mutate Manual order.
- **Sync & cache**: instant startup from a local SQLite cache, viewable offline,
  optimistic write-through with rollback — a pure mirror of Google Tasks.
- **Visual design**: btop-style rounded panels, braille Completion meter and
  Due-load histogram, Catppuccin palette (four flavors), ASCII fallback, and a
  `?` keybinding cheatsheet.
