# ADR 0003 — Syntax highlighting: Shiki via Expressive Code (default), no Rangi

- **Status:** Accepted
- **Date:** 2026-11-08
- **Scope:** Code block rendering across the docs site

## Context

Starlight renders code blocks through Expressive Code, whose supported highlighter is **Shiki** — TextMate grammars (the same grammar set VS Code uses), 180+ languages, dual light/dark themes with CSS variables, build-time execution (zero client-side JavaScript), and first-class transformers (line highlighting `{1,3-5}`, word wrap, copy button, `ansi` output blocks).

Rangi is a legitimate lightweight highlighter (~13 kB, ~46 languages, regex tokenizer), but its own documentation positions it for chat UIs, small SSG sites, and embedded apps — not documentation sites. It is not integrated with Expressive Code: adopting it would mean writing and maintaining a custom highlighter plugin with no upstream support, while losing transformers, Twoslash, and the language set documentation needs.

## Decision

Ship Shiki — Starlight's built-in default via Expressive Code — and do not add Rangi or any other custom highlighter. No `expressiveCode` override is configured: the default dual-theme setup is already correct for this site, and the configuration guidance is to add overrides only for a concrete need (custom theme, build performance, guaranteed language), none of which applies here.

## Consequences

- Authors must declare a language on every fenced block (the markdownlint MD040 rule stays enabled to enforce it).
- Terminal output uses the `ansi` tag; line highlighting uses Expressive Code's `{1,3-5}` syntax; the built-in copy button is kept — no client-side highlighter is added.
- Verified in the visual QA pass: code blocks show multiple token colors (not monochrome), a working copy button, and horizontal scroll instead of page overflow.
