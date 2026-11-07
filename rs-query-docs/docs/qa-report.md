# QA report — rs-query docs site

- **Build under test:** `pnpm build` — 30 pages, zero warning lines
- **`astro check`:** 0 errors / 0 warnings / 0 hints (TypeScript 6.0.3)
- **`pnpm lint`:** 0 issues across 30 MDX files
- **Automated visual QA:** `pnpm qa` — 29 routes × 2 viewports (1440×900, 390×844) × 2 themes (light, dark) = 116 screenshots, 0 failures
- **Search QA:** Pagefind modal on `/`, query `invalidate` → 14 results with titles + excerpts (`.qa/search-modal.png`)
- **404:** captured at `.qa/404.png` (excluded from the route walk by design)

## Per-route results

| Route | Desktop light | Desktop dark | Mobile light | Mobile dark | Eyeball notes |
| --- | --- | --- | --- | --- | --- |
| `/` | ✅ | ✅ | ✅ | ✅ | Logo sharp (2× crop), orange accent, CardGrid stagger, LinkCards |
| `/explanation/architecture/` | ✅ | ✅ | ✅ | ✅ | FileTree renders in dark mode |
| `/explanation/caching-model/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/explanation/tanstack-parity/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/devtools/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/framework-bindings/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/hydration/` | ✅ | ✅ | ✅ | ✅ | Caution aside renders |
| `/guides/infinite-queries/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/install/` | ✅ | ✅ | ✅ | ✅ | Tabs (recommended first), FileTree icons, dark tables fit |
| `/guides/keys-and-filters/` | ✅ | ✅ | ✅ | ✅ | Tables fit, inline code renders |
| `/guides/mutations/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/offline-and-network-modes/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/persistence/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/queries/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/guides/streamed-queries/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/client/` | ✅ | ✅ | ✅ | ✅ | Longest page: 6 tables fit, active sidebar, TOC tracks |
| `/reference/errors-and-cancellation/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/hooks/` | ✅ | ✅ | ✅ | ✅ | Mobile (390px, dark): no clipping, tap targets usable |
| `/reference/infinite-queries/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/keys-and-filters/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/mutations/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/observers/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/persistence-and-hydration/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/query-options/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/query/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/runtime-and-network/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/reference/utilities/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes |
| `/tutorials/first-query/` | ✅ | ✅ | ✅ | ✅ | Steps headings, Note/Tip asides, code highlighting, TOC |
| `/tutorials/quoin-app/` | ✅ | ✅ | ✅ | ✅ | Sampled via QA probes; same template as first-query |

## Eyeball pass checklist (verified visually)

- [x] **Logo sharp at 2×** in both themes (full-res header crops reviewed) — raster source at ≥5× display size; dark theme uses the light-plate variant
- [x] **Accent is brand orange/amber** (links, active sidebar item, asides, search highlight) — not Starlight default blue
- [x] **Code blocks** syntax-highlighted with multiple token colors, copy button, horizontal scroll (no page overflow)
- [x] **Tables** fit or scroll; no clipped columns
- [x] **Asides / CardGrid / Tabs / Steps / FileTree** render with icons, spacing, borders
- [x] **Sidebar** correct active state; nested groups (Core API, Runtime & Ecosystem) expand
- [x] **On-page TOC** present on long pages, tracks current heading
- [x] **Pagination** prev/next points at the correct neighbors
- [x] **Mobile** hamburger, tap targets, no clipped content (automated overflow probe: 0 failures)
- [x] **Dark mode** no pure-white panels, code background distinct from page
- [x] **Typography** no overlaps, no orphaned headings
- [x] **Search** modal opens via header button; results with titles + excerpts

## Defects found and fixed during QA

| Defect | Fix | Re-verified |
| --- | --- | --- | --- |
| `docsSchema({ extend })` crashed Astro 7.4 JSON-schema generation (`Cannot read properties of undefined (reading 'extend')`) | Standalone zod object passed to `extend` (deep-merged) | `pnpm check` + build clean |
| Bare `<T>` in prose broke MDX compile | Generics-free label in FileTree | Build clean |
| Missing MDX component imports on all pages | Import lines inserted per usage | Build clean |
| `Aside type="warning"` invalid | `caution` | Build clean |
| i18n collection warning | `i18nLoader()` + `en.json` | Build clean |
| sitemap `site` warning + Starlight 404 route conflict | `site`/`base` per ADR 0006; `disable404Route` with docs-content 404 | Build zero-warning |
| links-validator: 63 invalid links under `base` | Content links include the base path (Starlight convention) | All links valid |
| markdownlint MD060/MD046 misreads aligned tables and FileTree children | Style rules disabled in `.markdownlint-cli2.jsonc` with comments | Lint 0 issues |
| `astro check` hang: TypeScript 6's `@emnapi/runtime` 2.0.0-beta peer crashed the check binary (ADR 0007) | pnpm override aligns `@emnapi/runtime` to 1.11.1 | `astro check` 0/0/0 |
| QA script home-page slug: `path.relative` returns `""` not `".` | Slug logic handles both | Clean screenshots |

**Verdict: PASS.** The final repository state passed all gates as the last action before packaging.
