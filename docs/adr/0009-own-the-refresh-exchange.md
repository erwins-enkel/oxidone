# oxidone owns the refresh-token exchange

The `grant_type=refresh_token` call to Google's token endpoint is made by oxidone
(`src/auth/refresh.rs`, over `reqwest`), not by `yup-oauth2`. `yup-oauth2` keeps exactly one
job: the interactive loopback consent flow. This narrows the auth clause of ADR-0004.

`yup_oauth2::Authenticator::find_token_info` answers *every* failed refresh by running the
whole installed flow — a revoked grant and a connection reset on resume-from-sleep are the
same event to it. That is a browser window for a network blip, and nothing in the log to say
which of the two happened. The distinction is not available to a caller: it is made inside
the library, between two of its own calls.

Owning the exchange puts that decision in oxidone, where it is a pure function over Google's
status and body: only a refusal Google itself labelled `invalid_grant` — or a stored blob
with no refresh token to send — may reach consent. Everything else keeps its own `ApiError`
class and leaves the stored grant alone. The exchange is a form POST and one JSON shape, the
same trade ADR-0004 already made for the Tasks endpoints, and unlike yup's refresh path it is
testable against `wiremock` with no browser and no Google account.

## Consequences

- The refresh half of the flow is ours to maintain, including carrying the stored refresh
  token forward when Google's response omits one — which is Google's normal behaviour.
- `yup-oauth2` is still the consent flow, and its internal fall-through still lives there. It
  is unreachable on our path only because the store is cleared before we hand off; a future
  change that stops clearing first would silently reopen it.
- A failed *persist* is now distinguishable from a failed *request*
  (`ApiError::TokenNotPersisted`). A token that is acquired and never written is what makes a
  grant look like it dies daily, and it no longer reads as a transient network error.
- Access-token caching stays where it was — the stored `TokenInfo` and
  `TokenInfo::is_expired()` — so there is one expiry margin in the codebase, not a second of
  our own invention.
