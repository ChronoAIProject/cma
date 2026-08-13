# Web Style

**This document is the single source of truth for all chrono-bot frontend design and style.**

No colour, spacing value, type scale step, component pattern, or interaction rule may be introduced in `web/` unless it appears here. If a design need is not covered, extend this document first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

---

## 1. What the frontend is

chrono-bot's frontend is **one page, and that page is a terminal**.

The terminal is not a component sitting inside a dashboard. It is the entire interface. There is no navigation menu, no second route, no separate dashboard, no docs link. A user arrives, and what they see is a terminal.

The surface has exactly two states:

| State | What it is |
|---|---|
| **Unauthenticated** | A scripted introduction types itself out, then hands off to a real prompt that accepts genuine keystrokes. The visitor's first Enter captures what they wrote and routes them to sign-in; their text survives the round trip and becomes the first turn of their first session. |
| **Authenticated** | The same surface, driven live. Session tabs above it, profile menu top-right. |

The only persistent chrome anywhere is the **session tab strip** and the **profile menu**. Nothing else.

Design consequences, binding:

- **The terminal owns the viewport.** It is full-bleed. The page itself never scrolls; the terminal owns its own scrollback.
- **One surface, many drivers.** The unauthenticated introduction and a live agent session are the same engine driven by different sources. A visual difference between them that is not a difference in *content* is a defect.
- **Fidelity beats decoration inside the viewport.** Agent-emitted output is rendered faithfully. Styling applies to the frame, the chrome, and the authored introduction — never to output produced by a kernel.
- **Never build an interactive feature here that belongs in the API.** If a client would want it, it belongs in `api/`. Carried forward unchanged.

---

## 2. Colour

Defined as CSS custom properties on `:root`. Components reference tokens only — never a literal colour.

Dark is the primary expression of this product. Light is a supported, fully specified counterpart, not an afterthought — but the two do **not** share accent values.

### Light (default)

| Token | Value | Use |
|---|---|---|
| `--ground` | `#F4F6F9` | page background |
| `--surface` | `#FFFFFF` | panels, terminal frame |
| `--surface-2` | `#E9EDF3` | tab strip, insets |
| `--ink` | `#0A0E14` | primary text |
| `--ink-2` | `#3D4757` | secondary text |
| `--ink-3` | `#5B6779` | labels, metadata |
| `--rule` | `#D5DBE4` | hairline borders |
| `--rule-2` | `#B9C2CE` | emphasised borders |
| `--accent` | `#006D8F` | interactive, focus, prompt |
| `--emphasis` | `#B3006B` | attention, pending approval |

### Dark

Redefined under both `@media (prefers-color-scheme: dark)` guarded as `:root:not([data-theme='light'])` and `:root[data-theme='dark']`, so an explicit choice wins in either direction.

| Token | Value |
|---|---|
| `--ground` | `#05070B` |
| `--surface` | `#0B0F17` |
| `--surface-2` | `#131A26` |
| `--ink` | `#E8EEF5` |
| `--ink-2` | `#93A4B8` |
| `--ink-3` | `#7C8A9E` |
| `--rule` | `#1C2534` |
| `--rule-2` | `#2A3546` |
| `--accent` | `#00E5FF` |
| `--emphasis` | `#FF2E97` |

`#00E5FF` is the **dark-theme accent only**. It measures 1.83:1 on `#FFFFFF` and must never appear in the light theme. Light uses `#006D8F`.

### Semantic

Session and system state. Separate from the accent; never used decoratively.

| Token | Light | Dark | Meaning |
|---|---|---|---|
| `--ok` | `#00674A` | `#00FFA3` | ready, running, healthy |
| `--warn` | `#7A5200` | `#FFB800` | idle, recovering, degraded |
| `--danger` | `#C0182B` | `#FF3B4E` | terminated in error, invalid |
| `--neutral` | `#5B6779` | `#7C8A9E` | unknown, retired |

### Measured contrast

Every text token against every background it may sit on, computed per WCAG 2.1. Body text requires 4.5:1. **No pair falls below it in either theme.** Worst case in each theme is recorded here so a future change that regresses it is visible.

