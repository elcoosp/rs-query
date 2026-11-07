# ADR 0001 — Pin TypeScript to 6.x for `astro check`

- **Status:** Accepted
- **Date:** 2026-11-08
- **Scope:** `package.json` devDependency `typescript`

## Context

Astro 7.4's `astro check` does not support TypeScript 7. Running it with TypeScript 7.0.2 (the current `latest` on npm) fails with:

> `astro check does not currently support TypeScript 7.0`

An experimental workaround exists (`tsc` + `@astrojs/ts-content-mapper`), but it is not the stable default and the Astro docs point users at staying on TypeScript 6 for `astro check` for now.

## Decision

Pin `typescript` to the latest stable 6.x release — **`6.0.3`** (exact, no caret) — in `package.json` devDependencies. Verify at execution time with `pnpm view typescript version` (currently `7.0.2`, i.e. the wrong default) and keep the pin until Astro ships a `check` that supports TS 7.

## Consequences

- `pnpm check` runs on a supported TypeScript and reports 0 errors / 0 warnings.
- Dependabot-style bumps to TypeScript 7 must be rejected while this pin holds.
- Revisit when Astro announces `astro check` compatibility with TypeScript 7; then bump and delete this ADR's constraints.
