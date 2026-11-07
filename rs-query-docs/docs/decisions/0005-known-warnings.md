# ADR 0005 — Known warnings

- **Status:** Accepted (empty)
- **Date:** 2026-11-08
- **Scope:** Zero-warning release gate

## Policy

Any `warn`/`warning`/`error` line in `pnpm build`, `pnpm check`, or `pnpm lint` is a release blocker. The only permitted exception is a warning originating in a third-party dependency that cannot be fixed from this repository. Such an exception must be recorded here with the exact log line, the reason, and the narrow `grep -v` filter used — blanket suppression is forbidden.

## Current allowlist

None. The final QA run of the current build (`docs/qa-report.md`) produced:

- `pnpm build` — 0 warning lines
- `pnpm check` — 0 errors, 0 warnings, 0 hints
- `pnpm lint` — clean

No `grep -v` filters exist in the QA pipeline.
