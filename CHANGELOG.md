# Changelog

## [1.0.0](https://github.com/erwins-enkel/oxidone/compare/v0.1.0...v1.0.0) (2026-09-08)


### ⚠ BREAKING CHANGES

* **sort:** open in due order, keep Subtasks grouped ([#58](https://github.com/erwins-enkel/oxidone/issues/58))

### Features

* **add:** parse a trailing natural-language date off the captured title ([#80](https://github.com/erwins-enkel/oxidone/issues/80)) ([bf5175b](https://github.com/erwins-enkel/oxidone/commit/bf5175b811e4c09eea40a9fc49114238e6573720))
* Bullet Journal mode — entry types and migration ([#60](https://github.com/erwins-enkel/oxidone/issues/60)) ([0422432](https://github.com/erwins-enkel/oxidone/commit/0422432a30c59783ad66b8ef6b2543c2f1776e68))
* **due:** make the due editor fast to retype, nudge and read ([#106](https://github.com/erwins-enkel/oxidone/issues/106)) ([da023a7](https://github.com/erwins-enkel/oxidone/commit/da023a738730f7805870d7cc8d70b532e743f14a))
* **filter:** find Tasks by title/notes via / ([#96](https://github.com/erwins-enkel/oxidone/issues/96)) ([d4f3814](https://github.com/erwins-enkel/oxidone/commit/d4f38148142e2480649c7021c925aca561aaa94a))
* **links:** open URLs from a Task's notes with u ([#57](https://github.com/erwins-enkel/oxidone/issues/57)) ([e690ed1](https://github.com/erwins-enkel/oxidone/commit/e690ed15147f2d8434778f463de1db5488ff93b1))
* **move:** relocate a Task to another List with M ([#85](https://github.com/erwins-enkel/oxidone/issues/85)) ([9c16003](https://github.com/erwins-enkel/oxidone/commit/9c16003310f7cfe7d6b13bcee11a2cd096840599))
* **move:** type-ahead filter in the move-to-list picker ([#125](https://github.com/erwins-enkel/oxidone/issues/125)) ([f027314](https://github.com/erwins-enkel/oxidone/commit/f027314c1ee5046f84ab03cd1a9102702daaa7a7))
* **omnibox:** move the selected Task to another List ([#126](https://github.com/erwins-enkel/oxidone/issues/126)) ([195ac24](https://github.com/erwins-enkel/oxidone/commit/195ac244ca940b5fe80ffb9d8cbc75604b2ac906))
* **omnibox:** one query surface for jumping, running and capturing (`p`) ([#108](https://github.com/erwins-enkel/oxidone/issues/108)) ([29d178b](https://github.com/erwins-enkel/oxidone/commit/29d178bc430814479e4c9b877b7a949df8c57c05))
* **omnibox:** reach the manual Refresh by name (`:refresh`) ([#110](https://github.com/erwins-enkel/oxidone/issues/110)) ([f6af613](https://github.com/erwins-enkel/oxidone/commit/f6af61353316963798b2d9b8e5e029ab8855fc17))
* **overlay:** put a caret in the text popups (←/→, ^A/^E, Home/End, Delete) ([#112](https://github.com/erwins-enkel/oxidone/issues/112)) ([4647bc5](https://github.com/erwins-enkel/oxidone/commit/4647bc59e08419ae67b787a885613c3048e2febc))
* **search:** global cross-List Search pane (S) ([#101](https://github.com/erwins-enkel/oxidone/issues/101)) ([f9394e7](https://github.com/erwins-enkel/oxidone/commit/f9394e7e8948a165ba9e6242b6628fb6ca6f0890))
* **sort:** open in due order, keep Subtasks grouped ([#58](https://github.com/erwins-enkel/oxidone/issues/58)) ([f9c451b](https://github.com/erwins-enkel/oxidone/commit/f9c451bbb8149c6ac11cbd5856589e8b61ab205b))
* **sync:** fill sidebar meters for Lists never opened here ([#72](https://github.com/erwins-enkel/oxidone/issues/72)) ([dc0664e](https://github.com/erwins-enkel/oxidone/commit/dc0664efa70c6451b35779d602cbeb7de75af680)), closes [#63](https://github.com/erwins-enkel/oxidone/issues/63)
* **sync:** manual Refresh key (r) pulls latest from Google ([#52](https://github.com/erwins-enkel/oxidone/issues/52)) ([aa1ed91](https://github.com/erwins-enkel/oxidone/commit/aa1ed913e5f9297c47f47c96dbe7334cbd90fb03))
* **sync:** mirror Google's Task links[] alongside notes-derived URLs ([#70](https://github.com/erwins-enkel/oxidone/issues/70)) ([458aca7](https://github.com/erwins-enkel/oxidone/commit/458aca7e836a3942c32776d9e3d8bc11b0c17db0)), closes [#55](https://github.com/erwins-enkel/oxidone/issues/55)
* **sync:** move a Task with its Subtasks, and mirror the children ([#93](https://github.com/erwins-enkel/oxidone/issues/93), [#94](https://github.com/erwins-enkel/oxidone/issues/94)) ([#111](https://github.com/erwins-enkel/oxidone/issues/111)) ([675acac](https://github.com/erwins-enkel/oxidone/commit/675acac3b2e804a00d508bed6667e401a21d9b77))
* **today:** hide completions from earlier days ([#84](https://github.com/erwins-enkel/oxidone/issues/84)) ([d9740fc](https://github.com/erwins-enkel/oxidone/commit/d9740fc0643fa9631e00789577c70bb4707bd9a6))
* **today:** pinned cross-List view of what's due ([#61](https://github.com/erwins-enkel/oxidone/issues/61)) ([#82](https://github.com/erwins-enkel/oxidone/issues/82)) ([b9fcb31](https://github.com/erwins-enkel/oxidone/commit/b9fcb318e0fad35cfa01f860acc6b322e50fa168))
* **today:** render Today as a journal spread ([#62](https://github.com/erwins-enkel/oxidone/issues/62)) ([#91](https://github.com/erwins-enkel/oxidone/issues/91)) ([79a6c5c](https://github.com/erwins-enkel/oxidone/commit/79a6c5c3a6db315e3aaeeef8b67087718ff60da3))
* **ui:** always-visible hotkey legend below the status line ([#53](https://github.com/erwins-enkel/oxidone/issues/53)) ([97baa8d](https://github.com/erwins-enkel/oxidone/commit/97baa8d135af9d3518784e001026bed82a7fb0ab))
* **ui:** dim Today's List name below the notes preview ([#83](https://github.com/erwins-enkel/oxidone/issues/83)) ([1f6ff5e](https://github.com/erwins-enkel/oxidone/commit/1f6ff5e6ad395adf848ca5da25b58a0b4e99d79b))
* **ui:** inline notes preview in the task pane ([#75](https://github.com/erwins-enkel/oxidone/issues/75)) ([a9ddce9](https://github.com/erwins-enkel/oxidone/commit/a9ddce978c36f707e21d95000740fdbb7c7e7f7b))
* **ui:** mark Tasks carrying notes with ≡ in the task pane ([#69](https://github.com/erwins-enkel/oxidone/issues/69)) ([29c8198](https://github.com/erwins-enkel/oxidone/commit/29c81986be8a1486ce48424d5afc471ea1cae9e7)), closes [#54](https://github.com/erwins-enkel/oxidone/issues/54)
* **ui:** per-List sidebar meter and per-parent Subtask meter ([#64](https://github.com/erwins-enkel/oxidone/issues/64)) ([a105f77](https://github.com/erwins-enkel/oxidone/commit/a105f7786b74860c03892980264a4870570b1f70))
* **view:** hide tasks due beyond a configurable horizon ([#81](https://github.com/erwins-enkel/oxidone/issues/81)) ([ca26cc3](https://github.com/erwins-enkel/oxidone/commit/ca26cc3da4209739959c071816ffc41b33fdab73))
* **week:** make Week a selectable sidebar row, scoped to the selected List ([#119](https://github.com/erwins-enkel/oxidone/issues/119)) ([002b466](https://github.com/erwins-enkel/oxidone/commit/002b466b09781d44adc232329ac0bafdb8a5474b))
* **week:** weekly spread with a Mon–Fri day grid ([#117](https://github.com/erwins-enkel/oxidone/issues/117)) ([3da47f6](https://github.com/erwins-enkel/oxidone/commit/3da47f6dddfecf9c65776ea71a430c81d0e8cb02))


### Bug Fixes

* **api:** back off and retry rate limits at the send seam ([#99](https://github.com/erwins-enkel/oxidone/issues/99)) ([30df349](https://github.com/erwins-enkel/oxidone/commit/30df349a22438c4b3798fb4a92f51f2f42ee83c9)), closes [#89](https://github.com/erwins-enkel/oxidone/issues/89)
* **api:** follow nextPageToken on both collection reads ([#87](https://github.com/erwins-enkel/oxidone/issues/87)) ([#90](https://github.com/erwins-enkel/oxidone/issues/90)) ([9d20d6a](https://github.com/erwins-enkel/oxidone/commit/9d20d6a8e4bdb398a0475ba715bb8fc4c756db93))
* **api:** set Content-Length on bodyless POSTs, unblocking Move ([#88](https://github.com/erwins-enkel/oxidone/issues/88)) ([fa90037](https://github.com/erwins-enkel/oxidone/commit/fa90037e761ec110a60bb62c048450c46180660e))
* **api:** share one sleep budget across a paginated read ([#98](https://github.com/erwins-enkel/oxidone/issues/98)) ([#104](https://github.com/erwins-enkel/oxidone/issues/104)) ([376bc6b](https://github.com/erwins-enkel/oxidone/commit/376bc6bff950301cbd6af74c49d68ecf75498e4d))
* **auth:** only a grant Google refused may open a browser ([#124](https://github.com/erwins-enkel/oxidone/issues/124)) ([dd30e5f](https://github.com/erwins-enkel/oxidone/commit/dd30e5f46496dd646eaaa1e8b18e01abdfba43ba))
* **auth:** show the consent prompt in the frame, and only once ([#118](https://github.com/erwins-enkel/oxidone/issues/118)) ([45e3157](https://github.com/erwins-enkel/oxidone/commit/45e3157bb7cbba46c01d779bd1cc7b1da3b29940))
* **config:** expand ~ in config.toml paths ([#74](https://github.com/erwins-enkel/oxidone/issues/74)) ([8707797](https://github.com/erwins-enkel/oxidone/commit/8707797cdcb81cf13c43007bc21b05e33019cedf)), closes [#67](https://github.com/erwins-enkel/oxidone/issues/67)
* **dateparse:** screen date words against an exact vocabulary ([#109](https://github.com/erwins-enkel/oxidone/issues/109)) ([203b1f2](https://github.com/erwins-enkel/oxidone/commit/203b1f25f768473c8b5fbcc43b0d17c9be320164))
* **delete:** re-remove a row a refresh resurrected mid-delete ([#66](https://github.com/erwins-enkel/oxidone/issues/66)) ([85cf28c](https://github.com/erwins-enkel/oxidone/commit/85cf28cd4ab8204e03f127616ca207e53c70791e)), closes [#51](https://github.com/erwins-enkel/oxidone/issues/51)
* **help:** lay the ? cheatsheet out to fit the terminal ([#59](https://github.com/erwins-enkel/oxidone/issues/59)) ([036f016](https://github.com/erwins-enkel/oxidone/commit/036f01671bbd0033664adc629b502e94572fd724))
* **sync:** tombstone confirmed deletes so a stale refresh can't resurrect them ([#73](https://github.com/erwins-enkel/oxidone/issues/73)) ([0cb80dd](https://github.com/erwins-enkel/oxidone/commit/0cb80ddc87659fd87180cb297cc88ff8a7376d92)), closes [#65](https://github.com/erwins-enkel/oxidone/issues/65)
* **today:** drop the redundant Today header when nothing is overdue ([#100](https://github.com/erwins-enkel/oxidone/issues/100)) ([fc6a1a6](https://github.com/erwins-enkel/oxidone/commit/fc6a1a690a3097a4ebaff2746301dba9793b0904))
* **ui:** draw the input caret with the terminal cursor ([#127](https://github.com/erwins-enkel/oxidone/issues/127)) ([e2a48db](https://github.com/erwins-enkel/oxidone/commit/e2a48dba0321129fbce3cf97873faeadef2cf075))
* **ui:** repaint on terminal resize ([#92](https://github.com/erwins-enkel/oxidone/issues/92)) ([bc2b65c](https://github.com/erwins-enkel/oxidone/commit/bc2b65cd7ebc7dcd0ee8cb68330eb0a135fa36c1))

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
