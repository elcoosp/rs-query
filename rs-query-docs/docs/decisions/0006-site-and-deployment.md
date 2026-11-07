# ADR 0006 — Site URL and base path

- **Status:** Accepted
- **Date:** 2026-11-08
- **Scope:** `site` / `base` in `astro.config.mjs`

## Context

Starlight 0.42 unconditionally injects its own `@astrojs/sitemap` integration (`dist/integrations/sitemap.js` — pushed when no user-supplied `@astrojs/sitemap` exists). Without the Astro `site` option, the build emits:

> `[WARN] [@astrojs/sitemap] The Sitemap integration requires the 'site' astro.config option. Skipping.`

There is no Starlight config option to opt out of the sitemap integration. The zero-warning release gate in force is a hard blocker and instructs to **fix the cause** rather than filter the warning — so `site` must be set. The only URL this project can claim without inventing a domain is the standard GitHub Pages project-site address for the repository that hosts these docs.

## Decision

```js
site: 'https://elcoosp.github.io',
base: '/rs-query',
```

This is the canonical GitHub Pages deployment layout for `github.com/elcoosp/rs-query` serving this `docs-site` build. Consequences:

- The skill guidance "do not add `site` or `base` unless required" is satisfied by the two concrete requirements above (sitemap warning + GitHub Pages subpath).
- Local QA runs against the preview server with `QA_BASE_URL=http://localhost:4321/rs-query`, so every route is exercised under the same base path production uses.
- `public/llms.txt` keeps relative links, so it stays correct under any base.
- If the project later deploys at a root domain, change both values here and re-run the full QA pass.
