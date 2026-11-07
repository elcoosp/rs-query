# ADR 0004 — Plugin selection for a Rust crate's docs

- **Status:** Accepted
- **Date:** 2026-11-08
- **Scope:** Starlight plugins in `astro.config.mjs`

## Context

The toolbelt in force lists seven candidate plugins. Compatibility with Astro 7.4.7 / Starlight 0.42.10 was verified per plugin (`peerDependencies` allow `astro >=7.2.10`, `@astrojs/starlight >=0.41/0.42`), so compatibility is not the deciding factor — relevance is.

## Decision

**Installed:**

| Plugin                      | Why                                                                        |
| --------------------------- | -------------------------------------------------------------------------- |
| `starlight-links-validator` | Fails the build on broken internal links — a release blocker in force.      |
| `starlight-image-zoom`      | Click-to-zoom for brand assets and any future figures. Zero config.         |
| `starlight-heading-badges`  | `:badge[…]` markers on headings for feature-gated pages (`serde`, `devtools`). |

**Omitted, with reasons:**

| Plugin                 | Reason                                                                                                                             |
| ---------------------- | ------------------------------------------------------------------ |
| `starlight-typedoc`     | Generates API reference from **TypeScript**. rs-query is a Rust crate; its API reference is hand-written from the audited source.   |
| `starlight-openapi`     | No OpenAPI specification exists in the repository.                                                                                  |
| `starlight-blog`        | No blog content authored; the crate's CHANGELOG lives in the repository. A blog section would be an empty shell.                    |
| `starlight-versions`    | Single-version docs for a 0.2 crate; the plugin is early development and versioning is not requested.                               |

## Custom frontmatter (`type`)

The docs schema (`src/content.config.ts`) extends Starlight's schema with `type: tutorial | how-to | reference | explanation | overview` — the Diátaxis classification. `overview` is reserved for the two pages that are not Diátaxis content: the landing page hub (`index.mdx`) and the 404 page (`404.mdx`). The value is consumed by `scripts/generate-llms.mjs` to group `llms.txt` sections.

## Consequences

- `pnpm build` fails on broken internal links rather than shipping them.
- If a TypeScript or OpenAPI surface ever appears in the project, revisit the omissions above.
