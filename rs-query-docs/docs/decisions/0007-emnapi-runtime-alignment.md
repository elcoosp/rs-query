# ADR 0007 — Align `@emnapi/runtime` to 1.11.1 so `astro check` runs

- **Status:** Accepted
- **Date:** 2026-11-08
- **Scope:** `pnpm.overrides` in `package.json`

## Context

`astro check` hung indefinitely on Node 24. Running the underlying `astro-check` binary directly surfaced the real failure, which the astro CLI swallowed:

```
TypeError: this.bridge.setLastError is not a function
    at NodeEnv.clearLastError (@emnapi/runtime 2.0.0-beta.2 emnapi.js:798)
```

Dependency analysis (`pnpm why @emnapi/runtime`):

- The markdown pipeline (`satteri` / `@bruits/satteri-wasm32-wasi`, used by Astro 7.4 and all three Starlight plugins) declares `@emnapi/core` + `@emnapi/runtime` **1.11.1** for its WASM bindings.
- `typescript@6.0.3` ships a WASM toolchain that pulls `@emnapi/runtime` **2.0.0-beta.2**.
- pnpm resolved `@astrojs/check`'s emnapi peer to the beta, producing an incompatible pair: WASM modules built for the 1.x N-API bridge executing against the 2.0.0-beta runtime. `astro build` resolved the compatible 1.x pair and worked; the `check` entry path resolved the beta and crashed → hang.

## Decision

Force a single emnapi runtime across the tree:

```json
"pnpm": { "overrides": { "@emnapi/runtime": "1.11.1" } }
```

This is the version the WASM markdown pipeline declares, and `astro check` does not exercise TypeScript 6's new WASM server — it uses the TypeScript library API through `@astrojs/language-server`.

## Consequences

- `astro-check --help` responds and `astro check` completes: **0 errors / 0 warnings / 0 hints**.
- If a future dependency genuinely requires the emnapi 2.x runtime, revisit this override and re-run the full QA pass.
