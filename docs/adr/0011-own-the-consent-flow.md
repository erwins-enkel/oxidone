# Own the consent flow, and accept a pasted callback URL

oxidone builds the authorization URL, runs the loopback listener, and performs the `authorization_code` exchange itself, with PKCE (S256) and `state`. `yup-oauth2`'s `InstalledFlowAuthenticator` — and with it our flow delegate and the `TokenStorage` adapter — is retired; the crate stays for `read_application_secret`, `ApplicationSecret` and the `TokenInfo` shape the store persists. Alongside the redirect, the flow accepts a **callback URL pasted back by hand**, and whichever arrives first settles it.

This narrows ADR-0002 (which named `yup-oauth2` as the consent flow) and extends ADR-0009's reasoning from the refresh exchange to the code exchange: the decision we need lives between two of the library's own calls, where no caller can reach it.

The decision is forced by the machine oxidone is most useful on and least able to authorize on: one reached over SSH. The loopback listener binds there quite happily, but the browser is on the *other* machine, where `localhost` is somebody else's `localhost`, so the redirect never arrives. Port-forwarding is the usual answer and is a poor one — it has to be arranged before you are locked out, on a port you do not know until the flow picks it.

`yup-oauth2` offers no seam for that. `HTTPRedirect` owns the listener *and* the exchange with nothing in between, and its `Interactive` mode binds no listener at all — so it could only ever be a mode chosen in advance, which is the thing being avoided. Owning the flow makes the paste a second road rather than a second mode.

## Consequences

- **Nothing changes for a local run.** The listener still binds, the browser still opens, and the redirect still settles the flow with nothing typed.
- **The flow became testable.** It was "compile-verified only" by its own admission. The URL builder, the callback parser, PKCE, the listener and the exchange are now all exercised with no browser, no terminal and no Google account (`tests/auth_consent_flow.rs`), which is what the core-stays-terminal-free invariant asks for everywhere else.
- **PKCE and `state` are not optional here.** The pasted redirect lands on the *browser's* loopback, where oxidone is not listening and anything else might be: `state` rejects a callback that does not answer this attempt, and PKCE makes a code intercepted there useless without the verifier, which never leaves the process.
- **`prompt=consent` is now sent.** Google omits the refresh token when re-authorizing a client the user has already granted, and a grant with no refresh token behind it sends the user back to the browser as soon as its access token expires. Such a response is refused rather than stored.
- **`redirect_uri` moved from `http://localhost:PORT` to `http://127.0.0.1:PORT`**, which is what Google's installed-app guidance recommends: `localhost` resolving to `::1` on the user's machine is a known cause of a redirect that simply hangs.
- **Three small dependencies** (`sha2`, `base64`, `getrandom`) for the S256 challenge and the nonces. SHA-256 is not something to hand-roll.
- **`CONSENT_TIMEOUT` is ten minutes**, not three. Copying a URL between machines outlasts a browser flow on the same desk, and a timeout landing mid-paste throws the whole attempt away.
- **A callback that does not parse is retryable.** It is reported where the retry happens and the flow keeps waiting; only the token exchange settles a consent.
