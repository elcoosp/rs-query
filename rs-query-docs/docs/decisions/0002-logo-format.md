# ADR 0002 — Logo format: optimized raster variants, no tracing

- **Status:** Accepted
- **Date:** 2026-11-08
- **Scope:** `src/assets/logo-light.png`, `src/assets/logo-dark.png`, `public/favicon.png`

## Context

The rs-query repository ships exactly one brand asset: `docs/logo.png` — a 1264×1264 RGBA raster (≈1.0 MB) of a 3D-rendered mark in three colors (slate `#3d4249`, blue `#3d62a5`, orange `#cd8a48` sampled from the asset). No SVG original exists anywhere in the repository (`docs/`, `src/`, root). The skill in force prefers SVG and forbids upscaling rasters to fake larger logos.

## Options considered

1. **Trace the raster to SVG.** Rejected. The mark is a soft-shaded 3D render with gradients and contact shadows; any automatic or manual trace produces a flat approximation that visibly changes the brand mark — a worse failure than honest raster at adequate resolution.
2. **Ship the original PNG.** Rejected: 1 MB in the header is poor practice, and the raw canvas has large transparent margins.
3. **Generate optimized, adequately-sized raster variants** (chosen). The mark is displayed at ~2.5 rem (40 px) height in the Starlight header; 2×/3× render targets are 80–120 px. We ship a 620 px-wide light variant and a 640 px dark variant — ≥5× the largest render target, so no display size in the header can pixelate.

## Decision

Generate with `scripts/build_brand_assets.py` (kept alongside the docs project in the working repository):

- `logo-light.png` — the mark trimmed to its alpha bounding box (+3 % padding), Lanczos-downscaled to 620 px width, on transparency. Used for the light theme.
- `logo-dark.png` — the same mark composited onto a light rounded plate (`#f4f5f7`, 16 % corner radius, hairline outline). The slate ribbons of the mark are near-`#3d4249` and would be invisible on Starlight's dark background (`#1b1c22`); the plate preserves the artwork unchanged while keeping it legible. Used for the dark theme.
- `favicon.png` — 512 px square plate + fitted mark.

All variants are deterministic derivatives of the original asset — the mark itself is never redrawn, recolored, or upscaled.

## Consequences

- SVGO (`removeViewBox: false` etc.) does not apply — there is no SVG. If an SVG original ever lands upstream, regenerate and re-run this decision.
- The Astro `experimental.svgOptimizer` flag is **not** enabled: it is an experimental Astro feature that emits a build-time warning banner, which the zero-warning release gate forbids. The committed asset is already optimized, so the safety net is not needed.
- Dark-mode legibility comes from the plate, not from recoloring; confirm sharpness and legibility in both themes during every visual QA pass (2× zoom on the header).
- Logo-quality risk recorded per the pipeline: source is raster-only; the chosen mitigation is resolution headroom, not vectorization.
