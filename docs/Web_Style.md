# Web Style

**This document is the single source of truth for all chrono-bot frontend design and style.**

No colour, spacing value, type scale step, component pattern, or interaction rule may be introduced in `web/` unless it appears here. If a design need is not covered, extend this document first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

---

## 1. What the frontend is

chrono-bot's frontend is a **read-only operational dashboard plus the terminal surface**. It is not the primary interaction path — NyxID Chat and FKST Chat are. Design accordingly:

- Information density over whitespace. This is a console, not a landing page.
- State must be readable at a glance, encoded in form as well as in text.
- The terminal is the one place where fidelity beats styling. It is a faithful terminal, not a themed approximation of one.
- Never build an interactive feature here that belongs in the API. If a client would want it, it belongs in `api/`.

---

## 2. Colour

Defined as CSS custom properties on `:root`. Components reference tokens only — never a literal colour.

### Light (default)

| Token | Value | Use |
|---|---|---|
| `--ground` | `#F6F7F9` | page background |
| `--surface` | `#FFFFFF` | cards, panels |
| `--surface-2` | `#EFF1F5` | table headers, insets |
| `--ink` | `#12161C` | primary text |
| `--ink-2` | `#4E5866` | secondary text |
| `--ink-3` | `#7A8494` | labels, metadata |
| `--rule` | `#DDE1E8` | hairline borders |
| `--rule-2` | `#C9CFD9` | emphasised borders |
| `--accent` | `#2E4A78` | interactive, focus, links |
| `--accent-soft` | `#E7ECF5` | accent backgrounds |

### Dark

Redefined under both `@media (prefers-color-scheme: dark)` guarded as `:root:not([data-theme='light'])` and `:root[data-theme='dark']`, so an explicit choice wins in either direction.

| Token | Value |
|---|---|
| `--ground` | `#0D1015` |
| `--surface` | `#14181F` |
| `--surface-2` | `#1B212A` |
| `--ink` | `#E7EAEF` |
| `--ink-2` | `#A2ACBB` |
| `--ink-3` | `#7C8695` |
| `--rule` | `#242B35` |
| `--rule-2` | `#333C48` |
| `--accent` | `#8AACE4` |
| `--accent-soft` | `#1A2434` |

### Semantic

Session and system state. Separate from the accent; never used decoratively.

| Token | Light | Dark | Meaning |
|---|---|---|---|
| `--ok` | `#1E6A48` | `#6FC79B` | ready, running, healthy |
| `--warn` | `#8C5A0E` | `#D9A45C` | idle, recovering, degraded |
| `--danger` | `#97292A` | `#E08585` | terminated in error, invalid |
| `--neutral` | `#7A8494` | `#7C8695` | unknown, retired |

Rules:
- Every colour is defined on bare `:root` first. A colour whose only definition lives inside a media query or `[data-theme]` block is a bug.
- `body` always sets an explicit `background` from a token.
- Semantic colour never carries meaning alone — always pair with a label or icon.

---

## 3. Typography

| Role | Stack |
|---|---|
| UI | `-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif` |
| Data / code / terminal | `ui-monospace, 'SF Mono', SFMono-Regular, Menlo, Consolas, monospace` |

No webfonts. Nothing is loaded from a CDN.

### Scale

| Step | Size | Line height | Use |
|---|---|---|---|
| `xs` | 11px | 1.4 | uppercase labels, chips |
| `sm` | 13px | 1.45 | table cells, metadata |
| `base` | 15px | 1.55 | body |
| `lg` | 18px | 1.35 | section headings |
| `xl` | 24px | 1.25 | page title |

Uppercase labels take `letter-spacing: 0.12em`. Headings take `text-wrap: balance`. Any column of digits takes `font-variant-numeric: tabular-nums`.

---

## 4. Spacing and layout

4px base unit. Permitted values only: `4, 8, 12, 16, 24, 32, 48, 64`.

- Lay out sibling groups with flex or grid and `gap`. Never per-element margins that collapse or double.
- Page gutter: 24px; 16px below 640px.
- Wide content — tables, code, terminal — sits in its own `overflow-x: auto` container. The page body never scrolls horizontally.
- Border radius: `2px` default, `3px` for cards. Nothing more rounded.

---

## 5. Components

### Status chip
Monospace, `xs`, uppercase, 3px/8px padding, 1px border. Background from the semantic token's soft variant, text and border from the token. Always carries the state word — colour is reinforcement, never the sole signal.

### Data table
`surface` background, `surface-2` header. Header is monospace `xs` uppercase in `--ink-3`. Rows separated by 1px `--rule`; no zebra striping. Numeric columns right-aligned with tabular numerals.

### Terminal
- Rendered by xterm.js. **Its own colour handling is authoritative** — do not restyle terminal output, and do not override the ANSI palette.
- Container gets `--surface` background and a 1px `--rule` border.
- Font is the monospace stack at 13px, `line-height: 1.2`.
- Never place interactive UI over the terminal viewport; controls sit in a header bar above it.

### Buttons
Single accent button per view. Everything else is a quiet button: transparent background, `--rule` border, `--ink-2` text. Destructive actions use `--danger` and always confirm.

---

## 6. Interaction

- Every interactive element has a visible focus state: `2px solid var(--accent)`, `outline-offset: 2px`. Never remove outlines.
- All transitions ≤ 150ms, and every one is wrapped in `@media (prefers-reduced-motion: no-preference)`.
- Loading is a skeleton or an inline indicator; never a full-page spinner.
- Errors state what went wrong and what to do about it. No apologies, no raw error codes without a human sentence.
- Minimum target size 32×32px.

---

## 7. Accessibility

- Text contrast meets WCAG AA (4.5:1 body, 3:1 for text ≥ 18px) in **both** themes.
- Semantic HTML before ARIA. Landmarks on every page.
- Every view is fully keyboard operable, including the terminal, which must be escapable by keyboard alone.
- Never convey state through colour alone.

---

## Changelog

Append-only. Never edit or delete an entry.

| Date | Change |
|---|---|
| 2026-08-12 | Initial version. Established colour tokens for light and dark, semantic state colours, type stacks and scale, 4px spacing system, component rules for status chip, data table, terminal and buttons, interaction and accessibility baselines. |