| Theme | Worst text pair | Ratio |
|---|---|---|
| Dark | `--ink-3` on `--surface-2` | 4.97:1 |
| Dark | `--emphasis` on `--surface-2` | 5.06:1 |
| Light | `--ink-3` on `--surface-2` | 4.88:1 |
| Light | `--accent` on `--surface-2` | 4.99:1 |

`--ink-3` is the binding constraint in both themes. It carries 11px uppercase metadata, which is **not** large text, so the full 4.5:1 applies. Do not darken it in dark, or lighten it in light.

### Rules

- Every colour is defined on bare `:root` first. A colour whose only definition lives inside a media query or `[data-theme]` block is a bug.
- `body` always sets an explicit `background` from a token.
- Semantic colour never carries meaning alone — always pair with a label or icon.
- A new colour requires a changelog entry and a recomputed contrast row. There are no ad-hoc colours.

---

## 3. Glow and depth

Cyberpunk depth, bounded. Glow is a **state signal**, not decoration.

### Tokens

| Token | Light | Dark |
|---|---|---|
| `--glow-rest` | `0 0 10px rgb(0 109 143 / 0.16)` | `0 0 12px rgb(0 229 255 / 0.20)` |
| `--glow-active` | `0 0 16px rgb(0 109 143 / 0.28)` | `0 0 20px rgb(0 229 255 / 0.35)` |
| `--glow-emphasis` | `0 0 16px rgb(179 0 107 / 0.28)` | `0 0 20px rgb(255 46 151 / 0.35)` |

**These three are the only glow values permitted.** Arbitrary `box-shadow` or `text-shadow` values are forbidden.

### Rules

- Glow is expressed only through `box-shadow` and `text-shadow`, only in the accent or emphasis hue.
- Glow means something is **live**: a running session, a focused input, a pending approval. A static heading never glows.
- Glow is never the sole signal for state — it always accompanies a label, dot, or chip.
- **No glow on text below 15px.** It destroys legibility at the sizes this UI mostly uses. In practice this means glow applies to containers and the intro headline, essentially never to body copy.
- Glow is never applied to the terminal viewport or to any character rendered by a kernel.

### Overlays

One optional ambient overlay is permitted: a static grid or scanline texture.

- Maximum opacity `0.04`.
- Sits behind all content, `pointer-events: none`.
- **Never over the terminal viewport** — it corrupts glyph rendering.
- Suppressed entirely under `prefers-reduced-motion: reduce` if it animates at all. A static overlay may remain.

---

## 4. Typography

This is a terminal product. The monospace stack is **primary**.

| Role | Stack |
|---|---|
| Primary — terminal, prompt, data, chrome | `ui-monospace, 'SF Mono', SFMono-Regular, Menlo, Consolas, monospace` |
| Secondary — profile menu, dialogs, prose | `-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif` |

No webfonts. Nothing is loaded from a CDN.

### Scale

| Step | Size | Line height | Use |
|---|---|---|---|
| `xs` | 11px | 1.4 | uppercase labels, chips, tab metadata |
| `sm` | 13px | 1.45 | secondary text, metadata |
| `base` | 14px | 1.5 | body, chrome, menu items |
| `lg` | 18px | 1.35 | section headings |
| `xl` | 24px | 1.25 | intro headline |

### Terminal type

| Property | Value |
|---|---|
| Family | primary monospace stack |
| Size | 14px default, user-adjustable 12–18px |
| Line height | 1.35 |
| Letter spacing | 0 — never track a terminal |

Uppercase labels take `letter-spacing: 0.12em`. Headings take `text-wrap: balance`. Any column of digits takes `font-variant-numeric: tabular-nums`.

---

## 5. Spacing and layout

4px base unit. Permitted values only: `4, 8, 12, 16, 24, 32, 48, 64`.

