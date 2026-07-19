# BYO OAuth credentials, loopback flow, plaintext token store

Each user supplies their **own** Google Cloud OAuth client (`client_secret.json`) rather than oxidone shipping a shared client ID. Authentication uses the OAuth **loopback** flow (`localhost` listener) via `yup-oauth2`. The resulting refresh token is stored as a `chmod 600` plaintext file in the platform config dir, behind a `TokenStore` trait.

BYO avoids Google's app-verification treadmill and the "unverified app" screen entirely, keeps the repository free of any embedded secret, and is the honest posture for a niche technical TUI whose author does the Cloud setup once. Plaintext-600 (over an OS keychain) is chosen because the threat model is a personal machine, and a keychain adds a fragile C-dependency that prompts/hangs awkwardly over SSH — a common TUI complaint.

## Consequences

- First-run setup is a documented ~10-minute manual Cloud-project task; this narrows the audience to the technical, accepted deliberately.
- `TokenStore` is a trait so an OS-keychain backend (with plaintext fallback) is a clean later addition without touching call sites.
- A shared client ID for frictionless onboarding is a future option, not v1.
