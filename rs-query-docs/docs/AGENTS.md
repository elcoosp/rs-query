# AGENTS.md — working on the rs-query docs site

This is the **documentation site** for [rs-query](https://github.com/elcoosp/rs-query), built with Astro Starlight. It is a standalone deliverable: the crate source, `node_modules`, build output, and QA artifacts are never part of it.

## Stack

- Astro **7.4.x** + `@astrojs/starlight` **0.42.x** (floor; do not downgrade)
- TypeScript pinned to **6.0.3** — TypeScript 7 breaks `astro check` (see `docs/decisions/0001-typescript-version.md`)
- pnpm only. No npm/yarn lockfiles.
- Search: Pagefind (built into Starlight). No custom highlighter — Shiki via Expressive Code is the default and must stay (see `docs/decisions/0003-syntax-highlighter.md`).

## Commands

```bash
pnpm install        # once
pnpm dev            # local dev server
pnpm build          # generates public/llms*.txt then astro build
pnpm check          # astro check — must be 0 errors / 0 warnings
pnpm lint           # markdownlint-cli2 over src/content/docs
pnpm preview        # serve dist/ on :4321
pnpm qa             # screenshot every route × 2 viewports × 2 themes (needs preview running)
pnpm qa:search      # Pagefind modal probe
```

## Content rules

- **Diátaxis, one type per page.** Folders: `tutorials/` (learning), `guides/` (how-to), `reference/` (API), `explanation/` (understanding). Every page carries `type` in frontmatter, enforced by the extended docs schema in `src/content.config.ts`. The landing page uses `type: overview` (the only exception).
- **Accuracy is sourced.** Every signature must match the crate source. Do not document methods, options, or behaviors that do not exist in `src/` of the rs-query repository.
- Writing style: second person, present tense, active voice, imperative instructions. Never "simply", "just", "easy", "obviously".
- Every page: `title`, `description`, `type` frontmatter; ends with `## Next steps` using `<LinkCard>`.
- Code blocks always declare a language. Package-manager tabs put the recommended path first.
- Internal links are validated at build time by `starlight-links-validator` — a broken link fails the build.

## Branding

- Accent derived from the logo's orange ribbon (hue ≈ 30°). Values live in `src/styles/custom.css` with contrast numbers in comments. Do not reintroduce Starlight's default blue.
- Logo assets are generated from the crate's raster `docs/logo.png` — see `docs/decisions/0002-logo-format.md` before touching them.

## QA gate (release-blocking)

1. `pnpm build` exits 0 with zero warning lines.
2. `pnpm check` reports 0 errors / 0 warnings.
3. `pnpm lint` is clean.
4. `pnpm preview` + `pnpm qa` exits 0 over **every** built route, both viewports (1440×900, 390×844) and both themes.
5. Eyeball the screenshots (see `docs/qa-report.md` for the last pass) and update that report after content/branding changes.

Any change to content, config, CSS, or assets invalidates the QA pass — re-run all five steps.