- Lay out sibling groups with flex or grid and `gap`. Never per-element margins that collapse or double.
- **The page does not scroll.** The app fills exactly the viewport; the terminal owns its own scrollback. `body` never scrolls horizontally or vertically.
- **The terminal is full-bleed.** It has no page gutter. Its frame inset is 12px, reduced to 8px below 640px.
- Chrome — tab strip and profile menu — sits in a header bar 40px tall.
- Wide content inside chrome sits in its own `overflow-x: auto` container.

### Radius

| Value | Use |
|---|---|
| `0` | default — the terminal frame, tabs, the header bar |
| `2px` | chips, buttons, menu surfaces |

Nothing more rounded. Sharp is the aesthetic; rounding is the exception, not the default.

---

## 6. Components

### Terminal surface

The full-bleed host. One engine; the content source is swappable.

- Rendered by xterm.js.
- Container takes `--surface` background and a 1px `--rule` border.
- **Never place interactive UI over the terminal viewport.** Controls sit in the header bar above it.
- Scrollback is capped. Unbounded output must not be able to exhaust the tab.
- Selection background `--accent` at `0.25` alpha. Cursor `--accent`.

#### ANSI palette

The 16 terminal colours, derived from the tokens. This mapping is authoritative — do not invent terminal colours.

| Slot | Light | Dark |
|---|---|---|
| `black` | `#E9EDF3` | `#0B0F17` |
| `red` | `#C0182B` | `#FF3B4E` |
| `green` | `#00674A` | `#00FFA3` |
| `yellow` | `#7A5200` | `#FFB800` |
| `blue` | `#00557A` | `#00A8FF` |
| `magenta` | `#B3006B` | `#FF2E97` |
| `cyan` | `#006D8F` | `#00E5FF` |
| `white` | `#3D4757` | `#E8EEF5` |
| `brightBlack` | `#5B6779` | `#7C8A9E` |
| `brightRed` | `#8E0F1F` | `#FF6B78` |
| `brightGreen` | `#004D37` | `#6BFFC4` |
| `brightYellow` | `#5C3E00` | `#FFD166` |
| `brightBlue` | `#003F5C` | `#5CC6FF` |
| `brightMagenta` | `#8A0052` | `#FF6BB8` |
| `brightCyan` | `#00536D` | `#6BF0FF` |
| `brightWhite` | `#0A0E14` | `#FFFFFF` |

Agent-emitted output is rendered through this palette and is **not** otherwise restyled. Escape sequences never control anything beyond glyph presentation — see §8.

### Intro sequence

The unauthenticated state. Authored content, typed out.

| Property | Value |
|---|---|
| Machine/boot lines | 6ms per character |
| Prose | 18ms per character |
| Pause after `.` `?` `!` | 260ms |
| Pause after `,` | 90ms |
| Pause at line break | 140ms |
| Cursor blink interval | 530ms |
| **Time to interactive** | **target ≤ 3.2s, hard cap 4s** |

- The animation is **skippable**. Any keypress or click completes it immediately. A dim `--ink-3` hint appears after 600ms telling the visitor so.
- When typing completes, the prompt becomes live and accepts real keystrokes. The handoff must be visually seamless — the prompt the visitor types into is the same prompt the introduction was written to.
- The sign-in affordance appears **only once the visitor has typed at least one character**. Do not nag an empty prompt.
- `prefers-reduced-motion: reduce` — **all text is present instantly**, no typing, no blink, no skip hint. Fully usable, not a degraded fallback.

### Session tab strip

- One tab per session, in the header bar, left of the profile menu.
- Each tab carries a status dot from the semantic tokens **and** a text label. The dot is never the only signal.
- `+` affordance spawns a session.
- Past 6 tabs the strip scrolls horizontally within its own container; it never wraps and never pushes the profile menu off-screen.
- The active tab is marked by a 2px `--accent` bottom border and `--glow-rest`.
- A running session's dot takes `--glow-active`. An `idle` session is styled with `--warn` and **never** as an error — `idle` means the runtime was reclaimed and the session is revivable.

### Profile menu

