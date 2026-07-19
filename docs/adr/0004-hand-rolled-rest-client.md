# Hand-rolled REST client over the generated `google-tasks1`

The Google Tasks HTTP calls are written by hand with `reqwest` against our own thin request/response types, rather than depending on the generated `google-tasks1` crate from `google-apis-rs`. Auth is *not* hand-rolled — the fiddly, security-sensitive OAuth loopback and token refresh use `yup-oauth2`.

The Tasks API is tiny and stable (2 resources, ~11 methods, flat JSON). A generated client's verbose builder API would be wrapped into our own cache structs anyway, leaving us maintaining a translation layer over a translation layer. Owning ~300 lines of thin endpoint wrappers keeps the domain types clean and the dependency surface small and auditable — a virtue for an open-source project.

## Consequences

- We take on maintaining the wrappers if Google changes the API (rare for this endpoint).
- A `TasksApi` trait sits over the client so logic/sync code tests against an in-memory fake; a thin `wiremock` suite guards the real client's request-building and (de)serialization.
