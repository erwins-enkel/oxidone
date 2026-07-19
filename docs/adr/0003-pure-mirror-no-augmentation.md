# Pure mirror — no local augmentation of the task model

oxidone models **exactly** what the Google Tasks API models — no local-only priorities, tags, due *times*, recurrence, or custom fields. The live-task cache is a strict mirror: whatever you see is what Google has, and when Google clears or deletes a Task, the mirror drops it too.

The entire value of a Google Tasks client is that it *is* Google Tasks — the same tasks appear on the web, the phone, and every other client. Any local-only field silently forks that data: it would be invisible everywhere else and would demand a second sync problem across the user's machines. We say no to that.

## Consequences

- If a specific augmentation ever becomes unavoidable, the *only* sanctioned escape hatch is encoding it as text in the `notes`/`title` field (a "convention"), because that round-trips through Google. No sidecar store of extra fields.
- This is a deliberate *no* to the priorities/tags/due-times a "daily driver" might tempt us toward.
- Note the one deliberate exception, scoped narrowly and kept out of the mirror: the Completion log (ADR-0007).