- Avatar trigger sits top-right, minimum 32×32px.
- Opens a dropdown on `--surface` with a 1px `--rule` border and radius `2px`.
- Contains sign-out and account actions. It is not a navigation menu and must not grow into one.
- Fully keyboard operable: Enter/Space opens, arrows move, Escape closes and returns focus to the trigger, Tab is trapped while open.
- Dismissed by Escape, outside click, or focus leaving.

### Status chip

Monospace, `xs`, uppercase, 3px/8px padding, 1px border, radius `2px`. Text and border from the semantic token; background is the token at `0.12` alpha. Always carries the state word — colour is reinforcement, never the sole signal.

### Buttons

Single accent button per view. Everything else is a quiet button: transparent background, `--rule` border, `--ink-2` text. Destructive actions use `--danger` and always confirm.

---

## 7. Interaction

- Every interactive element has a visible focus state: `2px solid var(--accent)`, `outline-offset: 2px`. Never remove outlines.

  **One documented variant: the terminal prompt row.** A full-width, borderless input inside a terminal is not a control with edges, and a 2px ring drawn around it boxes in the whole prompt line and breaks the illusion the surface depends on. For that input only, the indicator is a 2px `--accent` bar on the row's leading edge (`box-shadow: inset 2px 0 0 var(--accent)`), reinforced by the `--accent` caret.

  This is a **relocation, not a removal** — same width, same token, still plainly visible, and still present under keyboard focus. It applies to the terminal prompt input and to nothing else; every other control takes the standard outline. Any future variant needs its own entry here.

- All transitions ≤ 150ms, and every one is wrapped in `@media (prefers-reduced-motion: no-preference)`. **The intro typewriter is bound by this rule** — see §6.
- Loading is a skeleton or an inline indicator; never a full-page spinner.
- Errors state what went wrong and what to do about it. No apologies, no raw error codes without a human sentence.
- Minimum target size 32×32px.

---

## 8. Terminal safety

The byte stream written into the terminal originates from a coding agent executing the user's own untrusted code. Escape sequences are an attack surface, and the unauthenticated introduction shares the same engine as a live session — so the engine, not the content source, carries the hardening.

- **OSC 52 (clipboard write) is disabled.** Terminal output must never write the host clipboard.
- Escape sequences must not set the browser tab title, and must not trigger any host-level action.
- Scrollback is capped.
- OSC 8 hyperlinks are validated before being made clickable.
- Hardening lives in the engine so that a future content source cannot forget to apply it.

---

## 9. Accessibility

- Text contrast meets WCAG AA (4.5:1 body, 3:1 for text ≥ 18px) in **both** themes. Measured ratios are recorded in §2.
- Semantic HTML before ARIA. Landmarks on every page.
- Every view is fully keyboard operable, including the terminal, which must be escapable by keyboard alone.
- Never convey state through colour alone.
- **The animated introduction is never the only route to its information.** The terminal viewport is exposed to assistive technology as a static, complete, already-finished transcript — a screen reader user is never made to wait for an animation, and never receives a character-by-character stream of updates.
- Focus on load rests on the prompt, so a keyboard user can type immediately without hunting.

---

## 10. Selectable terminal themes

The 16-slot palette in §6 is the product's own and remains the default in both
appearances. A terminal is somewhere people spend hours, and legibility at that
duration is personal, so the palette inside the terminal viewport — and only
inside it — may be changed by the person using it.

The scope is deliberately narrow. A selected theme repaints the terminal
canvas. It never changes the surrounding interface, which stays on the tokens
in §2, because a page whose chrome shifts with a terminal preference stops
being one product.

These are transcribed from their published definitions and vendored as data.
They are not a dependency: a theme is twenty hex values against a fixed
interface, and taking a package for that would add a supply-chain risk with
nothing to show for it.

Every theme names the appearance it belongs to. A dark theme selected in light
appearance would put a near-black rectangle in the middle of a light page, so
the selector offers only themes matching the current appearance.

