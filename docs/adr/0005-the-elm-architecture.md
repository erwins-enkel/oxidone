# The Elm Architecture for app state

App state is a single `Model`, all events (key presses, API results, timer ticks) are variants of one `Message` enum, and a single pure `update(model, msg)` reducer is the only place state changes. Rendering is a pure `view(&model)`. Async workers (auth, API, refresh) communicate by sending `Message`s over an `mpsc` channel into the same reducer.

TEA is the community-standard ratatui pattern for non-trivial apps and a near-perfect fit here: the pure `update` is unit-testable **without a terminal or a live Google account** (crucial for CI on an open-source project), and the future offline write-queue becomes new messages + reducer arms rather than a rewrite.

## Consequences

- Boilerplate: every action is a `Message` variant. Accepted as the "good kind" — explicit and greppable.
- Workers never mutate state directly; they only emit messages, keeping the reducer the single source of truth.