| Theme | Appearance | Background | Foreground | Origin |
|---|---|---|---|---|
| ChronoBot | dark | `#0B0F17` | `#E8EEF5` | §6, the default |
| ChronoBot | light | `#FFFFFF` | `#0A0E14` | §6, the default |
| Dracula | dark | `#282A36` | `#F8F8F2` | Dracula, MIT |
| Nord | dark | `#2E3440` | `#D8DEE9` | Nord, MIT |
| Solarized Dark | dark | `#002B36` | `#839496` | Solarized, MIT |
| Solarized Light | light | `#FDF6E3` | `#657B83` | Solarized, MIT |
| GitHub Light | light | `#FFFFFF` | `#24292E` | GitHub, MIT |

Contrast inside the terminal is the theme author's, not ours, and these are all
long-established palettes designed for extended reading. The product's own
default is the one measured in §2 and remains what an unconfigured session
gets.

---

## 11. Waiting for a session

Provisioning a sandbox takes seconds on a warm node and a minute on a cold one,
and nothing can say in advance which it will be. A static line held for that
long reads as a page that has stopped working.

The wait is therefore animated: the characters of the message lift in sequence,
left to right, on a 1.4s loop with each character starting 45ms after the one
before it. The displacement is `0.28em` — a sense of life, not a bouncing ball,
because this sits beside someone for a minute.

It is a wave rather than a spinner because §5 forbids a full-page spinner, and
because a wave makes no claim about progress. A progress bar would imply the
system knows how far along it is, which it does not.

**The motion never carries information.** The sentence is exposed once to
assistive technology as ordinary text, and under `prefers-reduced-motion` the
animation is removed entirely while the message stays. Nothing is lost by not
seeing it move.

**A session that ends badly says why.** Terminating normally and failing to
start are different events and must not read the same: the control plane
records a reason on the failure, and the surface shows it rather than the bare
statement that the session has ended.

---

## Changelog

Append-only. Never edit or delete an entry.

| Date | Change |
|---|---|
| 2026-08-12 | Initial version. Established colour tokens for light and dark, semantic state colours, type stacks and scale, 4px spacing system, component rules for status chip, data table, terminal and buttons, interaction and accessibility baselines. |
| 2026-08-13 | Terminal-first cyberpunk redesign. Reframed the frontend as a single full-bleed terminal page with no navigation, in two states (scripted introduction and live session). Replaced the colour system with a cyan/magenta-on-near-black palette, recording measured WCAG ratios and raising `--ink-3` to `#7C8A9E` in dark to clear AA for 11px metadata. Added the `--emphasis` token, a bounded three-step glow system with an ambient overlay cap, and an authoritative 16-slot ANSI palette. Promoted the monospace stack to primary and retuned the type scale. Made the page non-scrolling and the terminal full-bleed, and set radii to 0/2px. Specified the intro sequence timing, skip behaviour and reduced-motion equivalence, the session tab strip, and the profile menu. Added §8 Terminal safety covering OSC 52, tab-title suppression, scrollback caps and OSC 8 validation. Removed the data table component and the "information density over whitespace" premise, both of which described a dashboard this frontend no longer is. |
| 2026-08-13 | Documented one focus-state variant in §7: the terminal prompt row carries its 2px accent indicator on the row's leading edge rather than as a ring around the input, because a ring around a full-width borderless field boxes in the whole prompt line and breaks the terminal illusion. A relocation, not a removal, scoped to that one input. |
| 2026-08-13 | Added §10 Selectable terminal themes: six vendored palettes (Dracula, Nord, Solarized Dark, Solarized Light, GitHub Light) alongside the product's own, selectable per person and scoped to the terminal canvas only. Vendored as data rather than taken as a dependency. Themes are filtered by the current appearance so a dark palette cannot be placed on a light page. Also specified the appearance control in the profile menu, cycling system → light → dark, where "system" is the absence of a choice rather than a third palette. |
| 2026-08-14 | Added §11 Waiting for a session: the provisioning message animates as a left-to-right character wave (1.4s loop, 45ms stagger, 0.28em lift), chosen over a spinner because §5 forbids one and because a wave claims no knowledge of progress. Motion carries no information — the sentence is exposed once to assistive technology and the animation is removed under reduced-motion. Also specified that a session which fails to start must show the recorded reason rather than the same wording as one that ended normally. |
