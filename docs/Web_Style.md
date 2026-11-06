# Web Style

**This document is the single source of truth for all CMA (ChronoAI Managed Agent) frontend design and style.**

No colour, spacing value, type scale step, radius, component pattern, or interaction rule may be
introduced in `web/` unless it appears here. If a design need is not covered, extend this document
first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

---

## 1. What the frontend is

**One Agent conversation inside a visible Sandbox hierarchy.**

| View | What it is |
|---|---|
| Home | The owner's Sandbox -> Workspace -> Agent tree and creation state appropriate to their capabilities. |
| Sandbox | Runtime state, pinned profile facts, Workspaces and recorded preparation. |
| Agent | One conversation in one Worktree, with approvals, tools, usage and durable history. |
| Profiles | One library for personal Sandbox, Workspace and Agent Profiles, public Marketplace discovery, and versioned Event Worker Profiles. |
| CMA Trigger | Signed-in authoring, publication, placements and integration snippets. |
| Public Trigger | A visitor's landing, consent, review, connection assistance and progress. |
| Operations | Capability-gated inventory, usage, Profiles, activity and settings. |
| Account | Canonical Appearance choices and the owner-scoped Your usage dialog. |
| Bots | Owner Bot configuration and lifecycle through CMAEG, opened from Consumers in Event Gateway navigation. |

Each view has a canonical route. Refresh and browser history restore it without
admitting work. The terminal plane, separate Chat model, session list, canvas
arrangement, pricing and contribution-specific workflow are retired.

- The surface owns the remaining width. Details concede before the tree;
  narrow layouts keep the primary conversation or task usable.
- The first-party client uses public REST, AG-UI and the flagged Responses v2
  transport with rich Agent replay. It never uses private routes.
- Agent output and user-authored commands remain intact; styling applies to
  their reading frame.
- Progress comes from public records and events, not a local timer. Navigation
  and an OAuth callback never imply consent to start.

---

## 2. Tokens, in three tiers

Every colour enters through a token, and tokens are layered so that a component never names a
value and a theme never names a component.

| Tier         | Prefix             | What it is                                                                                                |
| ------------ | ------------------ | --------------------------------------------------------------------------------------------------------- |
| **static**   | `--dsw-static-*`   | The raw ramps. Identical in both themes — a ramp is a fact about the colour, not about the theme.         |
| **alias**    | `--dsw-alias-*`    | A role: a background layer, a label rank, a border weight, a state. **This is the tier components read.** |
| **specific** | `--dsw-specific-*` | One named surface — the sidebar fill, a message bubble, the composer's own ground.                        |

**A component referencing a static token is a defect.** The static tier exists so the alias tier has
something to point at; reaching past the alias is how a surface ends up unable to change theme.

The Harness ramps (`neutral`, `neutral-bluish`, `blue`, `deepseek`, and `green`) retire. Since
2026-10-21 both appearances are drawn from one neutral **`zinc`** ramp, the shadcn/ui neutral scale,
with Chrono **`lime`** as the brand and accent: lime in dark, its deep `lime 700` in light where the
bright step cannot carry text. The Inkwell `paper`/`ink` and Lumen `coal` ramps stay declared for
the remaining static consumers but no alias reads them. The raw ramps are declared once and are available to either theme, even where one theme is their
main consumer. A ramp and step expand directly to a static token name: for example, `paper 50` below
is `--dsw-static-paper-50`.

| Ramp    | Steps, from lightest or most vivid to deepest                                                                                                                      |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `paper` | `0 #FFFBF2`, `50 #FAF2E0`, `100 #F5EAD3`, `200 #EFE4CB`, `250 #EFE2C6`, `300 #E5D8BC`, `400 #CFBE99`, `500 #B9A57C`                                                |
| `ink`   | `50 #FDF8EC`, `200 #B7AB92`, `300 #8C8474`, `350 #756D5E`, `500 #4C5A68`, `800 #0E61AC`, `850 #0A4C88`, `900 #073A69`, `950 #15283C`                               |
| `coal`  | `50 #EDF2E2`, `300 #A9B199`, `450 #7D876F`, `500 #6B7361`, `650 #3F4630`, `700 #333A26`, `750 #21251A`, `800 #191C14`, `850 #121410`, `900 #0C0D0A`, `950 #060606` |
| `lime` | `300 #D7FF7D`, `350 #D7FF5A`, `400 #C3FD43`, `420 #C8F542`, `500 #AEE734`, `600 #8FC121`, `700 #4D7C0F`, `800 #3F6212`, `900 #24300F` |
| `zinc` | `0 #FFFFFF`, `50 #FAFAFA`, `75 #F7F7F8`, `100 #F4F4F5`, `150 #ECECEF`, `190 #E6E6E9`, `200 #E4E4E7`, `300 #D4D4D8`, `400 #A1A1AA`, `440 #8B8B94`, `450 #8A8A93`, `460 #85858E`, `500 #71717A`, `550 #62626B`, `600 #52525B`, `700 #3F3F46`, `720 #3A3A40`, `790 #2A2A2E`, `800 #27272A`, `810 #26262A`, `850 #1F1F23`, `870 #1C1C1F`, `880 #1A1A1D`, `890 #19191C`, `900 #18181B`, `915 #141416`, `920 #131315`, `925 #111113`, `930 #0F0F11`, `940 #0B0B0C`, `950 #09090B`, `960 #060607` |
| `moss`  | `100 #EDF6E7`, `400 #5F963B`, `500 #3E7A22`, `600 #2D6017`, `900 #1C2E14`                                                                                          |
| `red`   | `100 #F8E4DE`, `300 #FF806F`, `400 #FF5A45`, `500 #D7604A`, `600 #BC3B26`, `700 #A42D1B`, `900 #3A1711`                                                            |
| `amber` | `100 #F5E6C4`, `300 #FFC45A`, `400 #FFB020`, `500 #D18B12`, `600 #A9720B`, `700 #875A08`, `900 #33260C`                                                            |

`moss` replaces the old generic green ramp. Light success uses its paper-compatible green; dark
success uses `lime`, where success and the theme accent deliberately coincide.

### The complete alias contract

The following tables are exhaustive. Values labelled **derived** have no direct reference token; the
derivation is still fixed here so implementation never has to guess. Component sheets keep these
names while their values change, which lets the rebrand land surface by surface without inventing a
second vocabulary.

| Alias                                 | Light                   | Dark                    |
| ------------------------------------- | ----------------------- | ----------------------- |
| `--dsw-alias-bg-base` | `#FFFFFF` | `#0B0B0C` |
| `--dsw-alias-bg-layer-1` | `#FFFFFF` | `#141416` |
| `--dsw-alias-bg-layer-2` | `#F7F7F8` | `#18181B` |
| `--dsw-alias-bg-layer-3` | `#ECECEF` | `#1C1C1F` |
| `--dsw-alias-bg-mask-1`               | `rgba(9,9,11,.42)`      | `rgba(0,0,0,.72)`       |
| `--dsw-alias-bg-mask-2`               | `rgba(9,9,11,.24)`      | `rgba(0,0,0,.48)`       |
| `--dsw-alias-bg-mask-3`               | `rgba(9,9,11,.6)`       | `rgba(0,0,0,.84)`       |
| `--dsw-alias-bg-mask-photo`           | `rgba(9,9,11,.88)`      | `rgba(0,0,0,.92)`       |
| `--dsw-alias-bg-mask-drop`            | `rgba(255,255,255,.74)` | `rgba(17,17,19,.74)`    |
| `--dsw-alias-bg-module-platform` | `#FAFAFA` | `#18181B` |
| `--dsw-alias-bg-multi-select` | `#ECECEF` | `#1C1C1F` |
| `--dsw-alias-bg-overlay` | `#F4F4F5` | `#27272A` |
| `--dsw-alias-bg-skeleton`             | `rgba(9,9,11,.06)`      | `rgba(255,255,255,.08)` |
| `--dsw-alias-border-inverted`         | `rgba(9,9,11,.06)`      | `rgba(250,250,250,.08)` |
| `--dsw-alias-border-inverted2`        | `rgba(9,9,11,.08)`      | `rgba(250,250,250,.1)`  |
| `--dsw-alias-border-l1` | `#E6E6E9` | `#1F1F23` |
| `--dsw-alias-border-l2-darkmode-thin` | `#E6E6E9` | `#1F1F23` |
| `--dsw-alias-border-l2` | `#E6E6E9` | `#1F1F23` |
| `--dsw-alias-border-l3` | `#D4D4D8` | `#3F3F46` |
| `--dsw-alias-border-l4` | `#A1A1AA` | `#52525B` |

| Alias                                        | Light                 | Dark                    |
| -------------------------------------------- | --------------------- | ----------------------- |
| `--dsw-alias-brand-primary` | `#4D7C0F` | `#C8F542` |
| `--dsw-alias-brand-primary-invert` | `#FFFFFF` | `#0B0B0C` |
| `--dsw-alias-brand-text` | `#4D7C0F` | `#C8F542` |
| `--dsw-alias-button-contrast-fill` | `#18181B` | `#FAFAFA` |
| `--dsw-alias-button-elevated-fill` | `#FFFFFF` | `#1F1F23` |
| `--dsw-alias-button-floating-fill` | `#FFFFFF` | `#18181B` |
| `--dsw-alias-button-floating-hover` | `#F4F4F5` | `#1F1F23` |
| `--dsw-alias-button-ghost-active-border` | `#D4D4D8` | `#3F3F46` |
| `--dsw-alias-button-ghost-active-fill` | `#ECECEF` | `#1C1C1F` |
| `--dsw-alias-button-ghost-active-hover` | `#E4E4E7` | `#27272A` |
| `--dsw-alias-button-info-fill` | `#18181B` | `#C3FD43` |
| `--dsw-alias-button-info-hover` | `#27272A` | `#AEE734` |
| `--dsw-alias-button-primary-dimmed` | `#E4E4E7` | `#3F3F46` |
| `--dsw-alias-button-primary-foreground` | `#0B0B0C` | `#0B0B0C` |
| `--dsw-alias-button-primary-fill` | `#C8F542` | `#C8F542` |
| `--dsw-alias-button-primary-hover` | `#D7FF5A` | `#D7FF5A` |
| `--dsw-alias-button-primary-active` | `#AEE734` | `#AEE734` |
| `--dsw-alias-button-tool-bar-fill-invisible` | `rgba(9,9,11,.12)`    | `rgba(250,250,250,.12)` |
| `--dsw-alias-button-tool-bar-fill`           | `rgba(9,9,11,.64)`    | `rgba(250,250,250,.32)` |
| `--dsw-alias-button-tool-bar-hover`          | `rgba(9,9,11,.78)`    | `rgba(250,250,250,.46)` |
| `--dsw-alias-interactive-bg-active` | `#ECECEF` | `#1C1C1F` |
| `--dsw-alias-interactive-bg-hover`           | `rgba(9,9,11,.04)`    | `rgba(255,255,255,.04)` |
| `--dsw-alias-interactive-bg-hover-accent`    | `rgba(77,124,15,.08)`  | `rgba(200,245,66,.1)`   |
| `--dsw-alias-interactive-bg-hover-danger`    | `rgba(188,59,38,.08)` | `rgba(255,90,69,.1)`    |
| `--dsw-alias-interactive-bg-hover-solid` | `#ECECEF` | `#1C1C1F` |
| `--dsw-alias-image-outline`                  | `rgba(0,0,0,.1)`      | `rgba(255,255,255,.1)`  |

| Alias                                          | Light                | Dark                   |
| ---------------------------------------------- | -------------------- | ---------------------- |
| `--dsw-alias-label-caption` | `#52525B` | `#A1A1AA` |
| `--dsw-alias-label-dimmed` | `#A1A1AA` | `#52525B` |
| `--dsw-alias-label-primary` | `#111113` | `#F4F4F5` |
| `--dsw-alias-label-primary-bluish` | `#111113` | `#F4F4F5` |
| `--dsw-alias-label-primary-dimmed` | `#52525B` | `#A1A1AA` |
| `--dsw-alias-label-primary-foreground` | `#FAFAFA` | `#09090B` |
| `--dsw-alias-label-primary-inverted` | `#FAFAFA` | `#09090B` |
| `--dsw-alias-label-secondary` | `#52525B` | `#A1A1AA` |
| `--dsw-alias-label-tertiary` | `#62626B` | `#8B8B94` |
| `--dsw-alias-markdown-citation` | `#F4F4F5` | `#1F1F23` |
| `--dsw-alias-markdown-code-block` | `#FAFAFA` | `#09090B` |
| `--dsw-alias-markdown-code-block-banner` | `#F4F4F5` | `#18181B` |
| `--dsw-alias-markdown-code-segment-selected` | `#FFFFFF` | `#1F1F23` |
| `--dsw-alias-markdown-code-segment-unselected` | `#F4F4F5` | `#18181B` |
| `--dsw-alias-markdown-inline-code` | `#F4F4F5` | `#1F1F23` |
| `--dsw-alias-markdown-placeholder` | `#FAFAFA` | `#18181B` |
| `--dsw-alias-markdown-tag` | `#F4F4F5` | `#1F1F23` |
| `--dsw-alias-scrollbar-bg-l1` | `#D4D4D8` | `#3F3F46` |
| `--dsw-alias-scrollbar-bg-l2` | `#A1A1AA` | `#52525B` |
| `--dsw-alias-scrollbar-hover-l1` | `#71717A` | `#8FC121` |
| `--dsw-alias-scrollbar-hover-l2` | `#52525B` | `#AEE734` |
| `--dsw-alias-state-business-primary` | `#4D7C0F` | `#C8F542` |
| `--dsw-alias-state-business-tertiary`          | `rgba(63,98,18,.12)` | `rgba(195,253,67,.18)` |
| `--dsw-alias-state-error-primary` | `#BC3B26` | `#FF5A45` |
| `--dsw-alias-state-error-secondary` | `#D7604A` | `#FF806F` |
| `--dsw-alias-state-success-primary` | `#3E7A22` | `#C3FD43` |
| `--dsw-alias-state-success-secondary` | `#5F963B` | `#AEE734` |
| `--dsw-alias-state-success-tertiary` | `#EDF6E7` | `#24300F` |
| `--dsw-alias-state-warn-label` | `#875A08` | `#FFB020` |
| `--dsw-alias-state-warn-primary` | `#A9720B` | `#FFB020` |
| `--dsw-alias-state-warn-secondary` | `#D18B12` | `#FFC45A` |
| `--dsw-alias-state-warn-tertiary` | `#F5E6C4` | `#33260C` |
| `--dsw-alias-toast-bg` | `#18181B` | `#18181B` |
| `--dsw-alias-tooltip-bg` | `#18181B` | `#FAFAFA` |

| Specific token                                  | Light                | Dark                   |
| ----------------------------------------------- | -------------------- | ---------------------- |
| `--dsw-specific-bubble` | `#F4F4F5` | `#18181B` |
| `--dsw-specific-bubble-highlight`               | `rgba(63,98,18,.12)` | `rgba(195,253,67,.18)` |
| `--dsw-specific-input-major` | `#FFFFFF` | `#111113` |
| `--dsw-specific-login-input` | `#FFFFFF` | `#111113` |
| `--dsw-specific-menu` | `#FFFFFF` | `#18181B` |
| `--dsw-specific-selector` | `#ECECEF` | `#1C1C1F` |
| `--dsw-specific-sidebar-fill` | `#F7F7F8` | `#0F0F11` |
| `--dsw-specific-sidebar-new-current` | `#ECECEF` | `#1C1C1F` |
| `--dsw-specific-sidebar-nav-item-active` | `#ECECEF` | `#1C1C1F` |
| `--dsw-specific-sidebar-nav-item-active-accent` | `rgba(63,98,18,.12)` | `rgba(195,253,67,.18)` |
| `--dsw-specific-sidebar-nav-item-hover` | `#ECECEF` | `#1C1C1F` |
| `--dsw-specific-tip` | `#F4F4F5` | `#1F1F23` |

`--dsw-alias-brand-primary-new-colorprimary-new-color` is retired. Its malformed exported name had
zero consumers under `web/src`; retaining it would preserve no compatibility. Every other alias and
specific name above survives.

### What the palette means

Publisher badge previews must show both appearances regardless of the active
application theme. These four aliases therefore have the same values in both
themes; components still never read static tokens directly:

| Alias | Both themes | Existing source |
|---|---|---|
| `--dsw-alias-preview-light-bg` | `#FFFFFF` | `#FFFFFF` |
| `--dsw-alias-preview-light-label` | `#09090B` | `#09090B` |
| `--dsw-alias-preview-dark-bg` | `#111113` | `#111113` |
| `--dsw-alias-preview-dark-label` | `#FAFAFA` | `#FAFAFA` |

Message-avatar fallback aliases are also appearance-independent:
`--dsw-alias-avatar-bg` is `zinc 950` (`#09090B`), and
`--dsw-alias-avatar-label` is `zinc 50` (`#FAFAFA`). They retain a black circle
with readable initials in either theme.

The character body aliases (§28) are appearance-independent too:
`--dsw-alias-character-brand` is `lime 400`, `-running` `ink 800`, `-completed` `moss 500`,
`-pending` `amber 300` and `-failed` `red 600`.

- **Backgrounds.** Light is white with zinc 75 sidebar and zinc 150 selected layers;
  dark uses zinc 940 page, zinc 930 sidebar, zinc 915 card and zinc 870 selected. Separation comes from a 1px zinc border first and
  shadow second, as in shadcn/ui.
- **One primary per view.** Primary/info button fills are lime 420 in both themes,
  with zinc 940 text. Accent text/rings use lime 700 in light and lime 420 in dark.
- **Neutral interaction stays neutral.** Generic hover and active fills are zinc alpha in light and
  white alpha in dark. Only an explicitly accent interaction takes the lime wash.
- **Status remains semantic.** Warning is `#A9720B` light / `#FFB020` dark; error is `#BC3B26` /
  `#FF5A45`; success is moss `#3E7A22` in light and the same lime 420 as the accent in dark.
- **Glow is dark-only and rare.** Dark may use the `rgba(200,245,66,.35)` family on the primary
  hover, focus, live dot, progress fill and tab underline. Accent covers no more than 8% of the
  viewport. Light never glows; it uses a neutral `zinc 950` shadow at low alpha.
- **Tooltips and toasts invert.** The tip is `zinc 900` in light and `zinc 50` in dark, with the
  inverse label, so it reads as an overlay rather than another brand surface.
- **Selection and focus follow the accent.** `::selection` is accent ground with
  `label-primary-foreground`. The global `--focus-ring` remains the same whole-value double ring:
  `0 0 0 2px bg-base, 0 0 0 4px state-business-primary`.

### Measured text and control contrast (2026-11-06)

WCAG sRGB relative luminance, `(lighter + 0.05) / (darker + 0.05)`; ratios below
are rounded for display only. Primary/secondary/weak use label-primary/secondary/
tertiary. Accent text and icons use brand-text/brand-primary. Page, sidebar, card,
selected and menu use bg-base, sidebar-fill, bg-layer-1, bg-layer-3 and menu.

| Paint / ground | Light | Dark | Requirement |
| --- | --- | --- | --- |
| Primary on page | 18.86:1 | 17.90:1 | AA |
| Primary on sidebar | 17.62:1 | 17.42:1 | AA |
| Primary on card | 18.86:1 | 16.74:1 | AA |
| Primary on selected | 16.00:1 | 15.47:1 | AA |
| Primary on menu | 18.86:1 | 16.12:1 | AA |
| Secondary on page | 7.73:1 | 7.68:1 | AA |
| Secondary on sidebar | 7.22:1 | 7.47:1 | AA |
| Secondary on card | 7.73:1 | 7.18:1 | AA |
| Secondary on selected | 6.56:1 | 6.63:1 | AA |
| Secondary on menu | 7.73:1 | 6.91:1 | AA |
| Weak on page | 6.04:1 | 5.83:1 | AA |
| Weak on sidebar | 5.64:1 | 5.67:1 | AA |
| Weak on card | 6.04:1 | 5.45:1 | AA |
| Weak on selected | 5.12:1 | 5.03:1 | AA |
| Weak on menu | 6.04:1 | 5.25:1 | AA |
| Accent text on page | 4.99:1 | 15.55:1 | AA |
| Accent text on sidebar | 4.66:1 | 15.13:1 | AA |
| Accent text on card | 4.99:1 | 14.54:1 | AA |
| Accent icon on selected | 4.24:1 | 13.43:1 | UI ≥3:1; selected text uses primary |
| Accent text on menu | 4.99:1 | 14.00:1 | AA |
| Button text on fill | 15.55:1 | 15.55:1 | AA |
| Button text on hover | 17.18:1 | 17.18:1 | AA |
| Button text on active | 13.38:1 | 13.38:1 | AA |
| Error on task panel | 5.54:1 | 5.68:1 | AA |
| Error on task shell | 5.31:1 | 6.01:1 | AA |
| Usage dialog/sheet and Appearance notice primary | 17.62:1 | 16.12:1 | AA |
| Usage dialog/sheet secondary | 7.22:1 | 6.91:1 | AA |
| Usage dialog/sheet weak | 5.64:1 | 5.25:1 | AA |
| Usage icon/chart accent | 4.66:1 | 14.00:1 | UI ≥3:1 |
| Usage icon/chart secondary | 7.22:1 | 6.91:1 | UI ≥3:1 |
| Account selected Appearance / text size | 16.00:1 | 15.47:1 | AA; primary text, selected fill and 600 weight |

Light accent #4D7C0F on selected #ECECEF is below 4.5:1 and **must never be used
for text there**. Selected rows, pills and menu choices use primary text; only
their icon/outline uses the accent (≥3:1). Primary is the replacement text paint
on selected grounds and is measured above. Input/card borders and separators are decorative and use subtle border tokens.
Radio rings, selected outlines, focus indicators and icons carry the ≥3:1 non-text
contrast requirement; unselected radio indicators use weak text ink, not border-l3.
Radio circles, selected accent outlines, focus outlines and icons exceed 3:1;
the translucent focus halo supplements the opaque outline. Disabled paint is not
used for readable status. Task page, primary/weak text and accent follow shared roles; its approved
composer, panel, menu, placeholder, disabled and scrim paints remain specific. Error-primary remains #BC3B26 light / #FF5A45 dark; its task-panel/shell
contrast is checked alongside these roles. Button fill is lime in both themes;
its dark text has ≥15:1 at rest. Selection text retains the paired foreground
for the theme's accent, separate from button-primary-foreground.

Task page/text/accent/button roles follow the app roles. Approved New agent
composer, panel, menu, placeholder, disabled and scrim paints remain specific.
Account-menu Appearance and System text size selections use primary text, selected
fill and 600 weight, never accent text on the selected ground. Usage card text,
dialog text and the legacy Appearance notice use these same primary/secondary
roles. Existing theme state, Usage behavior and legacy redirects remain canonical.

The four former vendored sheets (`design-platform.css`, `effects-typography.css`, `scrollbar.css`,
and `shiki.css`) keep their filenames but become first-party token sheets governed here. The
"kept verbatim" policy retires. `web/src/styles/vendor/LICENSE-deepseek-harness.txt` remains because
the icon components under `web/src/ui/icons/` are still vendored under that licence.

Spacing, radius and z-index have **no theme tokens**. They are literal pixels in the component sheet
that uses them, which is this language's own idiom: a spacing scale invites a component to pick the
step above when the one it needs is not there.

Geometry never changes with the appearance. Radii come from the Lumen set — 4, 6, 7, 8, 10, 12,
16, 999, 50% — and apply in both themes. The z ladder is: content 0–10,
chrome 2 and 20, tooltip and menu 100, submenu 101, modal 1000, toast and onboarding 1100.

---

## 3. Theming, on the body

**Tokens are declared on `body`, and dark is an attribute on `body` — never `:root`.** This is
load-bearing rather than stylistic: sheets that style the scrollbar resolve against the same element
the tokens are declared on, and an `html`-scoped rule cannot see them.

The appearances are `light` and `dark`. The preference is `light | dark | system`, defaulting to
`system`, and is stored only at `localStorage['cma:appearance']`. Invalid or inaccessible storage
resolves to `system`; no historical key is read. The inline script immediately after `<body>`
resolves the canonical value before first paint — resolving `system` through
`prefers-color-scheme`, setting `color-scheme` on the document element and toggling
`data-ds-dark-theme` on `body`. Without it the first paint is the wrong theme and the correction is
visible.

While the preference is `system`, a `change` listener re-applies it, so an Agent conversation left open across
sunset follows the machine.

---

The task-home specific palette and sidebar refinements are defined in the dated
**2026-11-05 — New agent landing redesign** contract below. Its specific tokens
are part of this alias contract and resolve through the static zinc/lime ramps in
both appearances.

## 4. Typography

Both appearances use one self-hosted interface face and one code face, the shadcn/ui pairing:

| Role      | Both appearances                                                                                                                                                                  |
| --------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Interface | `'Geist', 'Noto Sans SC', -apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', system-ui, 'Helvetica Neue', Helvetica, Arial, sans-serif` |
| Code      | `'Geist Mono', 'SF Mono', 'JetBrains Mono', 'Fira Code', Consolas, 'Liberation Mono', Menlo, Courier, 'PingFang SC', 'Microsoft YaHei'`                                          |

Geist and Geist Mono are variable WOFF2 files (weights 100–900) from Vercel under OFL 1.1. The
earlier faces (Familjen Grotesk, Archivo, IBM Plex Mono, JetBrains Mono and Shrikhand) remain
vendored for the brand lockups and the signed-out lockup until those assets are redrawn. Every
family is OFL, is stored under `web/src/assets/fonts/` with its licence beside it, and uses
`font-display: swap`. There is no font `@import`, Google Fonts link, or third-party request. The
exact file list is tested.

Bare `monospace` is forbidden because Windows may resolve it to a CJK serif.
There is no terminal-specific font stack in this release.

The ramp is declared as **composite `font` shorthands** rather than loose size and leading tokens:
one name carries weight, size and line-height together, so a step cannot be half-applied.

| Step      | Weight / size / leading |
| --------- | ----------------------- |
| `xl-24`   | 600 24/32               |
| `l-20`    | 500 20/28               |
| `m-18`    | 500 16/28               |
| `base-16` | 400 16/24               |
| `s-14`    | 14/22                   |
| `xs-13`   | 13/20                   |
| `xxs-12`  | 12/18                   |
| `xxxs-11` | 11/14                   |

A `-strong` companion exists from `base-16` **downward only**. The three largest steps are already
emphatic; a heavier version of them is a size, not an emphasis.

Long-form text has its own ramp — headings at 700 down to 600, body at 16/28, tables at 15/25, code
at 14/22. The same sizes as the interface ramp, set looser: a label and a paragraph of one size are
different objects, and treating them alike is what makes a long answer tiring.

**Four weights: 400, 500, 600, 700.** An intermediate weight is not available. A non-variable face
snaps one unpredictably, so asking for 510 is asking for a surprise.

The reference voice sits around that existing composite ramp:

| Use                  | Light — Inkwell                          | Dark — Lumen                                       |
| -------------------- | ---------------------------------------- | -------------------------------------------------- |
| Body                 | 15px / 1.62                              | 15px / 1.6                                         |
| Display              | 44px, 700, `-0.035em`                    | 56px, 900, `-0.02em`; Archivo may set `"wdth" 118` |
| H1                   | 32px, 700, `-0.03em`                     | 32px, 800, `-0.03em`                               |
| H2                   | 22px, 600, `-0.02em`                     | 22px, 700, `-0.02em`                               |
| Kicker / field label | 12/18, 500, sentence case (`xxs-strong-12`) | 12/18, 500, sentence case (`xxs-strong-12`) |

Labels, kickers, eyebrows and pills are set in the interface face at `xxs-strong-12`, in sentence
case with no tracking (§29). Mono is kept for identifiers — IDs, paths, commands, versions — where
the face says "this is a literal". Letter spacing is otherwise zero unless this document names a
display exception.

---

Task home has a Geist display exception: `clamp(32px, 4.5vw, 56px)`, weight
700, line-height 1.05 and letter-spacing -0.04em. Its subtitle and textarea use
17px/26px, repository dialog title 26px/32px at 700, and keyboard/model text
Geist Mono 12.5px/20px. See the 2026-11-05 landing contract.

## 5. Depth

Depth changes with appearance. Light shadows with `zinc 950` at low alpha; dark separates by zinc
luminance and black depth, then adds lime glow only to an accent element named in §2.

| Whole-value token           | Light — Inkwell                                                      | Dark — Lumen                            |
| --------------------------- | -------------------------------------------------------------------- | --------------------------------------- |
| `--dsw-shadow-lv1`          | `0 1px 2px rgba(9,9,11,.06)`                                         | `0 1px 2px rgba(0,0,0,.45)` **derived** |
| `--dsw-shadow-lv1-blur`     | `0 4px 12px rgba(9,9,11,.06)` **derived**                            | `0 8px 20px -8px #000` **derived**      |
| `--dsw-shadow-lv2`          | `0 2px 4px rgba(9,9,11,.06), 0 8px 20px -8px rgba(9,9,11,.12)`       | `0 20px 44px -14px #000`                |
| `--dsw-shadow-lv3`          | `0 18px 44px -14px rgba(9,9,11,.18)`                                 | `0 40px 90px -30px #000`                |
| `--dsw-shadow-accent-hover` | the neutral `--dsw-shadow-lv2` value                                 | `0 6px 26px -8px rgba(195,253,67,.35)`  |
| `--dsw-glow-accent`         | `none`                                                               | `0 0 14px rgba(195,253,67,.35)`         |
| `--dsw-ring-accent-soft`    | `0 0 0 3px rgba(63,98,18,.08)`                                       | `0 0 0 3px rgba(195,253,67,.10)`        |

Every shadow, glow, and ring is a whole-value token. Component sheets never assemble a `box-shadow`
themselves. A floating surface still carries a 1px border as well as shadow, so its edge survives on
either ground. Ordinary borders are 1px; a named 2px leading state rule is a marker, not a second
general border weight.

The reasoning fades remain tied to their surface: light fades from `#FAF2E0` and its selected paper
surface; dark fades from `#060606` and its selected coal surface. Their existing `20.19%` stop and
transparent end remain unchanged.

---

## 6. Scrollbars

10px wide in both themes, with a `99px` thumb radius and transparent track and corner. Level 1 uses
`scrollbar-bg-l1` / `scrollbar-hover-l1`; an elevated surface uses the stronger level 2 pair. The
reference libraries paint the track and cut a 3px track-coloured border into the thumb; both are
deliberately dropped. An app scrollport is not a document page, and a transparent track leaves its
surface continuous.

**A scrollbar is drawn only while the pointer is inside its region**, and lingers briefly after it
leaves. A permanently visible scrollbar is chrome competing with content in a surface whose whole job
is reading. The WebKit and Firefox implementations stay mutually exclusive; drawing both produces
two different widths in engines that support both selector families.

---

## 7. Code and syntax

Syntax colours are first-party and palette-consistent, with nine roles per theme:

| Role                              | Light — Inkwell | Dark — Lumen |
| --------------------------------- | --------------- | ------------ |
| `--shiki-token-constant`          | `#0E61AC`       | `#AEE734`    |
| `--shiki-token-string`            | `#557A2E`       | `#E7D98A`    |
| `--shiki-token-comment`           | `#8C8474`       | `#6B7361`    |
| `--shiki-token-keyword`           | `#0E61AC`       | `#C3FD43`    |
| `--shiki-token-parameter`         | `#BC3B26`       | `#FF806F`    |
| `--shiki-token-function`          | `#7B3F93`       | `#9ACBFF`    |
| `--shiki-token-string-expression` | `#3E7A22`       | `#D7FF7D`    |
| `--shiki-token-punctuation`       | `#4C5A68`       | `#A9B199`    |
| `--shiki-token-link`              | `#0A4C88`       | `#AEE734`    |

These roles remain dormant in conversation rendering. **Agent content still takes no colour.** The
palette exists for the code primitive, but the Agent surface does not syntax-highlight what the Agent
wrote. Code-block chrome re-colours through aliases; content stays faithful.

Code blocks take a 12px radius on their own ground, a sticky banner carrying the language and a copy
control, and a body at 13/22 that wraps rather than clipping.

---

## 8. Icons

Icons are **inlined SVG components**, not a library. Each takes `{ size, className }`, defaults to
the size it was drawn at, and paints with `currentColor` so it inherits the label rank around it.

No icon font, no sprite sheet, no runtime fetch.

---

## 9. The frame

A full-viewport three-column grid — **sidebar, centre, details** — with no app bar, no status band,
and no footer. A small client-side router owns the canonical hierarchy, Profiles,
Marketplace, publisher, public Trigger, Operations and Developer routes. Routing changes which view the centre and details columns project; it never
starts work, confirms an action, retries a write, or treats a browser history event as authority.

| Column  | Min | Max | Default        | Collapsed  |
| ------- | --- | --- | -------------- | ---------- |
| sidebar | 264 | 420 | 280            | 56         |
| centre  | 640 | —   | takes the rest | —          |
| details | 300 | 520 | 360            | 0 (closed) |

**The concession chain.** When width runs out: details shrinks to its minimum, then closes — its
subtree stays mounted and the preference is kept, so reopening restores what was there. The sidebar
never concedes. The centre absorbs whatever is left, which is §1's rule that the surface owns the
leftover width.

Below 1024px the sidebar auto-collapses, with an explicit override so somebody who opens it on a
narrow screen keeps it open.

**Phones have no rail.** Below the centre's 640px floor a permanent 56px rail would take a seventh
of a 375px screen, so a collapsed sidebar takes no width and leaves the tab order. The centre gains
the frame's only bar: 48px high, with a 44px **Show navigation** key and the current service name.
The sidebar then opens as a sheet over the page, at most the viewport less 48px wide, behind the
`bg-mask-1` scrim. The page behind is inert while the sheet is open. Escape, the scrim, or choosing
a destination closes it. Between 640px and 1023px the rail and the automatic collapse remain.

Column widths are **transient preferences**, not persisted state. Drag handles are 8px strips with
pointer capture; the details edge shows a small pill on column hover, and the collapsed rail shows
none — there is nothing to drag when the width is fixed.

The grid transitions its tracks, except while dragging, where the transition would fight the pointer.

---

## 10. The sidebar

The sidebar is how somebody gets to a conversation, so it spends its height on the list of Agents
rather than on chrome.

**Header.** One row: the CMA mark as the **CMA home** link (a 32px target), the **service menu**, and
the 32px collapse key (**Hide navigation** / **Show navigation**). Clicking home opens `/`, the
same landing before and after sign-in. The home landing starts with the sidebar collapsed on every
visit; entering an app or service from home expands it, and direct service links start expanded.
Within and between services the manual choice persists until the next visit home, browser history
follows the same defaults, and below 1024px the automatic collapse still applies.

**Service menu.** A button showing the current service's glyph and name, **Agent Manager** or
**Event Gateway**, with a trailing chevron; its accessible name is `Service: <name>`. It opens the
shared §12.3 menu titled **CMA services**. Agent Manager and Event Gateway are real links, so a
modified or middle click opens a new tab, with the current one checked. **Image Manager** stays
listed, `aria-disabled`, with the reason *Coming soon. Image Manager is not available yet.* under
its label, and never routes. Consumer routes belong to Event Gateway. Collapsed, the button keeps
only its glyph. The build revision is no longer in the header; it is the **Copy build version · <version>**
row of the account menu.

**Action row.** Directly under the header, in this order. Every entry is a plain full-width row,
32px high: a 16px glyph, 8px gap, and a 14px label in the primary ink, with no border and no fill
at rest and the hover ground on hover. The current page alone uses selected fill,
primary text and an accent icon, matching New agent’s current treatment.

- **New agent** is shown whenever the caller holds `use_own_sandboxes` and is not a visitor. When
  it cannot be used right now it stays in place, `aria-disabled` at 40% opacity, with its reason in
  12px secondary text beneath it and as its accessible description: *Switch to Personal to create
  an Agent.* in Shared or Organization scope, *Checking your access.* while the scope is still
  being confirmed.
- **Search**, with its bordered `⌘K` (Ctrl K elsewhere) key hint in tertiary ink,
  opens the command menu below. Visitors do not get it.
- **Artifacts**, when available, remains a link to the owner’s document workspace.
- **Triggers**, a navigation row to `/triggers/manage` for callers who may publish Triggers and are
  not visitors, current on every Trigger route, where it takes the active-item ground and medium
  weight. It replaces the old *New agent trigger* row.

Collapsed, each keeps only its glyph and a hover title. Below 30rem every row is 44px.

**Filters.** The **Tasks** heading and **Filter** sliders key share a row. The
32px key (44px below 30rem) uses aria-expanded and aria-controls to disclose the
bordered **Navigation filters** group. It retains the **Navigation scope** picker,
**Agent status filter** / **Sandbox status filter**, and **Clear filters**. Each
picker keeps its own shared Menu; scope/status selection leaves the disclosure
open. Only the task view adds a separated **Manage environments** button at the
end. Activating it closes the disclosure, saves route/scope/scroll/focus, opens
Environments and focuses **Back to tasks**. Returning restores the saved task
state and filter choices. Back to tasks uses an inset 2px accent focus ring with
no outside outline or layout shift. There is no Settings heading or Settings link.
Appearance and Usage remain in the account menu as specified by the canonical
Account Usage entry. Non-default filters keep their dot and summary description.

Current sidebar links (Artifacts and Triggers) and New agent use the selected fill,
primary text and accent icon. Links retain their existing destinations and semantics.

**Agents view.** One flat list of every Agent the scope shows, most recently active first, with no
Workspace or Sandbox group headings. Each row is one line, 32px high (44px below 30rem):

- A 24px leading slot holding the Agent's character, posed by its effective status (§28). Only
  Starting, Working, Needs approval, Resuming, Pausing and Updating move; Ready, Paused, Stopped and
  Needs attention draw their still moment. The row is inset 8px and the slot sits 4px from the
  title, so the character's centre lies under the action-row glyphs and the title on the
  action-row labels' edge. The slot's hover title is the status word.
- The Agent title in 13px, truncated on one line with an ellipsis, full text on hover.
- The relative age (12px tertiary, tabular) and the row's **⋯** menu at the right, shown only while
  the row is hovered, holds focus, or is the current Agent. At rest the menu keeps its place
  invisibly, so revealing it never shifts the title; it is still reached with Tab from its row.

The status word is part of the row's accessible name even though no word is drawn, so the
list reads the same to a screen reader as before. The current row takes the active-item ground.
Deleted Agents and removed Workspaces are absent; Agents of a paused Sandbox are dimmed but still
open their conversation. The Shared and Organization Agent lists keep their two-line rows, which
also carry each Agent's creation, channel and access facts.

**Sandboxes view.** The tree is Sandbox -> Workspace -> Agent, grouped Live and Paused. Sandbox rows
use the Sandbox's own name — the same selector the Sandbox page uses — never a generated
`Sandbox Machine - <id>` label. Workspace labels use the selected Agent's confirmed runtime working
directory, with the recorded workspace path as fallback. Agent labels share one selector with the
Agent header and browser document title. An explicit native provider title takes priority, even
when it equals the automatic setup message. A prompt preview confirmed by public provenance to come
from automatic setup is omitted; the configured Agent name is the fallback. Preserve descriptions
whose legacy provenance is unknown. Do not guess setup from wording or choose a task by its position
among user messages: service-access callbacks can also use that role. Display selection leaves the
raw runtime description and conversation history intact. Runtime metadata arrives through the
public event stream, never a directory poll. Long labels truncate with ellipsis and expose full text
on hover. Rows retain stable status/icon slots, and expansion belongs to the signed-in viewer.

Agent metadata also shows its authorized immutable creation category in the
existing secondary text style: Manual, Trigger, Channel, or Unknown origin.
Parent summaries count each child's category; an unavailable child fact
suppresses the summary. The sidebar has no Agent creation or Agent channel
activity filters. Loading and refused facts never become the Unknown origin
category. No private actor or external source references appear in rows.

Expansion and navigation are separate keyboard-operable commands, with named
`aria-expanded` state. A collapsed group retains its count. Paused, pausing and
reviving Sandboxes retain descendant navigation and expansion for authorized
Agent conversations and history. Runtime mutations remain unavailable until
Active with no pending lifecycle operation. Revive has an explicit moving state
that becomes a still glyph under reduced motion. Loading and failed child reads never
masquerade as empty groups.

Menus expose only public operations: Sandbox New workspace/Pause/Delete,
Workspace New agent/Remove, Agent Stop/Delete. A Workspace's New agent opens New
agent in that Workspace rather than a new one. Destructive operations use §12.4.
Visitors see their own hierarchy but no creation controls. Capabilities are read
from the authenticated public snapshot, not inferred from a role label.

**Command menu.** ⌘K on Apple keyboards and Ctrl+K elsewhere toggle it from anywhere, except while
an input method is composing; the Search button opens it too. It is the shared §12.4 dialog, named
**Command menu**, holding a 48px search field over a result list. The field is a combobox: focus
stays in it while the arrow keys, Home and End move the active option, which wraps; Enter runs it
and closes the menu, and Escape closes it. Results are grouped **Agents** (title, `<workspace> ·
<sandbox>`, status word; most recent first), **Sandboxes**, **Workspaces**, **Actions** (New agent
when it can be used now, Appearance: System / Light / Dark) and **Go to** (Agent Manager, Event
Gateway, Triggers and Operations when permitted, Docs). Every word typed must match the label, its
second line, its status or its group. At most 60 results are listed; an empty match says *No
results.* The menu shows only what the shell has already read: opening it sends no request and
starts no work. Visitors do not get it.

The account row is the only foot content: avatar, name, and email. Long identity lines truncate
within the fixed seat. Its upward-opening menu contains Docs, capability-gated Operations,
Appearance, System text size, **Copy build version · <version>**, and Sign out.
The build row copies `CMA <version> (<full commit SHA>)` from the bundle's own
compile-time identity; local runs show `dev (local)`. It announces **Build version
copied** on success. A denied or absent clipboard announces **Could not copy. Select
the build version to copy it manually.** and exposes the full selectable identity
in the same enabled row for retry. The identity wraps in the existing secondary
menu text style; its code text permits selection and uses the established mono face.
Deployable artifacts reject missing or placeholder version/revision metadata;
ordinary local builds remain supported. Existing tabs keep their own identity.

There is no Chat/Agent switch, session group, pricing control, or terminal plane
choice. Selecting an Agent opens its conversation; selecting a Sandbox opens
its facts and Workspace list. Home keeps the tree with nothing selected and
details closed. It shows Welcome to CMA beside the existing unframed 3D brand
stage and creation action appropriate to the caller's capability. The home mark
rotates once every eight seconds, retains its interactive spin, and stops under
reduced motion or when hidden. Its geometry remains reserved while Three.js loads.

---

The New agent action may carry `aria-current="page"` on task home. Its current
fill, accent icon, bordered shortcut tag and account divider follow the
2026-11-05 landing contract; all existing sidebar collapse behavior remains.

## 11. The centre column

**Every centre view starts with one page header**, so the same three questions — where am I, what
state is this in, what can I do — are answered in the same place on every page. Top to bottom:

- A **breadcrumb**, a `nav` named *Breadcrumb* holding an ordered list in 13/18 secondary text with
  drawn `/` separators. Every step but the last is a real link (a modified click opens a new tab);
  the last is the current page, `aria-current="page"`. Long steps truncate with their full text on
  hover.
- The **title** as the view's only `h1`, 20/28 semibold, truncating on one line, focusable
  (`tabindex="-1"`) so a view can move focus to it. Beside it, the resource's **status** and short
  facts in the code face as 22px chips, such as a Worktree branch or a Sandbox id.
- An optional **description** line in 14/20 secondary text.
- The **actions** on the right: at most one primary, one secondary, and a *More actions* menu
  (§12.3) for everything else, destructive actions included, each unavailable row keeping its
  reason. Below 48rem the actions take their own full-width row.

The header has no padding or border of its own; each view places it in its own gutter. Breadcrumbs
by view: Agent — `<Sandbox> / <Workspace> / <Agent title>` with the Worktree branch as a chip (a
shared reader, authorized for the Agent but not its parents, sees `Agent Manager / <Agent title>`);
Sandbox — `Agent Manager / <Sandbox>` with its id as a chip; Workspace — `<Sandbox> / <Workspace>`
for its owner (a shared reader sees `Agent Manager / <Workspace>`); Agents — `Agent Manager / Agents`; Operations — `Operations / <tab>`; Triggers —
`Triggers / <Trigger title>`; Event Gateway — `Event Gateway / Channels / <channel>` and `Event
Gateway / Consumers / <consumer>`; Docs — `Docs / <document>`.

The Agent header's actions are a Pin toggle and *More Agent actions*. The menu starts with
**Show Agent details** / **Hide Agent details**, then the creation origin (§13), then **Stop agent**,
**Export history** and **Delete agent**, with Delete unavailable until the Agent is stopped
(*Stop this Agent before deleting it and its Worktree.*), every row unavailable while the Sandbox is
not active, and every row held while another action is still being confirmed. These are the same
public operations as the sidebar row menu, with the same confirmations. The Agent header folds its
breadcrumb into the title line and collapses out of the way; §13 describes it.

**The browser tab is named after the view**: `<view or resource> · CMA`. Services stay mounted
while hidden, so each view registers its name rather than writing it: the most recently registered
view that is on screen names the tab, and leaving or hiding a view hands the name back to the one
behind it. A view whose name is not known yet registers nothing.

There is no plane strip or terminal transition. Worktrees are Agent facts rather
than another navigation band. The composer and history own the remaining height.

A lost connection is a persistent status strip in the affected surface. Keep
loaded history and the draft visible. A read error must not replace successful
content or report an empty list. Errors and pending commands remain scoped to
their resource. Boot reserves geometry and never disables sign-in while a
decorative asset loads.

When the service cannot be reached while checking identity, the page says so in words a person can
act on: a 20px heading *CMA could not reach its service*, the line *It may be starting up. Reload
to try again.*, a primary **Reload**, and the service's own sentence beneath as a 12px mono detail
for whoever reads a report. Unreachable is never shown as signed out, and nothing on the page is
coloured as an alarm.

---

Task home uses the centered 780px fluid landing column defined in the
2026-11-05 contract, with wrapping controls and no sidebar restacking.

## 12. Controls

One shared button in five variants — accent `primary`, `ghost` (the default), `outline`, `toolbar`,
and tinted `danger`. The standard control is 40px high at radius 10; compact is 32px at radius 6.
An icon-only button uses the same fixed width as its height. Disabled is 0.4 opacity and a
not-allowed cursor, never removal.

Two heights and no third. A third is one everything else can then reach for, which is how a row ends
up with three controls on two baselines. A surface that genuinely needs its own proportions declares
them in its own sheet, where the decision is local and a reviewer can find it — never by widening the
shared set.

Primary takes `button-primary-fill`, on-accent ink, and `--dsw-shadow-accent-hover`; pressed takes
`button-primary-active`. Danger takes danger ink, a danger-tinted border, and a danger-wash ground at
rest, then a stronger wash and solid danger edge on hover. **Danger is not the safeguard.** A coloured
button is as quick to click as any other; the acknowledgement step below still protects an action that
loses something, and its final confirm remains the ordinary primary.

### 12.1 Badges and state

A **pill** is a 24px label at radius 12 — a span until it is given something to do, and a button the
moment it is. That is not a convenience: a span that reacts to clicks is unreachable by keyboard and
silent to a screen reader, and a button that never does anything announces every static label as
pressable. Selection is an inset ring rather than a border, so moving it along a row does not shift
the row.

A **state dot** is reinforcement and never the signal — it is `aria-hidden`, and every caller pairs it
with the word, so a state reads the same to somebody who cannot distinguish the colours. The
one-line personal Agent list (§10) draws the Agent's character (§28) instead of a dot and no word:
there the word is carried in the row's accessible name and the slot's hover title. Three of its
four states are a solid core inside a same-coloured halo. The fourth is not colour alone: _running_ is
a claim about right now, so its solid live core sends one expanding ping ring on a quiet loop. Under
`prefers-reduced-motion: reduce` the ring is absent and the labelled core stays still. This replaces
the eight-cell 3×3 chase with the reference libraries' simpler liveness signal.

A state that has ended draws **no dot at all**. Every state the dot has asserts something — succeeding,
needing you, running, failed — and a finished Agent asserts none of them; painting it green because
green was the nearest colour would claim it finished well, which is a different fact.

**One tone per status word, everywhere.** Every resource status maps to one tone in
`vocabulary/status.ts`, and the sidebar, page headers, tables and the command menu all draw it the
same way:

| Tone | Words | Dot |
|---|---|---|
| active | Starting, Working, Pausing, Resuming, Updating runtime, Updating access, Deleting | accent core with the ping |
| neutral | Live, Ready | a quiet static dot in the tertiary ink, no halo |
| warning | Needs approval | warning dot; the word takes the warning ink |
| error | Needs attention | error dot; the word takes the error ink |
| muted | Paused, Stopped, Deleted | no dot; the word in the secondary ink |

Live and Ready are not the success colour: in dark that is the same lime as the accent, so a settled
Sandbox read exactly like one that was working. A Sandbox with a lifecycle operation in flight takes
the active tone of the operation's word. A sidebar status that may be stale draws no dot.

A **badge** carries a status beside a title or in a table cell: 22px high at radius 6, a hairline
border, the dot and the word in 12px medium, and no fill — a filled status beside a filled primary
button reads as a second button. A badge whose status changes while somebody reads it announces
politely.

An **alert** is the one way a view says something about its resource: radius 12, one hairline, a
glyph, the words and any actions on one row that wraps. Information (neutral ground and edge) and
success (success glyph) are polite status updates; a warning (warning ground and edge) or an error
(danger wash and edge) interrupts. Only the edge, ground and glyph take the tone; the words stay in
the primary ink. A view stacks its alerts in one place under its header, 8px apart — on the Agent
surface that is one grid row, so a second message can never be placed below the composer.

**Loading reserves geometry.** A view that is loading draws a skeleton in the shape of what will
arrive — rows for a list, the page outline while identity is checked — with one polite status
naming what is loading. The shared `.skeleton` shimmer stops under reduced motion. Bare "Loading…"
sentences and full-screen characters are not loading states.

### 12.2 Fields

A 44px field at radius 10, with its border on the **wrapper** rather than on the input. That is what
lets a glyph sit inside the field instead of beside it, and it is why a field with an icon and one
without focus identically. A label is the §4 label voice (`xxs-strong-12`, sentence case) in the caption ink. Focus swaps the
edge to the accent and adds `--dsw-ring-accent-soft`; because the ring is a token and does not enter
layout, the control does not grow. Invalidity uses the same edge and the danger wash.

This is not the composer. A message box grows with what is typed into it and is a different control
with different rules; sharing one would make every search box growable.

A Sandbox Profile records explicit CPU and memory. The editor offers
Standard, Large and Custom in cores and GiB, validates both against deployment
bounds and converts to wire units at the boundary. Creation may select the
exact pinned deployment-default profile, never an unrecorded resource default.
Paired fields stack below 30rem.

**New agent** is one step in one dialog titled *New agent*. Its description names
where the Agent will work: *Start an Agent in a new Sandbox with an empty
workspace.*, *Start an Agent with a new workspace in this Sandbox.* or *Start an
Agent in the selected workspace.* **Where it works** is a radio group of bordered
choice rows at radius 10 with 12px padding, two columns from 30rem and one below:
*New Sandbox* (second line *Latest CMA runtime image*), then *New workspace in
<Sandbox>* for each of the owner's Sandboxes, then *<Sandbox> / <Workspace>* for
each of their Workspaces. A Sandbox or Workspace row shows its status badge as its
second line. The selected row takes the accent edge and the accent wash. A row
that cannot be used now stays listed and `aria-disabled`, with its reason as its
second line: *Available when the Sandbox is active.* for a Sandbox that is not
Active with no pending operation, *The Workspace is not ready.* for a Workspace
that is not Ready, and *You don’t have permission to create a Sandbox.* for New
Sandbox without that capability. Arrow keys move between rows and select a usable
one. The rows come from the same public Sandbox and Workspace reads as the
sidebar; until they arrive the target the dialog was opened for is still
selected. Opening New agent from a Sandbox selects its new-workspace row, and
from a Workspace selects that Workspace, whose Agent is created there through the
lifecycle request's `workspace_id`.

**GitHub repository (optional)** keeps its strict HTTPS github.com rules. It is
checked when the field loses focus and on Create agent, never while typing; the
error *Enter an HTTPS GitHub repository URL.* is announced once when it appears
and leaves as soon as the value is valid. A read-only **Agent settings** summary
reads *gpt-6 · High reasoning · Autonomous* with the line *The Agent works without
asking first. Change this anytime from the message box.*; there is no control to
change it here. The footer starts with the note *Includes access to ChronoAI
models and the Ornn skill library.* and
ends with Cancel and the primary Create agent, which reads *Starting
provisioning…* while the request is pending and *Recover creation* while a
retained request is unconfirmed. Each location keeps its own retained request,
so choosing a location with an unconfirmed one shows its recovery. The choice is
locked while a request is pending and once a receipt exists. Labels and help use
the shared field label and hint voices.

The profile-based compatibility creation names **Sandbox environment** and **Project workspace**, with
short 13px secondary explanations of the environment and the project folder.
The profile step explains model, skills and independence; optional title and
first-message fields explain their effect. Deleted and deleting resources are
absent from location choices, not disabled historical choices. Selected locations
refresh from public records so deletion cannot leave Continue enabled.

Hovering a profile row for 200ms or focusing its controls opens a nonmodal details
preview. It is anchored to the row, not pointer movement, and uses the existing
public profile settings renderer and original-name links. The preview is 360px
wide and 440px high, constrained to 12px viewport margins, with an 8px radius,
16px padding, menu fill and level-3 shadow. The body alone scrolls. It uses the
shared menu placement calculation, right of the row on desktop and below on
narrow screens, clamped inside the viewport and vertically between the owning
dialog's header and footer. On short screens its body becomes shorter and scrolls;
the close and submission controls must never sit behind the preview. Selecting a
profile dismisses its open or pending preview. It stays open while hovered or
focused, closes after 150ms outside, and Escape dismisses only the preview.
Loading and read failure occupy the same stable box; hovering never selects or
creates anything. Fade-in uses the existing fast duration, with reduced-motion
equivalence. Profile links remain independently clickable.

### 12.3 The dropdown

Operations usage filters use this same portalled Menu/Picker. Large identity and
runtime lists have an inline name-search field, a scrollable page of at most 50
choices, and Previous/Next controls. Search is debounced; paging replaces the
current page rather than accumulating DOM rows. Arrow keys move from search to
choices; Escape closes and restores focus. Loading, empty and retry states stay
inside the dropdown. Selected identities show a display name with a stable short
derived reference; missing names use Unnamed owner/Sandbox, never a raw ID.
The time-range picker retains Custom with local date/time fields. Apply filters
commits the draft atomically; Clear filters restores Last 24 hours and all choices.

One control for choosing a value and for choosing an action. Both are a card at radius 12 on the menu
fill with an inverted hairline and the level-3 shadow, holding 40px rows at radius 7. The Lumen row
radius wins for both appearances; geometry never flips with theme. Hover is the neutral hover
ground in the primary ink (§29), while a danger row uses the danger wash and danger ink.

Rows are reachable by arrow keys, Home and End, and **wrap** rather than stopping dead — a list that
stops at the last row puts the first one four presses away in a menu of five. Opening a value picker
puts focus on the row already selected, so arrows move from where you are.

A row that cannot be chosen is `aria-disabled` rather than `disabled`, so it keeps its place in the
keyboard order: the reason for listing an unavailable option at all is that the reason can be read,
and a row nobody can reach cannot have its reason read. The reason sits under the label, not beside
it, because these reasons run long.

A menu carrying a current value uses `menuitemradio` — that is what "one of these alternatives" means,
and it is the role `aria-checked` is defined for. Selection is a **trailing check, never a fill**: a
fill would make the selected row and the hovered row the same colour, so running the pointer down the
list would look like the selection following it.

**The list is always drawn on the body, positioned from the trigger's box** — never in place. This is
the load-bearing half of the rule above: nudging back from an edge can only happen to a list that is
free to be moved, and a list rendered inside its trigger is clipped by whatever scrolls above it
instead. Every list that was drawn in place was drawn wrongly somewhere — cut off by the dialog
scrollport it sat in, opened past the left edge of the window, run off the bottom of the screen — and
a trigger cannot know what encloses it or how near an edge it sits, so the question could not be
answered where it was being asked.

Near a viewport edge the list is nudged back, never flipped. Flipping moves it somewhere the pointer
is not; it should still read as belonging to the control it came from.

Closing returns focus to the trigger when it was still inside the list. Otherwise the next Tab starts
again at the top of the document, which is as far from where somebody was working as it is possible
to put them.

Escape from an open list or its trigger closes the list without changing its value. An enclosing
dialog stays open; a second Escape from the trigger can dismiss it.

A list may be held to **its trigger's width** where the trigger is a row rather than a glyph, so its
edges are the row's edges. The list is on the body and cannot take this from the trigger in CSS, so
the width is measured with the position.

### 12.4 Dialogs, and confirming something irreversible

Every modal uses the shared Modal primitive: one centered card on layer 2,
behind the theme's `bg-mask-1` and blur. Width is `min(700px, 60vw)` and maximum
height is `60dvh` with a `60vh` fallback. All four card edges stay at least 20%
inside the whole viewport on screens wider than 480px. No call site overrides
width, height or placement. Header and wrapping footer retain the shared style;
long contents scroll inside the body. Radius remains 16px. The overlay portals
to the document root and locks background scrolling. Anchored selection menus
and tooltips retain their menu semantics and are not modal dialogs.

At a viewport width of 480px or less the card takes the full width less a 16px
gutter on each side: 60% of a 320px phone is 192px, too narrow for a form's fields
or its footer keys. At viewport widths or heights of 480px or less, the entire card scrolls instead
of shrinking the body between the header and footer. Content keeps its natural
height so every field, close control and footer action remains reachable. At a
viewport width of 480px or less the card may also use the full height less a 16px
gutter top and bottom (`calc(100dvh - 32px)`), so a form's primary action is on
screen when it opens instead of below 40% of empty viewport; padding and radius are
unchanged, and wider screens keep the 60dvh cap and 20% margins.
Shared input wrappers may shrink to their available width without forcing a
narrow dialog to scroll horizontally.

Closed dialogs release their focus trap and return focus immediately. Their inert,
accessibility-hidden exit frame may remain for the shared 200ms opacity fade,
then unmounts. Initially closed dialogs render nothing. Focus is trapped only
while open; Escape closes a dismissible dialog. Reopening during the fade cancels
the pending removal and restores one active dialog.

A width chosen per call site is how two dialogs asking the same kind of question end up different
sizes. There are no per-dialog geometry exceptions or size props.

**No dialog opens over a dialog.** A surface that must ask something while one is open asks in place of
the row's own controls — which is also what a reversible destructive action does.

An action that **loses** something asks differently: an explicit acknowledgement, with the confirm key
unavailable until it is ticked, and a sentence saying what is actually lost rather than asking whether
you are sure. The tick is the safeguard; the warning glyph is the only red on the card, and the confirm
key is the ordinary primary. This is spent only where the consequence is loss — spending it on every
destructive-sounding thing is how it stops being read.

Ambiguous initial-task recovery is the one non-loss exception. CMA has already exhausted every
automatic reconciliation path but cannot establish whether the first task started, so authorizing a
replacement may execute it twice. Its dialog uses the same acknowledgement pattern, says both facts
without euphemism, and keeps the retained revision, payload hash, and idempotency key. An ambiguous
recovery response leaves that dialog mounted and non-dismissible while it offers only same-key
reconciliation; a finite response restores ordinary dismissal. This exception is bounded to the
server's explicit `recovery_required` prompt state and does not admit acknowledgement dialogs for
ordinary retries.

The rules §19 protects are unchanged by any of this: a control that can never work is **absent**; one
that is currently unavailable is **disabled and says why**.

A deployment-capacity refusal learned while creating a Sandbox stays in the dialog's existing alert
seat. It names the resources requested and the resources the deployment reports free, then
says that a smaller Sandbox may fit. It does not disable a key or add a capacity state to the form:
the client cannot know shared headroom before asking, and polling or retaining the last refusal would
draw a stale answer as current.

### 12.5 A card that asks in steps

When one card needs a second deliberate choice, it may replace its own body and footer with the next
step. It does not open another card: §12.4 forbids one dialog over another, and the replacement remains
the same card at the same width. A change of width between steps would read as a different surface.

The footer moves backward with a secondary **Back** key and forward with the ordinary primary action.
A step that states a cost states the rate the service supplied and never projects a total. A projection
assumes how long somebody will work and presents a number the service will never actually charge.

### 12.6 Tables

Records somebody compares across rows — channels, consumers, Operations records — are a real
`table` with column headers, so a row reads the same to a screen reader as it does to the eye. A
list of cards or bare rows makes every reader rediscover which line holds which fact. One shared
table carries them all:

- The table sits in one box with a 1px level-2 border at radius 12. A **40px header row** on the
  layer-1 ground names every column in 12px medium secondary text.
- **Body rows are 52px**, 14/20 primary text, 16px from the box edges, divided by a 1px level-2
  hairline — no zebra stripes, and no hover fill, because a row is not itself a control. A slug,
  identifier or image reference takes the code face; a count is tabular.
- **Row actions** take the last column, right aligned, in compact §12 buttons. Its header is
  visually hidden but still read (*Actions*), so the column is announced without printing a word
  that adds nothing on screen.
- A row that has to say something about itself — a failure, an unconfirmed outcome, a retained
  draft — gets a **note row** directly beneath it, spanning every column with no divider between the
  two, so the note reads as part of its row rather than as the next record.
- **Loading** draws three skeleton rows in the table's own columns, with one polite status naming
  what is loading. **Empty** keeps the headers and says so in one row spanning every column, with the
  view's create action when the caller may create. A failed refresh keeps the last rows and reports
  in an alert above the table.

A table never widens the page. Its box scrolls horizontally inside itself; below 48rem the table
keeps a 40rem minimum width so its columns do not crush into one-word lines, and the box is a
named, keyboard-focusable region so that scrolling does not need a pointer.

---

## 13. The Agent conversation surface

An Agent owns one conversation in one Worktree, Workspace, and Sandbox. There is
no separate Chat surface. The fixed header names the Agent and its Workspace and
Sandbox ancestry. The history and the composer share one centred reading axis, at most 768px wide
including the avatar column, like a desktop agent client's reading column: a reply
stays at a comfortable measure however wide the screen is. The cap is in pixels, so the
reader's text-size preference scales the text and not the measure. Below it the axis
keeps at least 24px of side gutter. Container queries measure the space
remaining beside the sidebar: at 48rem or less, use 24px side gutters; at 30rem
or less, use 14px. History, lifecycle sections and the composer share these
gutters. Rows are neutral flow items; the composer has a stable seat below the scrollport.

The owner's message is right-aligned on the existing accent wash, capped at the
smaller of 525px and its available message column. The Agent message is unboxed at 16/28.
Use the existing markdown parser and theme tokens. Tables and code scroll within
their own bounds; long words and links cannot widen the reading axis.
Inline code is quiet, because replies name many identifiers: the code font at
0.875em, 1px × 4px padding, radius 4 on the `markdown-inline-code` ground, with no
border. Code blocks keep their framed ground.

A run of consecutive messages from one author shows its identity once. The first
message of a run has a 32px circular avatar beside its reading column,
separated by 12px, with a 1px border-l2 edge. User avatars sit at the right; Agent avatars at the left. The
name uses caption ink at 12/18, weight 600, with 4px before the content. A message
whose previous message has the same role continues the run: it keeps the empty
32px avatar column, so its text stays aligned, and draws neither avatar nor name.
Tool calls, plans, reasoning, approvals and notices between them do not break a
run, because they are the same author at work. The message keeps its accessible
name either way. The Agent
name is always **Chrono Agent**, and its avatar is the CMA mark (§21) drawn inline as SVG in
`brand-primary` ink on a `bg-layer-2` circle, so it follows the appearance with no raster ground
of its own; message rows do not start WebGL scenes. Personal user
messages use the current signed-in display name and avatar, falling back to the
account handle and then **You**. Missing, unsafe or failed avatar images show
centered uppercase initials at 12/18, weight 600, on the avatar fallback aliases.
Only HTTPS avatar URLs and local absolute paths are rendered, without a referrer.
Names wrap instead of widening the message column. Avatars are decorative beside
the visible name. Durable, streaming and optimistic messages use the same layout
and identity. Shared messages without author identity say **Participant**, never
the viewer's name. Identity rendering does not alter message text or attribution.

An explicit Send or quick-message action immediately adds its text to the
owner's message flow before connection or network checks finish. Preserve line
breaks and use the existing caption label token at 12/18 for **Sending…**.
On valid Send, immediately hand the visible text and ready attachment records to
that outgoing row and show an empty, editable composer. Retain the original draft
snapshot until authorized acceptance; only then show **Sent** and discard that
snapshot. A refusal or uncertain outcome restores untouched input and ready
attachments without uploading again. New typing belongs to the next draft and
must never be cleared or replaced by a late callback, even when its text is
identical. Its earlier outgoing row still retains the submitted text and exact
file references. Uncertain delivery shows **Awaiting confirmation**; a cancelled
connection check or authoritative admission refusal shows **Not sent**. These
local rows never submit or retry an input. Quick messages preserve the current draft.
Reconcile with durable user messages by command or run identity, never text,
so intentional identical messages remain separate. Account, Agent and access
changes invalidate both local rows and late draft-clearing callbacks. Retained
pending requests keep the existing explicit same-request recovery action.

One active Thinking disclosure follows the current Agent reply and settled turn
activity inside the conversation scrollport, before queued follow-up messages.
Its left edge aligns with the Agent avatar and message row's left edge, without
the message body's avatar-and-gap inset. Before the first reply, it uses the same alignment.
It is transient turn status, never a persisted message or an extra transcript row.
Its 44px control
places the turn's 32px character (§28) before Thinking, posed by what the turn
is doing now — working while a tool is pending, replying while reply text
streams, thinking otherwise — followed by elapsed seconds and a single-line
current-activity preview. Clicking
or keyboard activation opens the existing 120px capped body, containing current
live reasoning and only pending or running tool calls. The character follows
the shared visibility, static fallback and reduced-motion behavior in §28.
The elapsed figure comes from the recorded active turn's start timestamp, so
navigation, reload and reconnect do not reset it. A new turn uses its own start;
an unavailable timestamp leaves the duration unstated rather than inventing zero.
Thinking leaves the conversation when the turn finishes, fails or stops. Reasoning
is live-only and never presented as recovered history. Appearance and disappearance
fade and expand/collapse over the shared 200ms duration; the exit is immediately
inert and leaves no spacer after removal. Recorded reply growth may ease the status
row's measured vertical movement over the same duration without animating message
text. Reduced motion skips these transitions and cancels active motion. The
existing follow-output behavior remains authoritative: reviewing older history
never forces the reader back to the status row. Elapsed time, current activity and
turn token reporting retain their existing sources and do not reset on relocation.
On narrow screens the token line wraps within the message content width so actual
counts and explicitly labelled estimates remain readable.

Tool activity is one quiet line in the timeline, not a box: no border, ground
or radius. It reads in 13/20 secondary ink, so the replies around it lead.
Each settled tool call is a row inserted at its completion position and retained
on reload, named **Tool call: <title>**. Rows use 8px vertical padding; a tool
line is 28px tall with 12px after it, and a message leaves 8px below its footer. The
row leads with a state dot and pairs it with a plain word: **In progress** (the
pinging dot, still under reduced motion), **Completed** (done dot), **Failed**
(error dot), **Cancelled** (no dot) for a call the provider reports as cancelled
or stopped, and **Outcome not reported (last status: <status>)** (neutral dot)
for a call left pending by an ended turn, without inventing success or failure.
The recorded title and kind wrap beside the word. A duration appears only when
the public contract reports one; it reports none today, so no row carries a
duration or a "not reported" filler, and unknown duration is never zero. Tool
activity never invents missing arguments or results: the recorded detail sits
behind a bounded **Details** disclosure only when the contract supplies it.

Two or more consecutive tool calls, with no message, approval, plan or notice
between them, form one group. Its header is a text button with `aria-expanded`:
a chevron and **<n> tool calls**. While a call in the group runs, the header adds
**· Running <title>**, truncated with an ellipsis; when calls failed it adds
**· <n> failed** in error ink. A settled group repeats no command line. The
header has the hover ground only on hover. A collapsed group hides its rows; an
expanded one lists every row with the same dot, word and disclosure, indented
past the chevron against a 1px border-l1 rule on its left. A group
containing a failure starts expanded, and stays so until the reader collapses it;
otherwise it starts collapsed. A single call stays a single row. Live and
replayed activity reconcile by call identity without duplicate rows or
regression to pending after completion.

A plan the Agent reports through the public stream (`plan_set` in the durable
history and Responses v2, the `/plan` state on AG-UI) renders as a **Plan** card
in the timeline: an eyebrow "Plan" and one row per entry, its title wrapping
beside a status badge. The badge reads **Done** (neutral), **In progress**
(active) or **Waiting** (muted); an unfamiliar status is shown verbatim, muted,
rather than mapped to a guess. Each turn has at most one card, placed where that
turn first reported a plan and replaced in place by the turn's latest report,
because a provider restates the whole plan on every change. Plans render when
the public stream carries them; history read from the transcript alone has no
plan, and no card is invented for it.

An approval is a card in the timeline at the point of interruption, on the
layer-1 ground with a warning-toned hairline and radius 8. While it waits on a
person, a 40px approval character (§28) leads the request; a decided card has
none. An eyebrow reads
**Approval**; the request's action reason is the heading and its detail the
body. Below them sit **Deny**, **Abort** and the primary **Allow**, each filling its equal column over a
one-line caption that states its effect: Deny refuses this request and lets the
Agent continue another way; Abort cancels the request without an answer; Allow
lets the Agent go ahead. The captions describe the buttons, they do not rename
them. The buttons hold while an approval command is pending or input is not
allowed. A viewer without authority reads **Waiting for an authorized
participant to answer this approval.** in place of the buttons. A resolved
approval shows its recorded decision instead.

AG-UI identifies the thread by Agent id and the run by durable turn sequence.
Live content can arrive without a preceding START frame. It must be visible
immediately and replaced, not duplicated, when the durable message arrives.
Ephemeral frames never advance the durable replay cursor. Reconnect reconciles
history without losing an unsubmitted draft or replaying already committed text.

If the authorized Agent, transcript, usage and Workspace reads succeed but an
event-history page is temporarily unavailable, keep the known conversation
visible. The existing status band names incomplete activity and automatic retry;
it is not complete-history acceptance. Keep ordinary message input available once
current access, identity, connections and Sandbox eligibility are verified. Hold
approval actions and history-dependent turn controls until their recorded targets
are available. Retry
from the start so missing tool and approval pages are not skipped. Authentication
or permission denials and malformed identities remain explicit failures.

Show the current authorized transcript as soon as it arrives, independently of
usage and rich event pages. Rebuild older activity offscreen; keep newer live
messages and the draft when hydration completes. History-dependent controls
remain held. The snapshot's sequence does not skip activity pages.

For new Agent tabs, the authenticated transport eligibility selects Responses v2
input with independent rich Agent snapshot/tail observation. Retain the tab's
selection through reloads and flag changes; existing AG-UI tabs keep their
transport. Preserve an uncertain message or response action with its exact key,
body and original target. Use the existing status band and compact Button for
**Recover pending request**; recovery is explicit and never retargets an action.
Keep the draft editable; the immediate visual handoff retains its original
snapshot until matching acceptance and restores untouched input on uncertainty.
A refused intent may be corrected after an authoritative admission refusal.

Responses v2 may load Agent/Workspace details, usage and the first activity page
concurrently. Publish identity-checked Agent/Workspace metadata after a fresh
account check to enable ordinary input, then the latest transcript as soon as it
arrives. Send still requires current Sandbox state and required connections; an
early activity page alone does not authorize interaction. Rich history completes
independently with its own fresh authorization check.

Once a retained creation operation names its actual Agent, its modal may prepare
that exact Agent's transport and owner-authorized connection check while lineage
is checked. Transfer both once to the same Agent, owner and client after personal
access is verified. Transfer the same v2
selection once into navigation, keeping the loader's fresh account check and the
existing persisted transport choice. Closing or changing the creation scope
cancels preparation; account and local sign-in epoch changes invalidate it. A
reload uses the normal freshly authorized opening path.

After current personal access is verified for an exact Agent, check required
connections while its conversation loads. Keep the result bound to that Agent,
viewer, client and uninterrupted authorization generation. Refused or uncertain
access discards the result and cancels pending checks. Send still waits for
current Sandbox eligibility. A v2 send may consume the creation's
transferred ready connection report once, within one second of receiving it on a
monotonic clock. Opening settings, a known grant/profile change, loss of focus or
visibility, or a changed authorization scope discards this handoff. Ordinary,
expired and subsequent sends freshly check connections before submitting the
captured draft; missing connections keep the existing recovery dialog. Every send
still freshly checks account identity and the server independently authorizes
admission and execution, including required connections before dispatch. No extra
preparation notice is introduced.

The same current personal access read may supply immutable Agent, Workspace and
Sandbox IDs to overlap fresh reads. Load the Workspace beside the Agent after
the normal v2 entry identity check, then verify both fresh projections against
those IDs before the final account check and any publication. Observe the parent
Sandbox while history loads; that observation admits input only with current
identity-checked Agent metadata and required connections. Owner/client/access changes,
visibility loss, observation expiry and newly discovered bootstrap authority
retain their existing invalidation and recheck behavior. A changed lineage
requires a new load and cannot reuse the previously loaded view.

A newly admitted Agent may open while its recorded state is **Starting**, after
verified Agent/Worktree/Workspace/Sandbox lineage and a fresh Active parent without
an operation or save in progress. The server may also explicitly prove the exact
creation operation's bootstrap lineage while its owned parent is Provisioning or
Installing. That proof permits opening the actual Starting Agent and durably
queuing v2 input before the runtime is ready. Match the creation receipt when
opening from its modal; before enabling bootstrap input, freshly read both the
exact Agent and parent, match the current owner and all lineage identifiers, and
require no parent operation, checkpoint or checkpoint refusal. A missing,
withdrawn or mismatched proof holds input. AG-UI does not use this allowance.
An unresolved Worktree retains its empty commit and actual creation state;
queued acceptance never claims runtime readiness. Keep the actual Starting label and recorded
preparation visible. Responses v2 tabs offer the normal editable composer,
**Message Agent** placeholder and **Send message** without an extra Starting or
sending notification in the composer. Hand off its visible draft immediately and
retain the snapshot until durable admission; the creation first
message keeps its earlier queue position. Existing AG-UI tabs wait for readiness.
Missing/stale parent state, access uncertainty, saving or lifecycle changes hold
Starting input and its explicit uncertain-request recovery.

Early navigation retains the owner-scoped creation request and follows its
operation on the Agent page until confirmed success. If a worker restart requires
connections again, reuse the status band and compact **Continue Agent setup**
Button, then the existing connection dialog. A successful check resumes the same
request; navigation and reload alone never resubmit it. A read failure offers
**Retry setup status** without replacing the conversation or draft. Viewer,
client or Agent changes cancel pending setup reads and connection callbacks.

For the latest locally accepted message, a fresh public observation may offer
**Cancel queued message** before execution starts. Reuse the existing status
band and compact Button, with a bounded preview identifying that message. Keep
queued cancellation separate from Stop response. Retain its original message,
source revision and key before submission; **Recover cancellation** repeats only
that exact intent after uncertainty. A completed cancellation receipt confirms
that the message was cancelled before starting. Account changes clear this view;
incomplete history and read-only access hold its actions. The draft and any other
active response stay visible while status observation and cancellation proceed.

Responses v2 tabs offer **Queue message** beside **Stop response** while a turn
is running, as two circular buttons at the right end of the composer's control
row. The steering chip beside the permission chip says whether that button
queues or interrupts; it appears only while a turn runs, because outside a turn
both modes send ordinary input. Enter
and Trigger
quick messages admit ordinary queued input; they do not steer the running turn.
Keep the next draft editable while retaining the submitted snapshot until
admission is confirmed. An uncertain
earlier input holds another queue submission until explicit recovery. Existing
AG-UI tabs retain their current composer behavior during protocol migration.

Read accepted action receipts to show queued, delivered or failed delivery in
the existing status band. Delivery does not finish the displayed response; only
its recorded outcome does. A turn lacking a safe response binding keeps history
readable and disables response controls with an explanatory title. Account or
local sign-in epoch changes clear the prior view before any new-account read is
shown. A live gap discards uncommitted partial text and Thinking while preserving
recorded messages, settled tools and the turn's original start time.

The composer is **one rounded box**: radius 16, a 1px `border-l2` hairline and the
level-1 shadow, on the floating-card ground (`--dsw-specific-menu`: white in light,
layer 2 in dark). Focus within the box draws its hairline in the accent. The draft
sits on top at 16/24 with the placeholder **Message Agent**; it starts at one line,
grows with its content up to the smaller of 336px and 40dvh, then scrolls inside
the box. The box grows upward from a stable bottom edge and never overlays history.
Attachment thumbnails and upload records sit above the draft. Initial conversation
and composer focus must not scroll the frame while its columns animate.

The **control row** is inside the box along its bottom edge, with 32px controls
(44px below 30rem) and 4px gaps. On the left: **Attach files** as a + icon button
with its paste/drop hint in the title, the permission chip, and the steering chip
while a turn runs. On the right: the model and effort chip, the context ring, and
the Send (or Queue/Interrupt) and Stop circles. There is no settings row and no
usage row below the box. A chip is quiet: 12/18 secondary ink, no border, radius 8,
an interactive hover wash, a leading icon or trailing chevron. Send is a 28px
circle answering a 32px square (44px below 30rem): a neutral `bg-layer-3` ground with dimmed ink while
there is nothing to send, the primary fill once the draft is ready. Below 30rem
the permission and steering chips show only their icons and keep their names as
accessible names; while a turn runs there the row wraps, putting the model chip,
ring and buttons on a second line.

The **permission chip** is a shield and **Guarded** or **Autonomous**, named
*Permission mode*; it opens the shared dropdown (§12.3) with those two rows. The
**steering chip** shows **Queue** or **Interrupt** and opens the shared dropdown
with each mode's reason. The **model and effort chip** reads `<model> <Effort> ⌄`
(the catalog label, else the model id) and is named *Model and reasoning effort*
with both values. It opens a popover card drawn on the body like a dropdown list
(§12.3): radius 12, the menu fill, an inverted hairline, the level-3 shadow,
256px wide and nudged inside the viewport, above the chip. The card holds:

- the current effort word (Minimal, Low, Medium, High, Very high) as its heading,
  14/20 semibold in the accent ink, and at the top right a 24px reset icon button answering a 32px square,
  **Reset to current settings**, which returns model and effort to the Agent's
  confirmed settings and is unavailable when they already match;
- the model name with a `›` chevron, named *Model*, opening the shared dropdown of
  the model catalog with each unavailable row keeping its reason; catalog loading,
  failure (with **Retry models**), emptiness and runtime-capability notices sit
  under it in 12/18 caption ink;
- a discrete slider named *Reasoning effort*: a 4px rounded `bg-layer-3` track with a
  6px dot per stop, filled in the accent up to a 16px round thumb on the
  menu fill with an accent edge. It is operated by pointer or with the arrow keys,
  Page Up/Down, Home and End, and announces the effort word as its value text. The
  stops are the effort levels the settings contract accepts; the model catalog
  reports no narrower set per model.

Escape or a press outside closes the card and returns focus to the chip; Escape
inside the open model list closes only that list.

Choosing a model, effort or permission changes a **draft**: the explicit Apply
rule, turn-idle and runtime-capability gating, and same-request recovery are
unchanged, and changes never submit conversation input. A settings status line
appears inside the box under the control row only when there is something to
say, in 12/18 caption ink: **Apply settings** or **Retry settings update** as a
compact button, *Applying settings…*, *Waiting for the runtime to confirm these
settings.*, *Settings can be changed after the current turn finishes.*, an error,
or the transient *Settings applied to this Agent.* (three seconds). While an
update is pending or retained the chips stay readable but their controls hold.

The **context ring** is a 20px ring in a 32px target (44px below 30rem), named
*Context usage*. A 2.5px neutral `border-l2` track carries an accent arc drawn to
scale of context used against the reported limit; without both figures the ring
draws only its track, never an empty arc that reads as zero. Hover or keyboard
focus opens a tooltip above it on the tooltip ground with inverted ink, 12/18,
radius 8 and 8px by 10px padding, listing **Context** `<used> / <limit>` (or the
used figure alone, or *not reported*), **Total tokens** `<input + output>` with
`<input> input · <output> output`, and **This turn** with its input and output
when the current turn reports usage. Figures are tabular; a missing figure reads
*not reported*, never zero. Escape dismisses the tooltip without moving focus.

Pasted screenshots use the clipboard's file items when its file list is empty;
plain text paste retains normal editing behavior. A confirmed message includes
both the text and the exact uploaded attachment paths for that Agent.
Stop interrupts the current turn; it does not delete the Agent.
Approval decisions are explicit at the point of interruption. Only public API
operations appear as commands.

Enter confirms an active input-method composition without submitting or clearing
the draft. Track composition lifecycle as well as native composition flags and
the IME key code used by Safari. Ordinary Enter sends after composition ends;
Shift+Enter inserts a newline. Composition completion alone never sends.

The **Agent header** is one compact line on the base ground with a bottom hairline:
the breadcrumb's Sandbox and Workspace links, then the title as the view's `h1` at
14/20 semibold, its status badge and the branch chip, and at the end a Pin toggle
and *More Agent actions* (§11), 32px controls with 8px 20px padding. It is
**collapsed by default** to a 28px strip that shows only the title in caption ink.
Hovering the strip or moving keyboard focus into it reveals the full line as an
overlay above the conversation, on the same ground with the level-1 shadow, so
the conversation never moves; it fades over the shared fast duration and stays
while the pointer, focus or an open menu is inside it. The heading stays in the
document and focusable for the surface's focus-on-open behaviour, and Tab reaches
the breadcrumb links and actions in the collapsed strip, which reveals it.
**Pin header** keeps the full line in flow; **Unpin header** collapses it again.
The choice is remembered per viewer in the browser's local storage; a storage
read or write that fails leaves the header unpinned and never breaks the view.

The More Agent actions menu labels immutable creation as Created manually, Created
by Trigger, Created by channel bridge, or Creation origin unknown, in a heading row
after the Details toggle. A missing or refused read says Creation origin
unavailable. A resolvable published Trigger appears separately as a link row
labeled **Published Trigger: <title>**; later Trigger reuse cannot relabel an
Agent. A subscription Agent offers **Sign in to your ChatGPT subscription again**
in the same menu. Source loading or failure does not block history or ordinary
messages. Do not substitute internal IDs for missing display names.

An unframed, wrapping row directly above the composer contains up to eight
publisher-defined quick-message buttons. Use existing compact outline buttons,
8px gaps and 44px mobile targets. Names wrap without widening the conversation.
The row has its own scrollport, bounded to the smaller of 144px and 24vh, so
eight long names cannot consume the conversation or displace the input.
Keep 4px inset around the scrollport for focus rings and 8px before the input.
Hover and keyboard focus show a description tooltip above the row, aligned to
its leading edge, bounded to 320px wide and 160px tall with its own scroll. It uses
the existing layer-2 ground, secondary ink, 12/18 type, 12px padding, 6px radius
and border-l2. Portal it outside the row scrollport using the existing menu
placement helper, 12px viewport inset and overlay level 1100. A 150ms hover-exit
delay lets the pointer reach and scroll a long description; Escape dismisses it
immediately without moving focus. Clicking
sends the configured text through ordinary message admission, preserving any
typed draft. The same availability and pending-request guards apply; active turns
allow ordinary queue submission only in Responses v2 tabs. Reads,
reloads and publication never send a message. Quick messages use the latest
published Trigger revision; original creation provenance and first message stay
unchanged. Retired Triggers expose provenance but no new quick-message actions.

### Preparation, pause, and recovery

While the New agent request is in flight, show **Starting your Agent…**. Show
same-request recovery wording only after the request has stopped waiting without
a receipt; a healthy admission must not flash a recovery instruction.

Preparation has stable step rows from Sandbox events and current instance facts:
software installation, Workspace preparation, Worktree creation, Agent startup,
and optional first-message delivery. Never infer successful work from elapsed time
or from an identifier existing. A failed step shows typed remediation and offers
only actions the server supports.

A paused Sandbox leaves every Agent's history visible. Render the server's
`paused_notice` verbatim above the composer. The enabled composer's placeholder
is **Send a message to continue.** Sending is the ordinary admission operation
and requests revive; no extra Revive click is required.

While admission is unresolved, retain the exact draft and show Starting without
inventing token progress. A readiness timeout keeps the draft available. Runtime
replacement is a quiet status notice, not an automatic failed state. When a
checkpoint boundary is known, show its recorded time without claiming later work
was captured. The owner never has to recreate the Agent after a rollout.

An unexpected interruption has a concise recovery notice in the Agent conversation
and Sandbox page. Only an explicit recorded out-of-memory cause names the configured
memory. Unknown causes display **Internal error** without inventing a more specific
diagnosis; the current recorded stage separately shows recovery progress.
Historical interruption text never claims recovery is still running.
Match the interruption time to the
current operation before attributing a new recovery to an earlier interruption.
The shared 44px character animates beside the actual recorded stage, with the
existing visibility and reduced-motion behavior. Use layer-2 fill, a layer-2
border, 12px radius and padding, a 12px gap and the existing 14px body voice.
Stage text fades with the existing transition duration; it never advances on a
timer. Entrance and exit use Presence. Failed recovery shows the recorded
remediation and keeps history and draft editing available. Only an observed
recovery-to-active transition shows a dismissible ready notice, which expires
after eight seconds and is not replayed on navigation. The last interruption and
its time remain in a closed Sandbox details disclosure. Saved-home stages name
verification and reconnection, without inventing downloads or file percentages.

The Agent conversation independently reads its exact public Sandbox while open,
using the settled five-second and saving 1.5-second refresh cadence from §16.
Returning from a hidden tab requires a new foreground observation; an otherwise
allowed observation expires after ten seconds without a successful refresh.
Actual saving phases and their recorded start time use the existing composer
notice. While saving is known, or lifecycle information is missing or stale,
hold Send and approval decisions without disabling draft editing or hiding
history. Pause eligibility is not message eligibility; a normally paused Sandbox
still resumes through the ordinary message command. No retry sends the draft.

An unresolved message admission shows Starting with a disabled Send control in
its existing circular seat. It is not an active Agent turn and exposes no Stop
response command. Stop is available only after an active turn is recorded, and
remains available during saving or failed Sandbox reads for a known active turn.

### Details and readings

The Details drawer starts with immutable creation origin and timestamp in the
existing fact layout. Authorized Workspace Profile provisioning is a separate
fact, including when creation category is unknown. Closing or changing the view
clears optional provenance until it is authorized again.

The drawer also contains Sandbox status, pinned profile, size and services;
Workspace path, source and resolved commit; Worktree path and branch; Agent
Profile model, effort, permission and skills; first-message state; and the
separately confirmed Stop/Delete controls. No credential, provider token, private
runtime authority, or internal checkpoint writer identifier is displayed.

**Service access** in the drawer is read-only. The NyxID key belongs to the
Sandbox, not to one Agent, so it is edited in one place, the Sandbox page's
Access tab (§17). The drawer lists the effective services as plain rows of
catalog names, with chrono-llm-public and ornn-api followed by the word Required
in the tertiary label colour, and ends with **Manage on Sandbox**, a real link to
`/sandboxes/<id>?tab=access`. **Sandbox operation progress** opens the same
page's Activity tab. How the panel is laid out is specified under Agent details
panel below.

Usage and context are read from the composer's context ring and its tooltip, with
tabular figures. Missing usage is not zero. Without a reported ceiling, the tooltip
shows only the known figure. Context-summary boundaries use neutral metadata, not
failure colour.

### Organization inventory and shared Agent views

The account menu's **Agents** entry opens `/agents`; an organization selection
uses `/organizations/{organization}/agents` so reload and deep links retain the
scope. Named organization choices use the existing Picker. The inventory keeps
creation and lifecycle status as separate filters, includes retained Agents,
and shows title, recorded status, immutable creation category and creation date.
Rows open the canonical Agent route. Previous/Next actions operate on fresh
pages; an empty filtered page with continuation explicitly invites the next page.
The **Export this page** action rechecks access and downloads only that page's
safe JSON summary fields. It cancels on changed scope or client. A completed
download cannot be recalled by later revocation.

Use the existing surface, border, font, Button and Picker tokens. The inventory
filters wrap, and its four-column rows become two columns on narrow screens.
Do not show private source IDs, creators, parent identities or profile controls
in these rows. An organization-choice outage has its own retry and leaves the
personal inventory independently usable.

Shared Agent routes have a safe title/status/creation header and Agent history.
They never start the personal controller, parent/profile loaders, connection
wizard or runtime readings. Pending approvals remain readable without action
buttons for a viewer. The composer, response interruption and approval buttons
follow current effective capabilities. Commands use the current public SDK
principal; an organization admin's message is attributed to that admin.
Personal controllers retain their draft and stream while the access-classification
read refreshes. A real refusal clears the selected shared/denied view, not the
account's independent personal work.

Scoped results refresh every 30 seconds and expire at the earlier server deadline
or 55 seconds from request start, even while another refresh is pending. A read
that exceeds 15 seconds loses its display authority. Foreground, scope/client
changes and denied access clear old shared values and cancel streams/exports.
Channel-use presentation remains subsequent work.

The sidebar header Picker chooses Personal, a named Organization or Shared
with me without changing the current conversation. Keep selection through sidebar
collapse and
preserve the personal composer/draft during organization browsing and access
loss. Organization and shared Sandboxes page independently; expanding a named Sandbox or
Workspace reads that authorized branch. The Agent inventory link includes narrow
Agent grants whose parents are not visible. Do not fetch hidden parent metadata
to fill out the tree.

Shared with me uses the aggregate Sandbox endpoint for parent branches and the
shared Agent inventory for flat Agent discovery. These lists page and refresh
independently. Empty Sandbox results explain where individually shared Agents
appear. A parent branch is never inferred from a shared Agent. Switching scopes
cancels pending branch reads and clears descendant labels; refreshing shared
access preserves the active personal composer, its DOM and its draft.

Shared Sandbox and Workspace details use safe names, status, child counts and
canonical Agent links. They expose no profile, connection, path, runtime or file
content. Refused child reads invalidate parent branch data. Scope/client changes,
foreground and bounded authority expiry clear stale labels and stop child reads.
Keep personal lifecycle pages behind their own accessible-resource classification.

### Owner sharing

The existing **Sharing** button in personal Agent Details, and the **Sharing** row
of the Sandbox page's More actions, appear only after current effective permission allows sharing management. Use
the existing Modal, Picker and Button primitives. The dialog starts on the Agent
when opened from Agent Details. Whole-Sandbox scope is a separate explicit
selection; creation requires acknowledging its current and future children.

Use named current organization choices and explain **View only** versus **Use
organization roles** before submission. Agent grants provide no implicit parent,
sibling or file exposure. Whole-Sandbox previews include children, scoped files
and the extra admin controls permitted by organization-role mode. A server
refusal for unproved Agent execution isolation explains view access and the
explicit whole-Sandbox option. It never changes scope or moves the Agent.

Direct grants show their current mode, an editable choice and an in-place revoke
confirmation. Explain that other grants may still provide access. Refresh after
changes and conflicts; a newer revision needs another explicit action. Unconfirmed
commands retain their request identity while the resource view remains mounted.
Organization lookup failure uses an unavailable-name label and still permits
independently authorized revocation. No internal organization, actor or grant IDs
are primary copy. Share changes preserve personal draft/controller state and
creation labels. The dialog follows current permission, expiry and context
cancellation and keeps every control within phone and desktop viewports.

## 13a. Agent resources

Machine resources are public point reads scoped to the Agent's Sandbox and
appear only in the Sandbox page's Overview tab (§17), never below the conversation
or in Agent Details. The read goes through one of the Sandbox's Agents; a Sandbox
without an Agent says so instead of showing figures. RAM, CPU and
network share a three-column row. The already observed Sandbox lifecycle gates
samples: only a fresh Active state with a runtime version and no operation or
checkpoint permits reads. Paused and unavailable runtimes show written states,
never zeroes. Cancel requests and reset the CPU sample pair when the runtime
identity changes. Mounting a conversation does not start machine-resource polls.

The conversation footer contains Context quota and cumulative Agent input/output
tokens, including earlier turns and preserved across compaction. The Thinking
row shows the current turn's input/output totals. Unreported input says unavailable.
Before a token report arrives, live visible reply and reasoning text may show
an explicitly labeled `≈ N visible output tokens` estimate using a local
o200k_base tokenizer. Its explanation excludes hidden reasoning, tool calls and
inputs, and states that actual model usage may differ. Load the tokenizer only
for an active turn, in a background worker; update at most five times per second,
reuse unchanged text chunks, and cap counted text at 65,536 UTF-16 code units with
a visible partial label. Missing text, unavailable tokenization and missing provider counts have
written states. Estimates are never added to session totals or retained as actual
turn usage. An authoritative turn report replaces its estimate and records the
visible text boundary it covers. Later model calls in the same turn display
`+ ≈ N newly visible output tokens` beside the last reported counts, counting
only new visible text past that boundary. Repeated identical reports do not
advance the boundary; restored text does not count twice. Missing counts
are never displayed as zero or inferred from elapsed time or context quota. Each reported
increase animates towards that actual value over 600ms, never beyond it. Values
use tabular numerals; reduced motion and hidden documents settle immediately.
Accessible labels expose the actual count without announcing every animation
frame. The cumulative footer uses the same observed-count animation. A new turn
resets only its own counters.

Each completed turn keeps its reported input/output counts below its final Agent
reply, in the existing 12/18 caption style, revealed with the message footer
(hover, keyboard focus, or always on touch devices). Earlier intermediate replies do not
repeat the same turn total. Full durable replay restores every available turn's
counts. A completed turn without a report draws no line at all: absence is still
never shown as zero, and the composer's context ring holds the totals.

Routine stream reconnection is quiet for 2.5 seconds; a continuing failure then
shows the existing connection status. If live fragments need refreshing, show
one neutral caption while recorded replies are restored. Fresh live observations
clear a reasoning-only refresh, and a recorded reply or turn completion clears
its reply refresh. A new turn clears the previous transient refresh. Confirmed
durable history gaps and authentication/terminal errors retain their separate
recovery messages.

Tracks use reported ceilings first, independently for each resource. When a
ceiling is absent, the exact pinned Sandbox Profile revision may supply a
configured capacity. Its id and revision must match, and its digest must match
when the pin includes one. That denominator visibly says "configured", including
the accessible numeric description; it is not a runtime observation. Missing,
forbidden, invalid or mismatched profile reads leave the honest absolute figure
without a track. Never substitute configured capacity into resource observations.

A missing grant, a runtime that cannot report, a reading not yet available, and
an absent individual measurement remain distinct written states. None becomes
zero. A first CPU rate sample without a predecessor says Measuring. Network
figures are unbounded and never receive a percentage track.

Pressure notices require an actual server observation. They preserve the history,
name the resource and its reported values, and use the existing alert treatment.
They never flash or imply the Agent's work has already failed.

---

## 13b. Public Trigger journey

The visitor routes are `/t/{publicId}`, `/t/{publicId}/{placementId}`, and
`/launches/{id}`; a ready result hands off to `/agents/{id}`. GET requests and
OAuth callbacks never start work. The only consent is the explicit CMA button
gesture, preserved as a bounded 15-minute browser intent across sign-in.

The public landing uses the reduced frame, publisher attribution, title,
description, server-authored next-step sentences, and the configured primary
button. It is a real entry into the user's own Agent, not a marketing page.
A Technical details disclosure carries provider, model, effort, size, services,
and the full installation disclosure. A retired target is a complete unavailable
view with no misleading launch button.

Registration copy comes from signup options and the publisher's optional hint.
Closed registration, an invitation requirement, and unavailable public
configuration are distinct. The return destination is a canonical local route;
no credential, prompt, provider state, or arbitrary URL enters it.

### Review and connections

An authenticated click submits `start_if_ready: true`. Accepted work opens
progress immediately. A review shows only the server's current needs, in order:
software trust, missing connection, publisher-required review, and repeat choice.
Trust exposes the commands and environment values verbatim, with the explicit
checkbox, and sends the matching digest as protocol data. Do not render that
protocol name in ordinary visitor copy.

A missing service has one Connect action. Open a blank popup directly from the
gesture, request the public connection handoff, and navigate only to its validated
HTTPS hosted URL. Poll every 1.5 seconds with backoff for at most five minutes.
CMA does not inspect cross-origin window contents. On satisfied, close the popup
and continue the same intent. Expiry offers another explicit attempt.

A pending connection keeps **Connect** and **Verify** available in the existing
wrapping action row. Connect reopens the current unexpired hosted link without
creating another handoff; after a same-tab return without that in-memory URL,
an explicit Connect may request a new link. Verify reads current status
immediately, disables itself while checking, and explains when the account is
still not connected. Late results from an aborted check cannot complete a newer
attempt. A blocked popup retains its same-tab destination. Services requiring
manual setup link to NyxID and offer a recheck; never claim they support hosted
setup. Do not request or display service credentials.

When connections are the only outstanding requirement, title the dialog
**Connect required accounts** and label its combined recheck **Verify connections**.
Do not describe this permission step as a new review or consent requirement.

Repeat choice offers **Continue where you left off**, with title, time and the
first-message excerpt, or **Start fresh**. The choice is an explicit server
command and never permission to reuse the publisher's Agent.

### Progress and vocabulary

One unframed ordered timeline shows Signing you in, Checking what's needed,
Preparing your sandbox, Setting up the workspace, Starting your agent, Sending
your first message, and Ready. Each row has a fixed icon slot, visible state word,
and safe detail. The state vocabulary is Waiting, In progress, Done, Needs you,
Paused by CMA, and Didn't work. Colour is never the only account.

The icon slot holds the §12.1 state dot the in-app setup rows use — done, the
live ping while In progress, warning for Needs you, error for Didn't work, and a
quiet dot while Waiting or Paused by CMA — not the 3D character, which stays on
the landing and the brand. The word beside it takes the warning or error ink when
the visitor must act or the step failed. The request's overall state under the
heading is a §12.1 badge that announces politely. A failure, and updates that
have stopped, are §12.1 alerts carrying their recovery actions. The words stay
the visitor vocabulary below, never a wire value.

Follow the owner-only Launch companion with bounded polling fallback. Retain the
last acknowledged durable cursor; live transport failure does not erase progress.
A ready result replaces the route with the Agent route and focuses its composer.
A failed result shows safe remediation, Refresh status for supported read
recovery, Start over as a new intent, and Copy support code. Retryability is not
a command: show no worker retry unless the public API supplies one.

Public sentences belong to the exhaustive vocabulary module. Ordinary visitor
and creation copy must not expose Launch, revision, digest, idempotency,
reconcile, retained, capability, ceiling, checkpoint, reach, fence, UserService,
MiB, millis, or snake_case wire tokens. Sandbox and workspace are allowed nouns.
Technical disclosures and verbatim user-authored commands remain data, not
translated or altered prose.

Use semantic headings and the shared dialogs. One polite live summary announces
progress; failures are announced once. On narrow viewports disclosures and steps
become one column, text wraps, and every action is available without hover.
A network outage preserves the last successful view and names its observation
time. No elapsed-time heuristic changes a server state.

During Sandbox preparation, the sandbox row also shows the public Sandbox's
current operation in plain language, total elapsed time from its recorded start,
and the last successful status check. The same unframed status band appears in
the Sandbox page's Activity tab. A native Activity details disclosure contains the ordered
operation stages and recorded start time, using existing 14/13px type, secondary
ink, level-2 separators and 16/12/8px spacing. Completed stages derive only from
the server's sequential phase contract. Unknown phases have no guessed steps.
Elapsed time never advances stages or supplies a percentage or ETA. Long file
restore explicitly says that file-level progress is not reported; a successful
status read is not described as proof of moving bytes. Reads are nonoverlapping,
visible-only every 1.5 seconds, and recover after transient failure. Authorization
or visibility denial clears the last observation. Raw runtime logs, credentials,
commands and paths never enter the activity summary.

Lifecycle screens also show **Live activity** outside Technical details: a keyboard-focusable
scroll region, 180px high and at most 30vh, with a 1px level-2 border, 6px radius,
12px padding, 8px row gap and existing 13/20 secondary text. Timestamp and message
use a two-column grid that wraps text; the region cannot widen or lengthen the
page. Only the latest 100 translated lifecycle entries are retained. New entries
follow the bottom only while the reader has not scrolled away. The adjacent
**Last synced** timestamp changes only after a successful event read catches up;
loading and reconnecting are explicit. No empty poll invents a log entry.
Read-only event polling is bounded, nonoverlapping and visible-only. Access denial
clears cached entries; transient failures retain them. Only allowlisted event and
step names are translated, never arbitrary payloads, failure details, paths,
commands or credentials. Launch, Sandbox and Agent pages share this treatment;
deletion dialogs use the same bounded rendering for recorded cleanup events.
In Agent conversations this progress follows the last message, inside the existing
reading column, so normal conversation following leaves current progress visible
beside the composer. The log itself is not a live region; the current-stage
summary remains the single polite progress announcement.

---

## 14. Retired terminal plane

There is no terminal plane in this release: no terminal renderer, theme picker,
plane-switch control, terminal resize behavior or xterm dependency. The former
terminal palette and transition contract are retired. Agent conversations follow
§13.

---

## 14a. Motion

One curve, `cubic-bezier(0.4, 0, 0.2, 1)`, and three durations, from the tokens. The references use
`.2,.7,.3,1`; the visible difference is too small to justify changing every transition, so the
existing curve stays deliberately. A hand-written `cubic-bezier` in a component sheet is a fourth
curve nobody chose, and the suite refuses it.

**Motion is the default and `reduce` removes it.** Never the other way round: `prefers-reduced-motion:
no-preference` matches only where somebody has explicitly expressed none, so a platform that does not
report the feature at all matches neither value and loses every transition. The suite refuses that
too.

**Nothing moves on hover**, and nothing is animated on the agent's own words. A pressed button
settles to 96% over the fast duration and returns on release (§29); under reduced motion it
does not move. A turn arriving may
fade; the text inside it does not animate as it is read.

### View and message continuity

Route changes fade the existing center surface over the shared slow duration
(300 ms). Returning to an already visited service fades its retained body with
the same timing. These transitions never remount working state, reset drafts or
scroll positions, request resource reloads, or postpone input. A newer route
cancels an unfinished transition. New optimistic outgoing messages and genuinely
live assistant replies fade once over the normal duration (200 ms); loaded
history, durable reconciliation, and subsequent streaming tokens do not replay
that arrival. Use opacity only so text remains sharp and layout stays stable.
Reduced motion disables arrivals and cancels an animation immediately if the
preference changes while it is running. Menu and modal exit frames release
interaction immediately; presentation must never delay an action.

### What motion is for here

Inside a working surface, motion reports a changed surface or ongoing work.
The signed-in sidebar's §21 mark is the only decorative brand animation in the
shell. Its geometry draws once at shell mount and its asset-defined lens highlight
loops quietly; its wrapper never bobs, turns or scales. Reduced motion shows
the finished mark at rest. The signed-out authentication gate uses a static
brand asset and keeps sign-in immediately usable. The Agent characters of §28
are not decoration: they move only to report work or a person being asked, and
are otherwise still.

**A surface that replaces another fades in.** Switching Agents changes the whole centre column, and
without a handover it reads as a different screen rather than the same screen showing something else.
One duration, no movement — a slide would imply a direction, and there is no direction between two
Agents.

**A surface that is waiting says so, and never claims to know more than it does.** A bar is allowed
**only** where it reflects steps the service actually reported — the Sandbox preparation bar does, and says
in its own file that it never advances on a timer, because a bar that moves while nothing happens
costs the reader their trust in every other bar. Where there are no steps to report there is no bar:
the idiom is the wave, or the reasoning row's sweep, which say _live_ without claiming progress.

A surface reading something it will draw uses **the shape of what is coming** — rows where rows will
be — rather than a sentence alone in an empty column. And it never says what it does not yet know: a
list still loading must not read "nothing here", because that is an answer, and it has not got one.

**Rows arriving fade in.** An Agent appearing in the tree, a turn arriving in the stream. Nothing
moves position: a row that slides in pushes everything under it, and a list that reflows while
somebody is reading it is worse than one that appears.

Notifications and dialog surfaces fade in and out using the existing 200ms
duration and shared easing curve. A dismissing notification retains its final
content only until the exit finishes, becomes inert and accessibility-hidden
immediately, and cannot dispatch another action. The semantic element and its
layout remain unchanged during the fade. Reappearance cancels stale removal;
reduced motion skips both transitions and any visual exit retention. Interrupted
menu, dialog and notice transitions reverse from the currently displayed opacity
without flashing back to a keyframe endpoint. A newly inserted surface starts
transparent through the shared starting style. Loading
skeletons, whole-page error states and conversation text retain their own existing
presentation rather than acquiring a notification wrapper.

An animation that loops has to say how it stops, and the suite checks that a sheet starting one
carries a `reduce` block.

---

## 15. Accessibility

- Required text contrast meets WCAG AA in **both** themes. Measured ratios and the restrictions on
  tertiary and dimmed paint are recorded in §2; neither may carry the sole small-text account of a
  fact.
- Semantic HTML before ARIA. Landmarks on every page.
- Every view is fully keyboard operable; dialogs trap and restore focus, and
  tree expansion has a named action independent of navigation.
- Never convey state through colour alone.
- Progress has one polite live summary. Decorative images and animation never
  carry the only account of state.
- Focus enters the primary task; entering a ready Agent focuses its composer.
- Minimum target 32px, 44px below 30rem.
- ⌘K (Ctrl+K) opens the command menu from anywhere and is ignored while an input method composes.
  It never takes the composer's `/` or Enter.

---

## 16. Resource states and actions

State comes from public Sandbox and Agent records. Pair each state glyph with a
visible word. An unavailable action names its reason where the user needs it.

The sidebar has independently collapsible **Live** and **Paused** Sandbox groups,
with owner-scoped persisted expansion and counts. Starting, resuming and pausing
Sandboxes stay in the Live group until a settled Paused observation. A durably
accepted Sandbox deletion leaves both active groups immediately. Its separate
**Deletion progress** disclosure keeps a named, keyboard-accessible progress
button until cleanup is confirmed; it does not expose child navigation.
Each group has a 32px disclosure header (44px on narrow screens), 8px internal
spacing, existing 13px secondary text, and a tabular count aligned to the right.
Separate groups by 16px. Each expanded group contains its own accessible tree.
Workspace children and expansion controls exist when the Workspace is Ready
inside an active, paused, pausing or reviving Sandbox; preparing and deleting
Workspace rows remain informational.

Sandbox state labels are Live, Starting, Resuming, Pausing, Paused, Deleting and
Deleted. A genuine failure is explicitly Needs attention, never a successful state.
Internal image/credential replacement is Resuming. Background capture does not
replace Live or Pausing. Agent labels are Starting, Ready, Working, Needs approval,
Paused, Stopped, Deleted and Needs attention; Workspace labels are Starting, Ready,
Deleting, Deleted and Needs attention. Done is only a completed operation step,
never a resource state. Header and sidebar use the same resource vocabulary.

| Resource/state | Working surface and permitted behavior |
|---|---|
| Sandbox provisioning | Recorded preparation and child progress; no invented percentage or success. |
| Sandbox active | Workspaces and Agents; New workspace within limits, Pause, confirmed Delete. |
| Sandbox paused | Retained Workspaces and Agent history remain navigable; the Sandbox page shows preserved-work information and the public Resume action. Existing durable message admission may revive a settled paused Sandbox. |
| Sandbox pausing, reviving or rotating | Durable history and recorded progress; no conflicting command or input that could be lost. |
| Agent starting | Recorded preparation; retain an unsent draft until readiness. |
| Agent ready or working | Conversation and composer; Interrupt only during active work; Stop. |
| Agent waiting approval | The actual approval question and decisions, never an unrelated revive prompt. |
| Agent paused | History and pause notice; follow the parent Sandbox's recorded revival. |
| Agent stopped or failed | Readable history and safe details; confirmed Delete when public state permits it. |

A transient read or transport error is not a resource failure. Unknown and
unreported remain unknown. Failed commands never destroy successful content.
Delete removes an Agent and its Worktree and requires the irreversible-action
safeguard; Stop is a distinct operation.

Paused, pausing and reviving Sandboxes keep retained Workspace and Agent navigation,
including the Workspaces tab of the Sandbox page. Child rows show the parent's
actual Pausing, Paused or Resuming state while preserving terminal child failures
and stops. Opening an Agent shows its existing conversation and history; pending
lifecycle work holds Send, approval decisions and runtime mutations without
discarding drafts. It never admits a message merely because history is visible.
Preparing, rotating and deleting Sandboxes keep their existing navigation limits.
An ordinary checkpoint save on an active Sandbox retains its readable children.
Resume availability still comes from the public lifecycle record. The Sandbox
page names the current state and the available history. Pause, Resume and deletion
show a brief state in the page's alert stack by default; their
plain-language stages, activity and elapsed time are in the page's **Activity**
tab, which reads nothing until it is first opened, and the state alert offers
**View progress** to open it. Agent Details includes a standard Button,
**Sandbox operation progress**, that opens this tab. Initial creation progress remains visible.
Raw phase names and other technical facts belong in Technical details.
No guessed readiness percentage.

Deletion uses the existing risk confirmation, followed in the same overlay seat
by the shared Modal and recorded progress treatment. Request acceptance is not
completion. The dialog stays until dismissed, separates confirmation from cleanup
inside a closed **Cleanup progress** disclosure, keeps failures visible outside it,
survives refresh with owner-scoped resource IDs and the same command
key, and follows accepted cleanup with read-only polling. Closing it does not cancel server work.
Uncertain writes offer same-key reconciliation; a failed status read never means
successful deletion. Sandbox retirement, Workspace removal and Agent deletion
with a removed Worktree are the terminal confirmations. Accepted Sandbox deletions
leave the working sidebar before cleanup completes; the deletion-progress entry
and retained dialog remain reachable. Terminal resources leave that progress entry;
retained history stays readable through its canonical URLs.
Deleting is written as Deleting, not Done. Workspace deletion accepts live child
Agents and durably stops them, removes their Agents and Worktrees, and then removes
the Workspace files. Its progress dialog reports those three stages followed by
Deletion confirmed, using refreshed public records and Sandbox event history.
Agent-only deletion still states its stop prerequisite. Conflicting actions remain disabled.

Pause and Resume use the public Sandbox lifecycle eligibility in both the
Sandbox page and its tree menu, never Active or Paused alone. Missing eligibility
or a failed refresh leaves those controls unavailable with a written reason.
The tree keeps its last successful hierarchy during refresh and transient read
failure. Authentication, permission or visibility denial clears cached hierarchy.
The selected Agent header, details and sidebar share one status projection. Newer
timestamped Agent status events may supersede a resource read; older history and
unversioned deltas cannot override a fresher resource observation. Newer matching
navigation records update status without reloading the transcript or losing drafts.
Message-submission progress stays beside the composer; it does not replace the
recorded Agent state in the header.
Normal owner or operator stops and resource deletion use the recorded terminal
state, not an Agent run failed notification. Genuine provider failures remain visible.
Visible Sandbox and hierarchy reads refresh every five seconds while settled,
and every 1.5 seconds while recorded lifecycle work is in progress; reads never
overlap or admit work. Hidden pages defer refresh until visible again.
Pause and Resume immediately show a polite submitting status in the existing
action-status seat, followed by request acceptance and observed completion as
separate facts. Accepted requests show a concise acknowledgement; the underlying
wait and the standard Button to the Sandbox's canonical progress page stay in a
closed **Operation progress** disclosure. Dismissing the acknowledgement keeps
background observation and the target's busy lock, and a later failure becomes
visible. Resume verifies the current owner's connections without opening a modal
when they are ready; missing connections open the existing connection wizard.
Unverifiable or changed-owner results keep the action held and show the error.
After command acceptance, the targeted Sandbox status and open deletion dialog
may poll every 250ms for ten seconds, then return to 1.5-second progress reads.
These reads never overlap; failed reads back off to five seconds and report
uncertainty. Timers never imply completion, and retries retain the original key.
Every poll refreshes the authorized inventory and mutable Sandbox subtrees.
Deleted Sandboxes are omitted before fetching their details or descendants,
including on initial load and explicit refresh. Their retained conversation
history remains available through direct Agent reads, independently of navigation.

An actual save reports **Saving progress** during capture and **Finishing save**
while writers resume within operation progress and Technical details, not the
resource state seat. The Technical details dialog includes the recorded start time;
there is no guessed percentage. Pause remains unavailable until the public
eligibility permits it. These fields govern no Workspace or Agent creation rule.
Tree menu reasons sit beneath their labels using the existing 13/20 voice and
secondary ink, wrap within the menu and remain readable when the action is disabled.
Written tree states also use secondary ink so small status text remains readable.

## 17. Sandbox and Agent details

The Sandbox page serves the ordinary owner: keeping an Agent environment healthy.
It starts with the §11 header: `Agent Manager / <Sandbox>`, the name with its
lifecycle badge (Live, Pausing, Resuming, Updating runtime, Needs attention,
Deleting) and its id as a chip. The header holds one primary action, **New agent**
(named *New agent in this Sandbox*); one secondary, **Pause** or **Resume**; and
**More actions** with **New workspace**, **Sharing** when current access allows
sharing management, **Technical details**, a separator and **Delete Sandbox**
last. Pause and Resume take their eligibility from the public lifecycle record,
never from the status alone. When either is unavailable its reason is one line of
13/20 secondary text under the header, which the button's description points at.
New agent's reason joins that line, and every unavailable menu row keeps its reason
beneath its label, such as *Wait for the current Sandbox operation to finish.*, *Agents
and Workspaces can start when the Sandbox is active.*, *Checking your service
connections.* or *Checking the current Sandbox status.*

One alert stack under the header holds every message about the Sandbox, each in
its §12.1 tone: the reconnecting warning (content stays), the recovery notice,
*Your sandbox needs attention.* with the reported remediation, and one information
line for a paused, pausing, resuming, updating, preparing or deleting Sandbox. When
the Activity tab has recorded progress to show, that line offers **View progress**,
which opens it. No paragraph repeats what the header badge already says.

Below the alerts an underlined tablist, drawn as in §19, offers **Overview**,
**Workspaces**, **Access** and **Activity**. Arrow keys move between tabs and select
them, Home and End go to the first and last, only the selected tab is in the tab
order, and each tab names the panel it controls, which is labelled by it. The
selected tab is the `tab` query of the Sandbox URL (`?tab=workspaces`), rewritten
in place so a refresh keeps it and Back does not step through tabs; Overview has no
query and an unknown value opens Overview, except that a Sandbox still being prepared
opens on Activity, where its progress is. Panels stay mounted while hidden, so an
unsaved scope selection or a connection check survives switching tabs; Activity
starts its event reads only when it is first opened.

- **Overview** holds the Runtime card, Machine resources (§13a) and an
  **Environment** list: Owner, Model access, Workspaces as a count of the limit, and
  Agents per workspace. Model access comes from the exact pinned settings; when they
  cannot be read the list says so and offers retry.
- **Workspaces** holds **New workspace** and a table with Workspace, Path, Agents and
  an actions column. The Workspace name links to its Workspace page; Agents are links
  with their status badges, labelled by the shared Agent title selector, and once read
  show their model and Guarded or Autonomous permission beneath. **New agent
  here** starts an Agent in that Workspace and is available only when the Workspace is
  Ready inside an Active Sandbox with no operation. Paused, pausing, resuming and
  rotating parents keep the rows and links readable; children show the parent's state.
  Empty states read *No workspaces* and *No agents*. On a phone the table scrolls
  inside its own box rather than widening the page.
- **Access** holds the NyxID service scope editor, *These services are shared by all
  Agents in this Sandbox.*, with chrono-llm-public and ornn-api locked as Required and
  the other services marked Already granted or Requested. The save path is the public
  Agent service-access command of one Agent in the Sandbox, which is notified when the
  key changes: the Agent holding a retained unsent or unconfirmed update first, then a
  running Agent, then any Agent. Same-key retries, connection prompts and the paused
  note are the editor's own. Without an Agent to carry the command, the tab lists the
  effective services read-only and says that adding an Agent enables changes.
  Effective reach comes from the public Sandbox service list, including explicit owner
  grants, with public Agent lineage as an older-server fallback. Requested profile
  scope never substitutes for effective reach; unconfirmed access is explicitly
  unconfirmed.
- **Activity** holds the recorded operation progress, the Live activity feed and the
  last recorded interruption.

Optional metadata failures preserve the header, the alerts and the actions and offer
retry. Loading draws a skeleton in the page's own gutter.

**Technical details** opens the shared wide Modal of the same name. IDs, limits,
paths, recorded preparation, operation phases, technical timestamps and Events
appear only there, except for the ordinary operation progress, its recorded start
time and translated Live activity. Raw technical Events load only while that dialog
is open and never hold the ordinary page hostage. The dialog scrolls its body, traps
focus and closes with Escape. It states that the Sandbox keeps its home on persistent
storage and offers no snapshot download.

The owner Runtime card uses the Sandbox band, definition list, buttons and status
seat. Show current and latest image references exactly as reported, wrapping long
references without inventing semantic versions. Display the server's daily 02:00
Asia/Singapore schedule and idle deferral. Manual updates retain their exact target
and command key while an outcome is uncertain, including navigation away and back.
Queued, updating, completed and failed states use the status dot, stage fade and
reduced-motion behavior. Show same-home preservation only when the server confirms
it. A rotating Sandbox keeps its Workspaces, Agents and conversation history
readable. Update progress uses the existing detail refresh; it must not reload the
page, reset drafts, or add a separate polling loop.

The **owner Workspace page** (`/workspaces/<id>` for the Workspace's owner) starts
with the header `<Sandbox> / <Workspace>`, the Sandbox step a link to the Sandbox
page, the Workspace status badge, **New agent** as its primary action (an Agent in
this Workspace, under the same availability as New agent here) and **More actions**
with **Delete workspace**, which uses the existing Workspace deletion flow. Facts:
Path, Source (the repository as a link, or Empty directory), Branch or ref, and
Resolved commit. A table, **Agents and Worktrees**, pairs each Agent (link and status
badge) with its Worktree branch and Worktree status, since each Worktree has exactly
one Agent. The page refreshes on the Sandbox page's cadence and keeps its last
content through a transient failure, marked as reconnecting; a refusal replaces it
with the unavailable state. A viewer who does not own the Workspace keeps the shared
projection: safe names, status, the access summary, authorized Agents with their
access and channel lines, and Worktrees. A successful create followed by a failed
detail read recovers the created resource, not another creation.

All profile, service and skill references use their original catalog names and
are links. Profiles open their canonical own-profile or public Marketplace page;
NyxID services open `https://nyx.chrono-ai.fun/services/<catalog-id>`, using the
canonical catalog identity rather than a service scope alias. Ornn skills open
`https://ornn.chrono-ai.fun/skills/<id>` with the pinned version query when present.
External links use a new tab with opener isolation; local navigation preserves
the shell. Picker selection and reference navigation are separate controls.
Names are resolved through public reads, deduplicated within the signed-in
owner's mounted scope, and never replaced with UUIDs on failure. Missing names
are explicitly unavailable, not invented; raw IDs remain only in technical views.

The Agent drawer uses §9's dimensions and ordered full-width bands: Sandbox,
Workspace, Worktree, Agent profile, first-message delivery, Agent controls.
Sandbox and profile links use canonical routes. Worktree path, branch and base
commit are facts, not editable settings. The pinned model, thinking, permissions
and per-Agent skills remain inspectable after a profile changes or retires.

Use semantic definition lists with labels quieter than values. Long paths and
names wrap within the drawer. Absent, empty and unreported are distinct.
First-message delivery is recorded with timestamps, never inferred from a
visible message or a successful Agent create.

Consumer review facts retain their two-column grid. Both labels and values use
`overflow-wrap: anywhere` so long labels such as **Acknowledgement delay** wrap
inside their own column on narrow viewports without overlapping the value.

Opening Details focuses its heading; closing restores the opener when present.
Stop has explicit confirmation. Delete has §12.4's acknowledgement and stays
disabled until allowed. Failure stays beside its controls and preserves history.

## 18. Profiles and Marketplace

The account menu has one Profiles entry. Its shared library navigation contains
My profiles (`/profiles`), Marketplace (`/marketplace`), and Event workers
(`/profiles/event-workers`). Navigation uses semantic links with `aria-current`,
existing brand focus/selection tokens, 12px by 16px link padding, a 2px selected
underline, and 16px by 24px container padding. It wraps on narrow screens.

Profiles home at `/profiles` has Sandbox, Workspace and Agent tabs. Cards show
name, latest published revision, visibility and update time, with New profile
for the active kind. Editors use
`/profiles/{agent-profiles|workspace-profiles|sandbox-profiles}/{id}`.
Marketplace uses `/marketplace` and `/marketplace/{kind}/{id}`, with short
kinds `agent`, `workspace` and `sandbox`.

Event Worker Profile management uses existing Profile cards and editor bands.
Creation/editing at `/profiles/event-workers/{id}` selects three owned, published
CMA profiles with the existing ProfilePicker and names the resulting complete
configuration. Each save creates an immutable revision; existing Bots keep their
selected snapshot. Retirement requires an inline confirmation and preserves
existing Bot configurations. Uncertain saves retain their request and key.

### Editor geometry and controls

An unframed 920px maximum form has 24px page padding, 16px below 30rem. H1 uses
the existing 24px token, band headings 18px, help/errors 13px. Hairlines and
24px spacing separate bands. Paired fields use equal `minmax(0, 1fr)` tracks
and 16px gaps, stacking below 30rem. Fields retain §12.2's focus and validation.

Mode/effort choices are labelled radio segments: 40px minimum height, 44px on
narrow screens, 8px group radius and 6px options. Labels wrap without widening
the group. Repository/folder location choices are labelled radio surfaces,
8px radius and 44px minimum height. Checkboxes serve services and binary settings.

Ordered command/profile/skill lists have move-up, move-down and remove icon
commands with names and visible focus. Search results scroll within 300px.
References name an exact published revision, never an implicit draft. Skills
retain registry identity and version.

Agent fields are Model, Thinking effort, How freely it acts, Skills and System
prompt. Prompt content is plain text. Workspace fields are public GitHub URL
and optional branch/tag or empty directory, directory name, ordered setup
commands and ordered Agent Profiles. Setup runs as the Sandbox user after clone.

Sandbox fields are Standard/Large/Custom size in cores and GiB, service scope,
fixed Codex provider, model access, count limits, ordered Workspace Profiles,
one-time install commands and environment name/value rows. Size conversion
honors deployment bounds. Installs run as the Sandbox user, before credentials,
under `/home/cma`. Environment values are never secrets.

Sandbox Profiles also have an Availability band using the existing labelled
checkbox rows and bounded numeric field: Enable warm up, Warm sandboxes (1–5),
and Always keep alive. Warmup is enabled only after the Profile is published and
public; unavailable controls explain those steps. Saving applies availability
to the Profile's existing and future Sandboxes without changing their pinned
environment. Explain that manual pause remains available. No ready-instance
count is inferred from the configured target.

Marketplace detail offers Customize a private copy beside Use. Explain that
the shared original is read-only and the new private copy is editable. Sandbox
copies start fresh and do not use the original Profile's warm instances.

### Drafts, publication and trust

Save draft and Publish revision are separate commands; publication confirms
the specific immutable revision. Visibility and retirement have explicit
outcomes. Concurrency failure keeps the draft and offers Load current or a
deliberate retry against the current version. An ambiguous write retains its
exact key and body; recovery never silently becomes another command.

Every field associates help and all validation errors through accessible
descriptions. Ordinary labels use plain language; wire tokens remain technical
data and user-authored content.

Marketplace search, kind tabs and pagination preserve the query. Repeated
profile items use the same cards in both directories: 8px radius, 16px
padding/grid gap, 280px minimum tracks constrained to available width. Following
Ornn's skill-card order, show a kind glyph and label, full wrapping name, a
separate visibility/revision/retirement row, a three-line description, then an
aligned author/usage/date footer. Internal vertical gaps and footer padding use
12px; the description reserves 60px. Own profiles omit unavailable author data.
Marketplace author names come from the retained public identity directory;
missing names read Author unavailable, never an account ID. Deployment-owned
default profiles name CMA. Name refreshes never change the owner identity used
for permission or trust decisions.
The heading is a native link with an expanded full-card hit target and visible
keyboard focus; modified clicks retain normal browser navigation. Hover changes
the border only. No content or metadata read is added per card. The unframed
detail discloses all pinned commands, environment values,
services, limits and nested profile revisions. Code wraps and is never executed.

Use selects the published profile for creation; foreign commands or environment
require explicit trust over exact server-provided content. Fork creates an
independent profile with provenance. Publication never implies trust.

---

## 19. Operations

Operations is a page, not a dialog. The account action appears only when the caller
holds a capability offered by at least one view. The four views are Sandboxes,
Profiles, Platform settings, and Activity; each is independently capability-gated.
Presentation is not authorization: every request uses the public admin API.

The fixed header names Operations at `--text-page`. An underlined tablist at
`--text-ui` follows it, selected by weight, accent ink, a 2px accent bottom border,
and `aria-selected`. The strip scrolls when it cannot fit. One scrollport owns the
page. Do not retain the retired Chat hosts or Chat settings views.

### Record lists and details

Sandboxes, Warm pools and Activity are §12.6 tables. A time column names its
zone in its header — *Last activity (SGT)*, *Time (SGT)* — and draws an en-SG
date and time in Asia/Singapore with tabular numbers. Identifiers use mono
text that wraps without widening the viewport. People are identified by their
retained public display names. Missing names say “Name unavailable”; subject IDs
are never a fallback for a person's name. This applies to Sandbox and Profile
owners, Activity actors, and Activity targets that are users. The ordinary Sandbox
information page also shows its immutable creator under Owner.

The Sandboxes table's columns are Sandbox (the name over its mono id), Owner,
Status (a §12.1 badge in the shared Sandbox vocabulary), Runtime (or *not bound*),
Agents, and Last activity (SGT), with Details and Retire as row actions. Owner,
status, and version filters retain their values during reads. Details opens the
existing dialog with Agents grouped beneath their named Workspaces, pinned Profile
references, and the durable Sandbox event timeline. No row-wide destructive target
exists. Unknown counts or unavailable runtime observations remain explicitly unknown.

A capability the caller lacks is said once for the page, not on every row: without
cross-owner control one information alert above the table reads *Cross-owner
control is required.* and every Retire stays visible but disabled. A Deleting or
Deleted status badge is the reason that row's Retire is unavailable.

The Warm pools table's columns are Size (CPU and memory, with the Current default
configuration tag and the Pool configuration disclosure), Status (Pending, Ready,
Disabled or Unavailable as a §12.1 badge), Desired, Ready, Pending (Unknown until
the controller has observed the pool), Target (the bounded input and Update
target), and Disable pool as the row action. Waiting for the controller, a pool
error, a failed or unconfirmed write, the retained-draft notice with Read current
target, and Target saved are the pool's note row. The sentence on disabling spare
capacity is said once above the table. Previous runtime pools use the same table
inside their disclosure without the Target column, under one notice that only
disabling is available and claimed Sandboxes keep running.

Profiles rows show kind, visibility, owner, name, and update time. The three filters
are native selectors or bounded inputs. A retired profile is not silently hidden
from an operator who has selected that state. Deployment defaults carry a
read-only default marker, not a second interactive control.

Retire uses §12.4 confirmation and the server-audited action. Controls require the exact
cross-owner capability and remain disabled with an adjacent reason when absent.
The confirmation names the target and the effect; it survives a failed request.
Success rereads the record. Admin retirement has no invented idempotency-key
contract. Never automatically repeat an ambiguous destructive request.

Activity is a table of Time (SGT), Actor (*Not signed in* or the actor's name),
Action, Target (*User <name>* or the kind with its mono id), Outcome (Succeeded
neutral, Refused warning, Failed error, as §12.1 badges) and Code. It keeps the
public audit fields. It does not
display credentials, profile contents, arbitrary runtime output, or internal
checkpoint authority. All lists have loading, unavailable, empty, and pagination
states. A failed refresh retains the last successful rows.

### Platform settings

Settings is an unframed form no wider than 920px, using full-width sections
separated by hairlines. Sections cover Defaults, Inactivity, Snapshots, Retention, Limits,
Triggers, and Marketplace. The three defaults use exact published-profile
revision pickers with kind-specific summaries. A mutable latest revision is never
silently substituted for a pinned choice.

Time and count settings use bounded numeric inputs with explicit units. Snapshot
frequency is presented in minutes (1 to 1440, default 30), stored in
whole seconds, and hidden only when connected to an older API without the setting.
Fractional minutes preserve values set through the public API. Binary
enablement uses checkboxes or switches; closed policies use native selects.
Trigger enablement and paused admission are distinct settings. Keep unfamiliar
or newer server fields in the document during an edit; never erase them by
reconstructing only the fields rendered by this client.

Every write names the revision read. A conflict keeps the operator's draft,
reads the current document, and offers explicit Load current values and
Review retained draft actions. No automatic last-write-wins retry is allowed.
Save is separate from any reset operation. Validation appears beside its field;
the typed server refusal remains authoritative.

At compact widths rows and confirmation actions become one column. Long profile
names, URLs, audit facts, and failures wrap. Drawers have bounded dimensions
and their own internal scroll; they trap and restore focus using §12.4.
No new palette, type scale, radius, or spacing token is introduced here.

---

## 19a. Developer (retired)

The Developer reading surface is retired; nothing renders it. `/developer` still parses so old links
keep working, but it opens Docs and the address bar is rewritten to `/docs` without adding a
history entry. Nothing links to it.

---

## 19b. CMA Trigger publisher

CMA Trigger is an authenticated, persistent authoring surface opened from the
account menu when publisher capability is present. Its routes are
`/triggers/manage`, `/triggers/manage/{id}`, and
`/triggers/manage/{id}/embed`. Refresh and browser history preserve the route.
Public `/t/{publicId}` and `/t/{publicId}/{placementId}` visitor pages are separate.

The fixed header names CMA Trigger and carries a compact New Trigger command.
Below it, one scrollport holds an unframed 240px rail, a 32px gap, and the editor
on a maximum 1,100px axis. Rail rows show title, the Trigger's status as a §12.1
badge (Draft muted, Published neutral, Retired muted), and short public
identifier; the open Trigger's header carries the same badge beside its
title. Selected rows use the existing navigation ground, a leading accent
rule, weight, and `aria-current`. Below 48rem the rail becomes a full-width
40px native selector, the gap becomes 16px, and field rows become one column.
No text or control may widen the viewport.

### Five-step authoring

The wizard uses §12.5's stepped-tool treatment without cards nested inside it.
Step labels use compact UI type. Back preserves all entered values; an unavailable
read or failed mutation does not discard the draft.

1. **What it opens** selects exact Sandbox, Workspace, and Agent Profile revisions.
   Each picker shows a one-line summary. Workspace choices include the selected
   Sandbox Profile's references and other usable own/public revisions. The
   resulting environment is summarized with concrete profile names and services.
2. **When someone clicks again** uses segmented or native option controls.
   Sandbox and Workspace each choose reuse or new; Agent chooses continue, new,
   or ask. The summary distinguishes a new Sandbox, a new Workspace in the
   visitor's Sandbox, and a new Agent in the visitor's Workspace. It never implies
   access to the publisher's instances.
3. **What it says** edits title, description, first message, the four closed button
   labels, theme, review-first checkbox, and optional sign-up hint. Render the
   actual server badge for preview after publication; unpublished labels use the
   checked-in server golden badge assets. Publisher content is plain text, not HTML.
   Quick messages are editable repeated fieldsets, with Name, Description and
   Message text. Add and remove use existing command buttons, never nested cards.
   Limit names to 120 UTF-8 bytes, descriptions to 2,000 and text to 32,768; names
   must be nonempty and unique ignoring case and surrounding whitespace. All
   three fields are required per message, with at most eight messages. Saving a
   draft does not alter visitor buttons until explicit publication.
4. **Where it lives** edits labeled source-URL placements with add/remove commands.
   Every mutation retains one key and exact payload until its outcome is known.
   A placement is attribution and an allowed source declaration, not authentication.
5. **Publish** presents a confirmation naming the immutable revision and targets.
   Publish is explicit, separate from Save, and followed by the embed panel.
   Private/unusable profile refusals appear beside the corresponding picker.

Profile content containing commands or environment variables is inspectable before
publication and before visitor trust confirmation. The publisher accepts no token,
OAuth code, GitHub credential, or Sandbox key. Reuse policies never grant access to
another owner's resources. The retired repository-verification/contribution/PR
workflow is absent.

Draft writes use the public API's version and idempotency contract. Conflicts retain
the editor and offer explicit current-draft loading or review against the new
version. Ambiguous outcomes retain the exact command and suppress duplicate writes;
never claim successful publication before the authoritative reply.

### Embed panel and retirement

Host tabs are README or Markdown, Web page, Page with JavaScript, React app, and
Just the link. Use actual tabs with `aria-selected`, not decorative chips. A native
placement selector and theme controls select the server descriptor. The badge
preview is shown against the established light and dark grounds.

Each snippet is read-only code with a symbol-only Copy command, tooltip, and polite
live feedback that does not change geometry. Render the descriptor verbatim;
never regenerate Markdown, HTML, JavaScript, React, SRI, or CSP strings in the
browser. The JavaScript view includes the exact SRI and both CSP directives.
A compact stats strip names Launch counts per placement and Agents ready, with
unknown/loading states distinguished from zero.

Retire requires §12.4 confirmation naming the target. Existing Launches remain
durable; new use follows the server's retired-state refusal. Failure preserves
the confirmation, and a successful reply refreshes the record.

All actions have visible focus. Dialogs trap and restore focus, status updates
do not move it, and narrow controls use the established 44px targets. Reduced
motion changes no validation, publication, copy, or recovery state. Use only
existing tokens, field primitives, code treatment, and button hierarchy.

---

## 20. The signed-out surface

The signed-out home says what CMA is and offers one primary action. It sits in
the centre column of the collapsed portal frame and scrolls as one page:

- **Hero.** The product name, ChronoAI Managed Agent, is the page's `h1`, set
  as a 13px 500 pill with a 1px `border-l2` outline, 999px radius and a 6px
  accent dot. Below it a display line states the value in one sentence at
  `clamp(36px, 5vw, 56px)`, 600, 1.05 leading and `-0.035em` tracking,
  balanced across lines; then an 18/28 secondary lede. The one primary Button
  is **Get Started**; an outline anchor styled as a Button jumps to How it
  works. A 13px caption names NyxID and doubles as the polite live summary
  while sign-in opens. The Three.js character sits beside the copy as a
  companion: it spins on press and is never the way in.
- **How it works.** Three ordered steps in 12px-radius `layer-1` cards with a
  1px `border-l1`, each numbered in a 28px mono square.
- **Services.** Agent Manager and Event Gateway cards each carry an outline
  `Open …` Button that signs in and returns to that service. Image Manager is
  a dashed card marked Coming soon, with no action, until its route exists.
- Section kickers are 12px code-face `brand-text`; section headings are 32/40,
  600, `-0.025em`, 24/32 under 760px.
- Below 900px the hero stacks with the character first at most 220px wide;
  below 760px the card grids become one column and nothing scrolls sideways.
- Sign-in fades the page to 40% for 200ms, or immediately under reduced
  motion, and starts once. A failure restores every action and shows an inline
  alert with Try again.
- Copy never names the agent provider; CMA is the brand.

A public Trigger URL has its own complete read-only landing (§13b), not this
generic gate. Sign in and Create account preserve the canonical local return
route and the visitor's bounded intent. No page accepts credentials, and no
OAuth callback starts work without the explicit consent belonging to that
intent.

---

## 21. The mark

The CMA mark: an arc over a four-peak base, with two lenses and a bridge. It ships as SVG under
`web/src/assets/brand/`, in a still form and an animated one, and both are the same geometry — the
animated file adds timing and changes no path, so the two can never drift into being different logos.

| File                     | Where it is used                         |
| ------------------------ | ---------------------------------------- |
| `cma-mark.svg`           | signed-out mark and still uses |
| `cma-mark-animated.svg`  | signed-in sidebar                        |
| `cma-favicon.svg`        | the browser tab                          |
| `appicon-*`              | installed application and device chrome  |
| `cma-lockup-horizontal*` | anywhere outside the product             |

The animated mark draws its arc, then its base, then its lenses, and afterwards a highlight crosses
the lenses on a long loop. It lives in the signed-in sidebar and mounts with the authenticated shell,
not with Agent or view changes, so its entrance does not replay while somebody navigates their work.
The sidebar wrapper stays still: the bob that belonged to the former signed-out treatment does not
move into chrome. The asset uses CSS rather than SMIL so `prefers-reduced-motion` can show the finished
mark with no draw or highlight animation.

The geometry never changes: arc, four-peak base, two lenses, and bridge are re-inked rather than
redrawn. The mark takes the theme accent, `#0E61AC` in light and `#C3FD43` in dark. This deliberately
reverses the former fixed-blue rule: the two design languages use ink and light differently, and a
mark that ignored that would be the one foreign-coloured object on each surface.

The favicon contains both accent treatments and selects with `prefers-color-scheme`, so the browser
tab agrees before application JavaScript runs. The app icon is the fixed cross-theme exception:
lime `#C3FD43` on a `#060606` tile, because operating-system chrome does not expose the application's
resolved body theme. Horizontal lockups ship in the corresponding light and dark accent treatments;
their path geometry remains identical.

The wordmark is set live in the theme's interface face rather than shipped as paths, so it stays
selectable text and changes from Familjen Grotesk to Archivo with the appearance. `cma` in
mono stays for anything naming the repository, the images or the API — those are identifiers, not
the product's name.

**Set live, the wordmark is set the way the lockup sets it: weight 700 at `--track-display`.** The
shipped lockup tracks its own CMA at `-1.6` on a 54px face, which is `-0.030em`, and
`--track-display` is `-0.035em` — near enough that the name in chrome and the name on the lockup
read as one wordmark rather than two. Positive tracking is a treatment for small caps and label
runs and must never be applied to it: holding a three-letter name apart at reading size makes it
look like a category label.

**In chrome the mark is 32px.** The sidebar's logo row is 60px with 44px of content box, and a
smaller mark sits in more than its own height of empty space, reading as an icon beside the name
rather than as the product's mark. 32px also clears the 36px square the collapsed rail gives it, so
one size serves both states and the closed rail needs no rule of its own.

---

## 22. What does not change

These rules apply to every view:

- A turn read from durable history and the same turn watched arriving converge
  on the same content, with live-only observations clearly distinguished.
- Absence is a fact, not a zero.
- Agent content is not recoloured for decoration.
- A refusal names what is missing and a supported next action.
- Recorded resource states govern input and lifecycle actions.
- Credentials are never rendered.

A style change must not alter ownership, consent, durability or these behaviors.

---

## 23. Pre-refactor visual continuity

The September 7 Trigger-loop requirements restore the presentation shipped at
`f5b6ded65750a75b5f2144e7d90436ceb402776d` (immediately before the retired
model removal), without restoring the retired data model. This section
supersedes the static-authentication-mark wording in sections 14a, 20 and 21.
Shared Button, Picker, Menu, font and palette geometry remains the existing
Inkwell/Lumen implementation from that baseline.

The original Three.js character geometry, materials, studio lights, pointer
look, press-to-spin and visibility disposal return to the unframed Home and
authentication surfaces. The stable square is bounded to the available width,
220px minimum when space permits and 520px maximum; compact Home uses 280px.
The signed-in wordmark is 48px, weight 700, with zero tracking. The signed-out
home follows §20: the character is a companion beside the hero copy, not the
sign-in control, and carries no invitation cloud. The glasses have no text.
The invitation cloud below is retired from the signed-out home by §20; its
styles and Shrikhand remain only for a character given an activation handler.
A cloud-shaped, 264px by 88px prompt says Click to start... above the character,
with its top 120px above the reserved square. The signed-out lockup reserves
144px top padding so the cloud never covers the character or clips the viewport.
It uses layer-1 fill, primary ink,
24px/1.2 bold Shrikhand text, a quiet shadow and a 16px speech tail placed 92px
from the left and 4px below the body. Two 112px by 64px rounded lobes, positioned
40px from the left and 12px above the body, and 108px from the left at its top,
form the cloud without an image or new palette. Hover or keyboard
focus reveals it; five seconds without either reveals it automatically, including
touch and WebGL fallback. Its 0.45-second entrance rises 12px from 0.85 scale
with a small 1.04 overshoot. Hover/focus also triggers a 0.65-second character
bump of 0.08 scene units and 6% scale, gated to signed-out activation only.
The logo is a keyboard-operable Get Started button; activation makes one complete
0.95-second turn, followed by the existing 0.2-second opacity handover into sign-in.
Repeated activation cannot start duplicate sign-ins. Reduced motion keeps the
prompt static when revealed and skips bump, turn and fade; unavailable WebGL retains a
labelled, clickable static fallback. Failed sign-in restores an explicit retry.
This activation behavior never applies to signed-in Home or step characters.

> Superseded by §12.1 (2026-10-21): in-app steps, connection rows, recovery and runtime notices and
> the Thinking disclosure use a status dot, and loading surfaces use skeletons. The paragraph below
> records the earlier behaviour.

Every provisioning step uses that same character in a fixed 44px square.
Completed steps are green and make one small jump every two seconds: a 0.6-second
sine arc with 0.08 scene-unit lift followed by rest. Running steps stay blue and
continuously turning; pending steps are yellow and turn once per second.
Failures are red and still. Motion stays inside the reserved square and never
moves a row or its text; it does not advance the recorded step state.
These semantic character aliases resolve to existing moss 500, ink 800,
amber 300 and red 600 in both themes; the colour is a redundant reinforcement
of the adjacent written status. Compact characters omit the contact shadow.
Running, pending and completed characters schedule frames only while visible;
reduced motion makes every state still. Loading surfaces use the same running
character, never another spinner. WebGL fallback remains still and state-coloured.

The Agent composer restores the baseline 22px-radius capsule, 12px internal
gap, full-width 16/24 textarea growing to fourteen lines, and a separate lower row.
The 34px circular Send control becomes a neutral circular Stop control in the
same seat. On narrow screens its visible circle remains 34px inside a transparent
44px hit area contained by the existing 44px grid seat. The Thinking disclosure
retains its 120px capped body, with the inline status, compact rotating character,
restored elapsed time and separate completed tool containers specified in §13.
Tool output is disclosed only when actually supplied; live reasoning
is never fabricated on reload. The character stops when the turn ends.
Message role labels return outside the owner's bubble at 12/18 with an 8px
gap; turns keep the baseline 16px rhythm. Thinking adds no extra bottom margin.
Required small text uses caption ink. This is an explicit accessibility
accommodation to the baseline: Thinking body and tool details use secondary
ink instead of the old tertiary ink, metric text retains caption, and the
composer placeholder uses caption instead of the old dimmed ink. Thinking's
layer-2 ground needs secondary ink: light caption there measures only 4.29:1,
while secondary measures 5.92:1. The old tertiary and dimmed inks do not meet
the required small-text contrast in both themes; they are not restored or
described as exact colour parity.

Conversation opening and live output follow the latest content while the reader
remains at the end, including content-height changes after layout. Scrolling
up preserves the reader's position until they return to the end. Completed Agent
preparation leaves the conversation surface once no step is active or failed;
the pinned configuration remains available in Agent details.
The Agent's single grid column is shrinkable to the available center width;
long titles and tables never widen the conversation, composer or instruments.

Below the composer, restore the baseline two-column tokens/context row and
three-column RAM/CPU/network row with 12px figures, 12px column gaps and 2px
neutral tracks capped at 240px. Resource observations use the public Agent
resources read, sampled every five seconds while visible. CPU rate derives
only from two monotonic samples of the same Sandbox uptime; restart, failure
or absent samples remain Measuring, unavailable or not reported, never zero.
Network has no ceiling or percentage. Failed refresh retains the reading and
labels it stale. Layout stays three columns on narrow screens, wrapping within
each column rather than widening the viewport.

For a published Trigger with review disabled, an external button navigation
carrying a server-confirmed browser gesture supplies the bounded consent.
The browser consumes that evidence through the public entry-intent operation,
retains its exact command across necessary sign-in and proceeds without an
extra Proceed/Next command. Generic navigation, reloads, crawlers and OAuth
callbacks without this evidence remain read-only. Explicitly reviewed
Triggers retain their review controls. This supersedes wording that limited
the consent gesture to a second click on the CMA landing page.

### Confirmed Agent creation and retained navigation

New Agent preparation replaces the dialog's form with a heading, one status
line and a four-step stepper with no percentage: **Sandbox**, **Workspace**,
**Service access** and **Agent startup**. Each step shows a state dot, its name
and its state word: Waiting, Queued, In progress, Ready, Waiting for connection,
Needs attention or Not reported. Ready takes the done dot, In progress the
ongoing dot (whose ring stops under reduced motion), Waiting for connection the
warning dot, Needs attention the error dot and the rest the neutral dot; the step
in progress is the current step. Steps sit in four columns from 30rem and stack
below. The heading reads *Preparing your Agent*, *Agent ready* or *Agent
provisioning needs attention*; *Agent ready* requires the successful operation
plus its actual Agent identity, even when all four steps are Ready. Provisioning
and per-step elapsed times are information only and never advance a step. The
same stepper is the one setup band above a starting Agent's conversation. This
replaces the 64px percentage circle and its closed Setup details disclosure.

Agent key scope rows are 64px high. Skill rows are 112px, with 18px / 18px / 40px
internal tracks and 6px gaps. Text is single-line ellipsis with full hover text;
controls remain visible. All lists use existing typography, borders and tokens.

Recently opened Agent conversations remain in a bounded memory-only cache for
the current account and client. Switching back restores recorded content while
current resource access is checked; actions remain unavailable during that
check. Reopening resumes events at the saved cursor rather than downloading
history from zero. Denied access clears the view, and account/client changes
cannot reuse it. An ordinary stream handshake does not create a lost-update
warning. Explicit lag and actually discarded live fragments remain visible.

The same account/client scope retains up to six recently visited owner Sandbox
snapshots and their loaded Workspace/Agent details. Re-entry paints this recorded
content immediately while current access and lifecycle state are checked; all
mutation controls, connection setup and live resource readings wait for those
checks. Shared views never consume owner snapshots. Access denial, missing or
retiring/retired resources clear their retained display data. Retain only the
immutable Agent creation category for the owner header; privileged provenance
and source actions still require fresh authorized reads. Memory retention adds
no background polling and ends with the authenticated scope.

## Changelog

Append-only. Never edit or delete an entry.

| Date       | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-20 | Give conversations at most 300px wide a full-width draft and usage row, with a separate touch-sized action row; verify settled shell geometry, cold mobile loads and retained desktop navigation preferences. |
| 2026-08-12 | Initial version. Established colour tokens for light and dark, semantic state colours, type stacks and scale, 4px spacing system, component rules for status chip, data table, terminal and buttons, interaction and accessibility baselines.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| 2026-08-13 | Terminal-first cyberpunk redesign. Reframed the frontend as a single full-bleed terminal page with no navigation, in two states (scripted introduction and live session). Replaced the colour system with a cyan/magenta-on-near-black palette, recording measured WCAG ratios and raising `--ink-3` to `#7C8A9E` in dark to clear AA for 11px metadata. Added the `--emphasis` token, a bounded three-step glow system with an ambient overlay cap, and an authoritative 16-slot ANSI palette. Promoted the monospace stack to primary and retuned the type scale. Made the page non-scrolling and the terminal full-bleed, and set radii to 0/2px. Specified the intro sequence timing, skip behaviour and reduced-motion equivalence, the session tab strip, and the profile menu. Added §8 Terminal safety covering OSC 52, tab-title suppression, scrollback caps and OSC 8 validation. Removed the data table component and the "information density over whitespace" premise, both of which described a dashboard this frontend no longer is.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 2026-08-13 | Documented one focus-state variant in §7: the terminal prompt row carries its 2px accent indicator on the row's leading edge rather than as a ring around the input, because a ring around a full-width borderless field boxes in the whole prompt line and breaks the terminal illusion. A relocation, not a removal, scoped to that one input.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-08-13 | Added §10 Selectable terminal themes: six vendored palettes (Dracula, Nord, Solarized Dark, Solarized Light, GitHub Light) alongside the product's own, selectable per person and scoped to the terminal canvas only. Vendored as data rather than taken as a dependency. Themes are filtered by the current appearance so a dark palette cannot be placed on a light page. Also specified the appearance control in the profile menu, cycling system → light → dark, where "system" is the absence of a choice rather than a third palette.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-14 | Added §11 Waiting for a session: the provisioning message animates as a left-to-right character wave (1.4s loop, 45ms stagger, 0.28em lift), chosen over a spinner because §5 forbids one and because a wave claims no knowledge of progress. Motion carries no information — the sentence is exposed once to assistive technology and the animation is removed under reduced-motion. Also specified that a session which fails to start must show the recorded reason rather than the same wording as one that ended normally.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 2026-08-14 | Added §12 Page frame and layout and §13 The platform typeface. Established a persistent top bar in both signed-out and signed-in states, a 10% inset on page content with terminal output exempt, a vertical session column on the left labelled by creation time in Singapore time (newest first, reversible, date shown only once not from today), and a collapsible session details panel on the right that collapses to a handle. Columns stack below 60rem. Adopted Geist Pixel as the platform typeface, self-hosted under the SIL Open Font License with a system fallback and `font-display: swap`; terminal content keeps its monospace stack because cell metrics govern its rendering. Replaced the horizontal session tab strip specified in the 2026-08-13 redesign, which truncated labels once sessions accumulated.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 2026-08-14 | Corrected §12: the console obeys the page inset like every other surface, so the session column aligns with the product mark and the details panel with the account control. The earlier entry exempted terminal output from the inset on the grounds that cropping it wasted width; alignment between the bar and the page beneath it matters more, and a full-bleed console under an inset bar reads as a detached bar. Also moved the session lifecycle controls out of the top bar and into the details panel, beside the session they act on.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-14 | Added §14 Session states and what they allow: a single table binding each of the six session states to what the terminal does and which controls are available, with one rule shared by the terminal, the buttons and the list so they cannot disagree. Established that controls are disabled rather than removed and that every disabled control and every non-interactive terminal states its reason in the owner's terms; that a session with no sandbox still shows its history read-only, with keystrokes dropped rather than buffered; that ended sessions leave the list unless they ended by failing; and that revive is the primary action wherever it is offered. Adds the `rotating` state, during which the session is neither usable nor stopped and no control acts on it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| 2026-08-14 | Added §15 Canvas — every session at once: a second arrangement of the same sessions, in which each listed session is a 420×280 framed node on a pannable surface hosting its own terminal, chosen by a two-button toggle at the trailing edge of the top bar whose stored value is validated on read. The canvas replaces the console's middle column only, so the rail, the details panel and the bar keep their places and one selection serves both arrangements. Established that exactly one node is focused and only it receives keystrokes, with hover distinguished from focus by border weight rather than hue; that focus is never a second opinion on §14, so a provisioning, rotating, idle or ended session stays read-only on the canvas, says so on the node, and shows its stored transcript rather than an empty frame; that the canvas is one tab stop with clamped arrow navigation and a polite live region announcing moves; that unfocused terminals are never dimmed, tinted or covered; that the only motion is the 150ms focus transition, wrapped in `prefers-reduced-motion: no-preference`; and that at most 12 nodes hold a terminal at once, evicted by least recent focus, with the count of unattached sessions stated below the viewport because silent truncation reads as breakage. Adds the `--canvas-node-w` and `--canvas-node-h` tokens, and labels nodes by start time as §12 labels the rail rather than by identifier, so one session is not called two things in two views.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| 2026-08-14 | Specified in §12 where a failed action is reported: one `sm` `--danger` line with a `--rule` underline at the head of the middle column, above whatever the column is showing and never in place of it, recording 6.16:1 light and 5.47:1 dark against `--surface`. The console previously rendered the failure instead of the surface, which was survivable when the surface was one terminal and is not now that §15 puts up to twelve attached terminals there — a stop returning 409 detached eleven sessions that had not failed. Also established that the line reports the most recent attempt, cleared by every action as it starts, so a stale failure never sits above a console that has recovered.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 2026-08-14 | Recorded in §15 that a node's size is the geometry its runtime is told to draw at, so choosing the canvas resizes every attached session to roughly 47 columns and a dozen rows and each agent redraws at that width. Kept the behaviour — a terminal in a small frame is a small terminal, and declaring anything else wraps output against a width the frame does not have — but stated its two consequences: output already wrapped narrow stays wrapped, and a session detached mid-arrangement keeps its last geometry until something attaches to it and re-declares one. The mode toggle was previously described in the code as reaching nothing beyond the arrangement; it reaches the runtime of every session on the canvas, and may not be described as a browser-only control.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| 2026-08-14 | Added §16 The operator panel: a dialog opened from the profile menu as an account action, keeping the product one page, whose entry point is drawn only when the control plane reports that the caller holds an operator capability — presentation, never protection, since the API refuses on its own. Established that each of its four views is offered on its own capability, so a view is never drawn empty because of entitlement; a view switch marked up as a tablist (monospace `xs` uppercase labels; selected by `--accent` text plus a 2px `--accent` bottom border and `aria-selected`, never colour alone); and a record list — rows rather than the data table removed on 2026-08-13 — with a leading tabular-nums timestamp, `base` primary label, `xs` `--ink-3` monospace metadata, hairline separators, no zebra striping and no row-level click target, ordered newest first always and filterable by person and by action. Established that a person is named by display name then email and never by their subject identifier, which is shown in identifier style when nothing else is known, and that an unattributed action reads "Not signed in"; that a provider which could not be asked renders as unknown carrying its reason and never as one running nothing, with the running total withheld while any provider is unobservable; and that stop and terminate on another owner's session keep the §6 destructive pattern — terminate confirms inline, both are disabled with a stated reason rather than removed, and deleting somebody else's session is not offered at all.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-17 | Ground control: full redesign of the frontend's visual language and layout, replacing the cyberpunk terminal-first system established 2026-08-13. Retired the cyan/magenta pair for one lime accent with a measured light counterpart, and reworked the surface ramp into ground/surface/raised/inset with worst-case contrast recorded per theme (dark `--ink-3` 5.38:1 on inset; light `--ink-3` 4.59:1 on inset). Replaced the bounded glow system with letterpress depth — hard-offset impression shadows with zero blur, hover translating into the impression, active flush, disabled carrying none, focus stacking a page-halo and accent ring over whichever impression the state holds — leaving one 6px halo on a live session's dot as the only glow in the product. Adopted Geist and Geist Mono, self-hosted, and retired Geist Pixel in favour of a mark drawn from four squares. Removed the 10% page inset, which cost the terminal ~290px at 1440px, and replaced it with a band grid: a 52px head band and a 40px foot band whose heights are set rather than accumulated from padding, one type size per band, and a second band for any column needing more chrome. Established one app bar and one status band on every page; two control heights and no others, every control `box-sizing: border-box` so a bordered and an unbordered control measure alike; peer cards as grids so content length cannot decide card size; one dropdown pattern, instrument closed and plate open; and a collapse pattern in which a panel becomes a 48px handle carrying a summary rather than disappearing. Reframed the frontend as one page frame with two views — sessions and operations — where the canvas is an arrangement of sessions rather than a view of its own, and moved operations out of a 560px dialog onto a page for the one surface that shows many rows at once. Kept the canvas node fixed at 420×280 and restated why: a node's size is the geometry its runtime draws at, so a fluid grid would re-wrap everything an agent has printed. Promoted the session lifecycle controls into the session header, demoted the details panel to a 412px sheet beneath the terminal, and added a last-activity line to each rail row. Added §16 Responsive with four widths, and the rule that the terminal is the last surface to give up width. Set light `--ink-3` and `--ended` to `#686871`: at the `#6B6B74` the handoff carried, both measured 4.39:1 on `--inset` and failed the 4.5:1 floor this section sets for the 11px uppercase metadata `--ink-3` exists to carry.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |     |
| 2026-08-18 | §1 admits the conversation plane. The frontend was "a terminal, and the chrome needed to run several of them", which was true while a terminal was the only thing it could offer; `acp-kernel` made a second plane possible and nobody had built one. It becomes two planes onto one session — terminal and conversation — drawn in the same middle column and chosen by what the session was granted. A plane is not a view, exactly as an arrangement is not: the views stay two, the frame stays one, and there is still no navigation. Replaced the binding rule "never build an interactive feature here that belongs in the API" with a stricter one: the conversation surface may call no route a third-party client could not call, and where it needs something AG-UI lacks the answer is to add it to AG-UI rather than here. The old rule asked whether a feature _belonged_ in the API, which is a judgement; the new one asks whether the API _already projects it_, which is checkable — and a first-party client held to the same constraint is the only thing that keeps `acp-kernel`'s claim honest. Re-examined §1's three surviving design consequences: "the terminal owns whatever width is left" becomes the _surface_ owning it, terminal or conversation; "one surface, many drivers" survives unchanged and now covers a conversation driven by a recorded transcript in development and a live stream in production; "fidelity beats decoration inside the viewport" survives and extends to agent-emitted message content, so the code blocks, lists and inline formatting an agent wrote are rendered as it meant them and only the frame around them is styled.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-18 | Added §18 The conversation surface, the plane §1 admitted on the same date, written before it is built so its review is "does this match §18" rather than a matter of taste. §18.1 the message stream: turns as bands separated by a rule rather than speech bubbles, the owner's turns in `--ink-2` and the agent's in `--ink`, nothing restyling itself on commit because a message that does tells a reader the earlier state was provisional, the surface pinned to the newest turn only while nobody has scrolled away from it, and the equivalence rule that governs the whole plane — a turn drawn from history and a turn watched arriving must produce the same result, since the one a person reloads into is the one they will believe. Extended §1's fidelity rule concretely: code blocks, inline code, lists and headings take the scale §4 already defines, and no colour is ever applied to agent-emitted content, because the semantic tokens describe session and system state and describe nothing an agent said. §18.2 tool calls as structure: collapsed by default at one control height, the kernel's own words for kind and target rendered verbatim rather than mapped onto a set this document invented, a running call distinguished by the wait animation and a failed one by `--fail` with a status word beside it, and enormous output bounded by disclosure rather than truncated — bounding is a frame decision and dropping the tail is not. §18.3 the approval card: the card impression and an accent border, drawn in the message stream where the turn stopped, carrying the kernel's own wording verbatim and the answers the kernel offered rather than a hardcoded allow/deny pair, the affirmative answer taking the surface's one accent key, surviving a reconnect because approvals ride the durable stream, and explicitly **not a modal** — a session can wait on one while its owner reads something else, and a modal makes every other session in the rail unreachable. §18.4 the composer: a block-height field growing to six lines, and behaviour mid-turn read from the session's own `concurrent_input` policy rather than invented or guessed at, with the sentence on the surface beside the key. Steering shows no affordance at all where it was not granted, because a control that can never work is not one that is currently unavailable, and interrupt lives beside the composer for the same reason somebody reaches for it there. §18.5 usage, and the honest absence of it: the count and cost in the surface's foot band, never estimated, and a session not granted `usage` saying "not reported" rather than showing a zero — the same rule §14 holds for a running total while a provider is unobservable, for the same reason, since a session with no usage grant has often spent a great deal. §18.6 plane and changing it: the affordance absent rather than disabled where the kernel does not advertise the other plane, an inline confirmation stating the two costs that are not obvious, `rotating` governing the window, and the boundary afterwards drawn as a rule across the message stream at `rotated_at` rather than as a status chip — a gap somebody can see is one they can recognise. §18.7 what the surface does not do: it renders no terminal, offers no operation AG-UI does not project, names the capability behind every refusal on screen rather than only in a console, and never renders a credential. Amended §11 so its one table covers both planes rather than growing a second that could disagree with it, and added what the three routinely-misread states mean — `rotating` where clients must refuse input rather than accept and lose it, and `idle` and `recovering`, neither of which is a failure and neither of which takes `--fail`. Amended §12 where it assumed the middle column was a terminal: three columns are the list, the session's _surface_, and what is known about it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 2026-08-18 | Extended §18.7 with what the terminal can do that the conversation surface cannot, and where each of those lives instead. The section already said the surface renders no terminal and offers no operation AG-UI does not project; what it did not say is what somebody should do about it. Three of the six rows have one answer — the plane changes and the session does not, keeping its identifier, its history and its place in the rail — and the other three are capabilities the kernel behind the session either offers or is named for lacking. Stated rather than left to be discovered, because a person hunting for a control that was never going to exist blames the interface and then themselves.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-18 | Added §18.9 A run that failed. §11 already said a session that ends badly says why and §18.7 already said every refusal names its capability on screen rather than only in a console; neither was drawn, because the conversation surface folded a failed run into state nothing read, so a run that failed looked exactly like a run still going — nothing at all. Established that a failure is drawn in the stream at the point it stopped rather than in a band above it, on the same reasoning §12 uses for a failed action and one step further: a failure has a position, and what the agent had already done stays readable beside it. Established the three things it says and their order — what failed in the kernel's own words, the ability it lacked **named** and only where the failure was a refusal, and the stop reason as the API spells it — and that most failures name nothing, since a name invented for those would be read as one and acted on. Recorded the trap that made this worth writing down: a session being carried onto another runtime ends the run and arrives on the _same frame_ a genuine failure does, because the protocol has one event for a run ending badly, so a client that cannot tell them apart draws every rotation as a fault — which §11 forbids outright. What distinguishes them is stated by the API rather than inferred from wording, and where it is absent the frame is drawn as a failure, that being the safer of the two to be wrong about.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-18 | Amended §18.1 with what a turn looks like while it is still running, which the section had described only for its two settled ends. Between the turn starting and anything arriving the surface draws the §11 wave with a word: a turn that has started and produced nothing is otherwise indistinguishable from a dead stream, and a turn takes as long as the agent takes. Established that the message being typed is **not a stream entry** — it is drawn after the last entry, in the same band and ink as any other, and is _replaced_ by the committed message rather than appended to. That is what makes the section's own equivalence rule hold by construction rather than by discipline: history contains no in-flight message, so a reload folds none and has none to draw, and the two agree without anybody remembering to make them. Modelling it as an entry and removing it on commit would be the same rule enforced by hand, and the hand is what fails. The existing caret rule now follows from that rather than standing as a second one: the caret belongs to the message being typed and to nothing else.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-18 | Added §18.8 What the agent is working through. A turn spent thinking was indistinguishable from a turn spent stuck, and thinking is most of what a long turn is. Established that reasoning is **liveness and never in the record** — removed when the turn ends, absent after a reload — and that the surface says so in words rather than leaving it to this document, because a reader who saw reasoning and reloaded to find it gone reports it as lost work. Recorded why it may not be durable: §18.1 requires a turn drawn from history and a turn watched arriving to produce the same result, so a durable thought is one that gets replayed, and somebody reloading to check what the agent decided would watch it think a second time; the turn's own boundaries already say when the thinking happened. One band above the message being typed, height-capped with its own scroll pinned to the newest, since reasoning is unbounded and arrives fast and would otherwise push the conversation off the screen while saying less than any line of it. **No colour, ever** — §2's semantic inks describe session and system state and describe nothing the agent thought — with the label carrying the §11 wave while reasoning arrives. A kernel that does not reason draws nothing at all rather than an empty band, on §18.4's existing rule. And it is not announced: the band changes many times a second, and a live region there would make the surface unusable with a screen reader, while the message that follows is announced and is what is worth hearing.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-18 | Quiet instrument: full redesign of the frontend's visual language, replacing the Ground control letterpress system of 2026-08-17. Kept the accent and every rule in §19, changed everything that made the interface hard to read. Replaced the neutral ramp with warm near-neutrals in five steps per theme (ground/surface/raised/inset/sunken), recording worst-case measured contrast per theme — dark `--ink-3` 4.69:1 on `--inset`, light `--ink-3` 5.06:1 on `--inset` — and set `--ink-3` to `#8A9082` dark and `#64685E` light so 11–12px metadata clears AA on every surface it sits on, including a disabled control's own fill, which the previous revision left at 2.5:1. Retired letterpress depth for two levels and no others — one for a thing resting on the page, one for a thing floating over it — with hover raising the background a step rather than translating into an impression, and nothing in the interface moving on hover. Replaced the single 2px radius with five steps from 8px to a pill, since a hard offset shadow was the only reason a sharp corner was required. Adopted Figtree and JetBrains Mono, self-hosted under the OFL, and retired Geist and Geist Pixel; raised the reading sizes so the agent's own words are 16px at 1.65 and nothing read at length is below 15px, which is what "squeezed" meant. Capped the conversation at a 760px centred reading column, gave the owner's turns a right-aligned block on `--inset` and the agent's turns the full column unboxed, so the two roles are told apart by shape as well as ink. Moved what is known about a session from a 412px bottom sheet to a 360px right drawer, because the sheet took its height from the surface it was describing. Reduced the session header to one primary, one secondary and an overflow menu, and moved End and Delete to the foot of the drawer. Made the command trigger a working `⌘K` plate over the surfaces it can already reach, rather than a disabled field in the middle of the bar. Collapsed tool calls to 40px rows carrying a status glyph, the kernel's own kind and target, a duration and a status word, and collapsed reasoning to one key stating that it is not kept. Replaced the conversation surface drawn inside a 420×280 canvas node — where the composer clipped mid-placeholder — with a recap of the last thing the agent said and a key that opens the surface which can hold a reading column, and gave the canvas an explicit trailing fade and pan affordance so a viewport narrower than the grid reads as pannable rather than clipped. Fixed one live defect in §11's wording rather than its rules: a session parked on `requires_action` is `idle`, and told its owner it was stopped while an approval it could answer sat above the composer; it now says it is waiting for them, in the rail, the header and the composer. Fixed the double hairline where a panel and its first band both drew a border on the shared edge, and added §5's rule that one boundary draws one hairline.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-19 | §18.2 records tool bodies, and says when it does not have them. A tool call's row now discloses what it was asked to do and what it produced when the session was granted the recording, drawn from the durable log — never estimated, never reconstructed from the terminal. A body cut to fit the log's bound is marked `(truncated)` rather than silently shortened, the same rule the section already holds for output: disclosure is a frame decision, dropping the tail without saying so is not. Recording tool bodies makes §18.7's "it never renders a credential" non-trivial for the first time, so the core redacts the values it handed the session before a body is written, and the surface renders nothing it was not given.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-19 | Added §18.10 A stage and a mode change. The conversation surface now says what the agent is at and what it is waiting for, because the kernel already said so. A stage is the agent's plan drawn as structure rather than prose — a `--sunken` list of steps, each the kernel's own words verbatim, a full revision that replaces the one before it — and it sits above the in-flight content, since a plan is what the agent is doing now rather than a thing it said. A mode change is a safety fact about the session, drawn as a mono `--text-label` `--ink-3` line carrying the kernel's own word, never mapped onto a set this document invented. Both ride the durable log through `STATE_DELTA`, so a reload shows the same stage and the same mode as a turn watched arriving, on §18.1's equivalence rule.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 2026-08-19 | Amended §18.6 for a plane change that keeps its sandbox. A plane change that replaces the pod still draws the boundary, because something really did not travel; a plane change that restarts the program inside the same pod draws no boundary, because nothing moved. A boundary drawn where nothing was lost teaches an owner to distrust boundaries that mean something, so the surface draws one only where the record carries a snapshot timestamp, and none where it does not.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 2026-08-19 | Wrote §18.10, which the changelog announced on this date and nobody wrote. The surface had been drawing a stage, a mode change and a kernel's command list ever since, which put all three outside this document's authority — the thing §7 exists to prevent, arriving through a changelog entry rather than through code. Written against what the kernel already sends: a stage is the agent's plan as a `--sunken` list of steps in the kernel's own words, above the in-flight content because a plan is what the agent is doing now rather than a thing it said, replaced whole on each revision since merging two would invent a plan neither described; a mode change is one mono `--text-label` `--ink-3` line carrying the kernel's own word, drawn where state is drawn and never in the reading column, because a reader scanning the agent's words must not meet a sentence the agent did not write. Moved the command list out of the stream, where it was printed below every turn and could be read but not used — the reader retyped what they had just read. Added §18.11 What a reader can take away: every fenced block carries a header with the fence's info string rendered verbatim and uppercased by `text-transform` only, since casing is typography and mapping `py` to `Python` is naming, which §18.2 and §18.10 both forbid; a fence with no info string draws no label rather than an invented one; the header is sticky against the stream's scrollport on a wrapper outside the `<pre>`, because the `<pre>` owns the horizontal overflow and a sticky child of a horizontally scrolling box slides away sideways. Every settled turn carries a copy control that writes the turn's **markdown source** rather than the rendered tree, which is the point and not an implementation note: selection-copy already works and already yields degraded text, because the renderer strips heading hashes, list markers, fences and backticks before drawing, so somebody pasting an agent's steps into an issue or its command into a shell gets something the agent did not write. Persistently drawn and labelled rather than revealed on hover, since §3 permits no hover-gated affordance and nothing in this interface moves on hover; disabled with its reason stated while the turn is in flight rather than removed, because copying half an answer is the thing to prevent and removal is the wrong instrument for it. Added §18.12 What an agent wrote: links drawn but carrying **no colour** — underline and cursor and nothing else, since §18.1 forbids colour on agent content without exception and a blue link is that exception arriving by habit — with §9's OSC 8 rule as the precedent for not deciding where an agent's link points; tables in the structure they were written in, scrolling inside their own frame rather than widening the reading column; nested lists keeping their nesting, because a plan with sub-steps flattened to one level is a different plan; blockquotes on a `--rule` left border in `--ink-2`; and a soft line break drawn as a line break, because an agent that wrote its steps on separate lines meant them on separate lines and joining them rewrites the shape of the answer. Added §18.13 When the surface cannot hear the session, whose finding is that the failure mode is **asserting liveness that is false** rather than failing to narrate the transport: while the stream is down mid-turn nothing but the run ending clears the in-flight state, so the surface goes on drawing the §11 wave and blinking the caret on a message that has stopped growing, and the one affordance built to tell a working agent from a dead stream is the one that lies. A transient reconnect says nothing, on the line the terminal plane already holds; the moment the link is not live the liveness claim is withdrawn, because withdrawing a false assertion is quieter than adding a true one and is correct at every point in between; and only a link down past a bounded threshold is named, once, where the turn is rather than in a band across the head. None of it is a failed run — §18.9 draws a run the agent could not finish, this draws a surface that cannot hear one, and conflating them tells an owner their work broke when their network did. Amended §18.2 so each body the log carried is its own region with its own scrollport under the existing cap: what a call was asked to do and what it produced are two bodies, and one window with one scroll position means a long argument payload pushes the result out of sight, so the reader scrolls past what they already knew to reach the thing they opened the row for — which is the scrollback the section exists to replace. Amended §18.4 so the kernel's commands are reachable from the field: `/` at the head of an empty draft opens a filtered list built from what the kernel advertised and nothing else, reusing §3's existing plate behaviour, and a kernel advertising none opens nothing. They stay liveness rather than history, which is why they may not be drawn in the stream: a stale list replayed from the log would offer commands that no longer exist, against §18.1's equivalence rule. |
| 2026-08-19 | Corrected §18.12's blockquote rule before it shipped. The section as first written gave a blockquote `--ink-2`, which §18.1 forbids without exception — "no colour is applied to agent content, ever" — and which the test enforcing that rule caught on the first implementation. It takes the `--rule` left border and no ink of its own. The reasoning is the section's own: quoting is the agent's emphasis, and drawing it fainter than the words around it is this surface deciding the quote matters less than the agent did.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 2026-08-20 | **Harness**: full redesign of the frontend's visual language, replacing the Quiet instrument system of 2026-08-18. The language is adopted from DeepSeek Harness (MIT licensed) and implemented in our own terms, because our surfaces are not theirs — a terminal plane, a canvas of many sessions and an operator panel have no equivalent there, and §20.9 derives those from the language rather than leaving the old look in place. Four decisions carry the rest. **The palette is flat in light and stepped in dark**: every background layer is the same white in light, with hairlines and shadow doing the separating, while dark steps by luminance because a black shadow is invisible on a dark ground — so a card declares both a hairline and a shadow and each does the work in one theme, one rule with no media branch in the component. **The accent is no longer the primary**: primary is near-black on white and near-white on black, and the accent is demoted to status — a dot, a selected tab, a focus ring — never the call to action. That removes the surface's loudest element and puts the emphasis on type and weight, and it means the lime that had been the thing you press is gone. **Two type scales sharing sizes and not leading**, tight for interface text and loose for anything read at length, because treating a 14px label and a 14px paragraph alike is what made long answers hard to read; four weights and no others, since a non-variable face snaps an intermediate weight unpredictably. **The agent's work is drawn as typed cards** rather than one generic row with a blob in it: a command and its output take a terminal card, a file edit a diff, structured data a tree, each with a 24px header carrying the kernel's own words. Specified but not yet buildable in full, and said so rather than claimed: AG-UI carries a body as text and does not say what shape it is, so until the core carries that, a card is chosen only by what can safely be detected and falls back to the terminal card — §9 forbids claiming a structure that was inferred wrongly. Also: hairlines are 1px everywhere with interactive capsules taking half their height as radius and containers a fixed step; hover, active and selected become alpha tints rather than solid greys, so the same hover reads the same over a card and over the page; the app bar and status band are retired into the rail and session header, which were saying the same things twice; the interface webfont is dropped for the system stack while the mono face stays, since code and terminal output cannot be left to platform variance. Three things are deliberately **not** adopted. Focus stays ours — the source has no focus treatment worth taking, relying almost entirely on a `:focus-within` border change, which is kept on top of our ring rather than instead of it. Theme switching stays ours, declared on `:root` with the three-state contract that survives with scripting disabled, rather than the source's inline script and body attributes. And every behavioural rule §19 protects is untouched, along with §18's equivalence rule, absence-is-not-zero, no colour on agent content, refusals naming what is missing, and the session-state meanings — a restyle that changed any of those would not be a restyle.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-20 | Rewrote the document to the design language adopted from deepseek-harness (MIT licensed), per #317. This replaces the prescriptive content of the Quiet instrument system and of the partial Harness revision written earlier the same day; both are superseded rather than amended, and their changelog entries stand as the record of what they were. The language itself: **three token tiers** — static ramps that do not change between themes, aliases carrying roles, and specific tokens naming one surface each — with a component reading past the alias tier treated as a defect, because that is how a surface ends up unable to change theme. **Tokens are declared on `body` and dark is an attribute on `body`, never `:root`**, which is load-bearing rather than stylistic: the scrollbar sheet resolves against the element the tokens sit on, and an `html`-scoped rule cannot see them. An inline script before first paint resolves the preference, so the wrong theme is never shown and corrected. **Brand primary is a neutral and blue is not the primary** — blue means running, current, send, and an interface whose main action is the same colour as its status cannot distinguish them. **Typography is a composite ramp**: one name carries weight, size and leading together so a step cannot be half-applied, with a `-strong` companion only from the base step downward, since the three largest steps are already emphatic and a heavier version of them is a size rather than an emphasis. Four weights and no intermediate, because a non-variable face snaps one unpredictably. **Depth is per theme in one rule**: a floating surface declares both a shadow and a hairline, and each does the work in the theme where the other cannot — no component needs a media query. **Spacing, radius and z-index get no theme tokens**, only literal pixels per sheet from fixed sets, which is this language's own idiom: a spacing scale invites a component to take the step above when the one it needs is missing. Also recorded: scrollbars drawn only while pointed at; syntax colours adopted whole rather than reassembled from the ramps; icons inlined as components painting with `currentColor`, no library; a three-column frame with a stated concession chain in which details closes first, the sidebar never concedes and the centre absorbs the rest — which is §1's rule that the surface owns the leftover width, restated in geometry. The conversation surface keeps its 748px axis and tells the two roles apart by shape rather than ink, which is what lets §1's fidelity rule hold: no colour is applied to what the agent wrote. An action the API does not project is omitted entirely rather than drawn disabled — a disabled ghost for something that will never work is the absence rule broken in a costume. **§1's four binding consequences are untouched**, as are accessibility, the session-state table, and the surface sections for the two arrangements, operations and the signed-out state: those carry behaviour rather than style, and an earlier draft of this rewrite deleted them and then referred to a table it had removed. They are restored and renumbered. Nothing in the changelog was edited.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 2026-08-20 | Expanded §12 Controls from the button alone to the whole control set the dsh adoption introduced, written after the controls were built rather than before — which is the wrong order, and is recorded here as such. Added §12.1 badges and state: the pill's span-until-interactive rule and why (a clickable span is unreachable by keyboard; an always-button announces every static label as pressable), selection as an inset ring so moving it along a row does not shift the row, and the state dot as reinforcement that is never the signal. Established that _running_ is not a colour but a chase, because it is a claim about right now and a still dot cannot say whether it is still true, and that a finished state draws no dot at all — every state the dot has asserts something a finished session does not, and green would claim it finished well. Added §12.2 fields: the border on the wrapper rather than the input, which is what lets a glyph sit inside it and why a field with an icon and one without focus identically, and the explicit exclusion of the composer, which grows and would make every search box growable if it shared this. Added §12.3 the dropdown, now one control for choosing a value and choosing an action, where before they were two and only one had a keyboard route: arrow/Home/End with wrapping, focus landing on the selected row, `aria-disabled` rather than `disabled` so an unavailable row keeps its place in the keyboard order (the reason for listing it is that the reason can be read), `menuitemradio` for a menu carrying a current value, selection as a trailing check because a fill would make the selected and hovered rows the same colour, and edge handling that nudges rather than flips so the list still reads as belonging to its control. Added §12.4 dialogs: one card, a mask that blurs as well as dims, portalling out of the page because a stacking context leaves sticky chrome above the mask, closed meaning unmounted because a mounted trap steals focus, and the acknowledgement pattern for an action that loses something — the tick is the safeguard rather than the colour, spent only where the consequence is loss. Recorded that there is no destructive button variant and why: a red key is one a person can click as fast as any other, so it was never the safeguard it looked like.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 2026-08-20 | Amended §18.3 on where an approval waits, and §14 on how the terminal is framed. **The approval takes the composer's seat rather than sitting in the message stream.** The section's binding constraint is unchanged and still holds: it is never a modal, so a session parked on an answer leaves every other session reachable — this sits inside one session's own surface with the sidebar untouched. What changes is that an approval in the stream _scrolls away_: a turn that stopped five entries ago leaves its question five entries up, and the one thing somebody must answer becomes the one thing they can scroll past. In the composer's seat it is where they were already looking, and it replaces the field rather than sitting beside it, because typing is not what the session is waiting for. Its border takes the warning tone rather than the business one — blue reads as information, and this is a decision with a cost. The sign-in card stays in the stream, deliberately: it is a thing to go and do somewhere else, and a person can leave it open while they read. **The terminal takes the same card a code block takes** — the code-block ground, a 12px radius, an l1 hairline — because it is the same kind of content, a program's own output in its own font, and two treatments for that made the two planes read as two products. Its type becomes the product's own code stack at 13/22, matching the code blocks, with one documented exception: the terminal stack alone keeps a bare `monospace` tail. Everywhere else that keyword resolves to a CJK serif on Windows and wrecks the column widths this stack exists to hold; here, a terminal with no font at all is not a terminal, so a last resort has to exist.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 2026-08-20 | Inverted how motion is gated, which fixes a real defect rather than restyling one. Micro-transitions were wrapped in `@media (prefers-reduced-motion: no-preference)`, which matches only where somebody has _explicitly expressed no preference_ — a platform that does not report the feature at all matches neither value, so every hover fill, every caret and every focus transition was silently absent for those people. That is the opposite of what the gate was for. Motion is now the default and a `reduce` block removes it, which is the idiom that gets this right, and two tests pin it: no sheet may gate on `no-preference`, and every sheet that starts an `infinite` animation must say how it stops. Replaced `--motion-colour` and `--motion-press` with the adopted `--ds-transition-duration*` tokens and the single `--ds-ease-in-out` curve, with a third test forbidding a hand-written `cubic-bezier` in a component sheet — a fourth curve nobody chose. Added the reasoning row's sweep: a band of lighter ground crossing the row on a 2.6s loop, which says the row is live without claiming progress, for the same reason §5 refuses a spinner — a bar that fills implies somebody knows how far along it is, and nothing here does. It rides on the row rather than on the text, so the agent's words stay at full contrast while it passes, and it stops entirely under `reduce`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 2026-08-20 | Corrected §2's description of the background ladder, which said the opposite of what the sheets do and of what the language it was adopted from does. It read "in light they are white and near-white"; `bg-layer-1/2/3` are **all the same white as `bg-base`** in light, and step only in dark. The implementation was right and the document was wrong, which is the worse way round: §7 makes this document the authority, so a reader checking the sheets against it would have concluded the sheets had a bug and "fixed" a ladder into them. Found by measuring a pill in the browser, seeing it draw white on white, and going to the source before changing anything. Recorded the consequence beside the rule rather than leaving it to be rediscovered: a surface distinguished only by `bg-layer-2` has no visible container in light, so a pill is its text there and a filled capsule in dark, and anything that must read as a container in both themes carries a border rather than a fill. Nothing in the sheets changed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 2026-08-20 | Amended §10 for the sidebar foot and §12.3 for where a menu is drawn. **The foot is the account row and nothing else**: an avatar, the signed-in name and the address beneath it, because on a service that spends the owner's own vendor subscription which account is in use is the fact most worth checking without opening anything — it was a 32px avatar whose only text was a screen-reader label, so the answer was reachable only after clicking. Both lines truncate rather than wrap, since the sidebar is resizable and an address is arbitrarily long. Operations moved off the foot and into that row's menu: it had been the app bar's segment until §9 retired the bar, then a row of its own, which left the foot a two-item navigation strip beside an account control that omitted the one place an operator goes. It stays one entry that **names where it goes** rather than marking itself current, because a menu carrying a current value is a set of alternatives and these rows are actions. Recorded in §12.3 that a menu's list is **always drawn on the body** rather than in place, which is the half of the existing nudge-back rule that was missing: a list rendered inside its trigger cannot be nudged anywhere, only clipped by whatever scrolls above it, and every list drawn in place turned out to be drawn wrongly somewhere — cut off inside a dialog's scrollport, opened 53px past the left edge, run off the bottom of the window. A trigger cannot know what encloses it, so the in-place default asked each caller a question none of them could answer. Also recorded: closing returns focus to the trigger when focus was still in the list, since otherwise the next Tab restarts at the top of the document; and a list may be held to its trigger's width where the trigger is a row, which has to be measured with the position because a list on the body cannot take a width from its anchor in CSS.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| 2026-08-20 | Retired the plane strip from the session header, per #370, and stopped asking a profile which plane it wants. **A session is a conversation.** The terminal is where its owner moves it, deliberately, from the overflow menu. Two tabs made a rotation look like a view switch — free, instant and reversible, which it is none of — and the profile form put the question before anybody had seen a session, which is the worst moment to ask something whose answer costs a rotation to change. Recorded in §14 that the plane is reached rather than chosen, and that the entry names the **act** rather than the destination: _Interact directly with terminal_, and _Return to chat mode_ while it is there, because "Terminal" beside "Conversation" reads as two views to flip between. It confirms in a card in the middle of the page rather than inline in the header, which is a change of instrument and not only of position: the inline confirmation took over the row's own controls, which was quiet enough to be missed and put the question somewhere the eye was not, for the one entry in that menu that costs something. What the card says is unchanged and was already required — the session restarts on the other plane, it is not interactive while it moves, and the turn in flight is lost — because none of it is recoverable by pressing the key again. The refusal moved with the question: a runtime that cannot hold a conversation is now named under the **agent**, which is the only choice left that can produce one. §11's header rule and §18.6's boundary rules are otherwise unchanged, and the API still serves both planes — this is what the interface offers by default, not what the service supports.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 2026-08-20 | Grouped the session rail by state and renamed the pause, per #381. Three groups — **Active**, **Paused**, **Terminated** — each collapsible, Active open and the rest closed, because a rail carrying forty finished sessions buries the two somebody is working in and those two are why they opened it. The filter is retired rather than kept beside them: choosing "stopped" and collapsing everything but Paused are the same gesture done twice, and carrying both let the rail hold two states that disagreed — a group expanded inside a filter that excluded it. Order is unchanged and applies across the whole rail, sorted once and then split, so three groups cannot disagree about it. Recorded that a group's header is the control rather than a chevron beside it, that it carries its count so a closed group still says how much is inside, that an empty group draws a zero rather than vanishing — a group that disappears is one somebody looks for — and that the header is **named explicitly for a screen reader**, because the label and the count concatenate and it read as "Paused1". The groups share one scrollport: three inside a 280px rail is three thumbs and three places to be lost. Separately and more importantly, **a pause is now called a pause**: the control labelled _End session_ calls `stopSession`, which releases the sandbox, keeps the history and lets its owner pick the session up again — so the label said an act was final when it is not, and the rename is a correction rather than a preference. It is also what makes the Paused group nameable, since a group has to be called after the thing that put sessions in it. _Revive_ becomes _Resume_ for the same reason. **Delete is untouched**, keeping its acknowledgement tick: it is the one action that loses something.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 2026-08-20 | Added §14a Motion, per #375, written before the surfaces adopt it rather than after — a set of transitions invented per component is the drift §7 exists to stop. Records what motion is **for** here and nothing else: saying a surface changed, and saying a surface is working. A surface replacing another fades, with no movement, because a slide implies a direction and there is no direction between two sessions. A surface that is waiting says so and never claims to know how long, which is §5's existing refusal of a filling bar restated as an idiom — the reasoning row's sweep, saying _live_ without claiming progress — and a surface reading something draws the shape of what is coming rather than a spinner in an empty column. Rows arriving fade without moving position, since a row that slides in pushes everything under it and a list that reflows while somebody reads it is worse than one that appears. The existing rules are restated where they belong rather than left implicit: one curve and three durations from the tokens; motion the default with `reduce` removing it, never `no-preference` adding it, because a platform that does not report the feature matches neither value and would lose every transition; nothing moving on hover; and nothing animated on the agent's own words — a turn may fade in, its text does not animate as it is read.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| 2026-08-20 | Renamed the product to **CMA** — ChronoAI Managed Agent — on every surface a person reads, and rewrote §20 and §21 around the brand mark that arrived with it. Discarded the signed-out surface: it was a simulated terminal typing a scripted boot log beside a statement column, and the pretence was the defect — everything on it was fake except the input, so the one real affordance was the least conspicuous thing on the page. What replaced it is the animated mark at `clamp(200px, 52vmin, 560px)`, centred in the viewport, the wordmark, the expansion, and a prompt with its sign-in key on one line. The prompt is kept, and kept real: what a visitor types still survives the sign-in round trip and becomes the first turn of their first session, which is the reason the page has a field rather than a bare button. The mark is cropped to 100:60 with `object-fit: cover` rather than drawn whole — its artwork occupies y 23-77 of a square viewBox, so a third of the file is empty and drawing it whole spends a third of the mark's height on nothing, which is what "not as big as it should be" turned out to mean once it was measured in a browser rather than reasoned about. The animated file shipped with no animation in it: a `clipPath` named `lenses` and a white rectangle sized to sweep them, with nothing to move either. Written as CSS inside the file rather than SMIL, because CSS honours `prefers-reduced-motion` and SMIL cannot read it, and under that preference the mark is shown finished rather than slowed. The highlight is a gradient, not the flat white bar it was drawn as — a hard edge crossing the lenses reads as a rendering fault, which is how it looked when it was one. Chrome takes the still mark, so the logo does not replay its entrance on every navigation. Stored keys keep their old spelling on purpose — the terminal theme id and the arrangement preference are what a returning person's browser already holds, and renaming them would silently discard settings to no visible end.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 2026-08-20 | Corrected §20's crop, which was cutting the mark. The band was written from `getBoundingClientRect`, which reports **fill** bounds and excludes stroke, so the arc's apex was read as 19.58 when the painted edge is 14.08 — half of an 11-unit stroke sits above the apex. The crop at y 20-80 therefore sheared six of those eleven units off and left the crown of the arc a flat horizontal line, on the one element the signed-out page is built around. It was visible in the screenshots taken at the time and read as the shape of the logo. The band is now y 11-80, clearing the artwork by about three units at each end and centred on the artwork (45.29) rather than on a viewBox whose padding is not symmetric. Added a browser gate for both this and the reduced-motion defect beside it: `check:styles` now renders the signed-out page in both motion modes and fails if any part of the mark is unpainted, if the crop overruns the artwork at either edge, or if the mark is an `<img>` rather than inline — through an image the mark's own reduced-motion rules are inert and nothing outside can see in. Both failures were reproduced against the gate before it was kept, because a gate that has only ever passed guards nothing. Also corrected the three landing files, which cited §15 for the signed-out surface when §15 is Accessibility and the surface is §20, and the comment in the live lane's projection, which still asserted the command list "never rides the log" after the log became where it actually reaches anybody.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 2026-08-21 | Rebuilt the session details drawer in §17 from a flat list of eighteen labelled rows into six titled bands, because a list that says nothing about which facts belong together is read by finding one fact at a time. Put the state word at the head at `--text-title` weight 600 with the start time and the creator under it, on a 3px left edge in the session's own state tone, so the first thing read is what the session is doing. Moved facts from stacked pairs to a two-column grid at `minmax(72px, 34%) 1fr` and inverted the ranking the previous revision had backwards — labels now 12px `--ink-3` beside values at 14px `--ink`, where before a column of labels sat one colour rank from its values and competed with them. Added a sentence beneath any value that names a setting rather than describes one, since "Guarded" is the name of something nobody chose and cannot look up from the panel. Added the counts beside band titles holding lists. Added four facts the panel never carried: the model, the reasoning effort, the permission mode, and whose account the model calls are billed to — ChronoAI managed against the owner's own vendor account — plus the skills installed into the sandbox, which required recording the grant on the session rather than reading it back from a profile that may since have been edited. Established the rule that **the drawer spends colour once**: the state band and a value carrying a fact of its own, today an unrestricted permission mode, and nowhere else. That is a constraint rather than a preference — `--live`, `--wait` and `--fail` already mean something under §12.1, and tinting a band per section would leave no way to say that the one row which genuinely matters differs from the four around it, so bands are separated by rule, rank and rhythm instead. Made the runtime band disappear when it holds nothing, because two empty rows under a title read as a section that failed to load.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 2026-08-21 | Set the wordmark in chrome the way the lockup sets it, in §21. The sidebar's CMA was weight 600 at **+0.04em** — positive tracking, which holds letters apart and is a treatment for small caps and label runs, not for a three-letter product name at reading size. It is now 700 at `--track-display` (-0.035em), against the shipped lockup's own -1.6 on a 54px face (-0.030em), so the name in chrome and the name on the lockup read as one wordmark rather than as two that happen to spell the same thing.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-21 | Gave the chrome mark a size in §21, which had none. It was 24px in a 60px logo row with a 44px content box, so it sat in more than its own height of empty space and read as an icon that happened to be beside the name rather than as the product's mark; it is now 32px. That one size serves both rail states — it still clears the 36px square the collapsed rail gives the brand button — so no second rule exists for the closed case, since a second rule is how the two drift.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 2026-08-21 | Added a one-line caption under a band title in §17, and put it on Reaches only. The band lists slugs like `api-github-pat`; its count answers "how many" and its entries answer "which", and nothing answered "of what" — a reader had no way to tell that those were services on the identity platform rather than something this product invented. Set at `--text-meta` below the title and above the content so it reads as a caption on the band rather than as its first entry, and stated the rule that most bands do not get one, because a caption on every band is a caption nobody reads. Also corrected §17's claim that band titles are `--ink-3`: nothing in the drawer sits on that token, for the contrast reason recorded on 2026-08-21 and tracked in #441.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2026-08-21 | Let the signed-out mark bob once it has finished drawing, and carved the exception into §14a rather than leaving the sheet contradicting the rule. §14a said motion is for saying a surface changed and saying a surface is working, and that neither decorates — a looping bob is decoration, so the doc had to change deliberately or the code would have been quietly wrong. The reasoning recorded: that rule is about a working interface, where motion meaning nothing competes with motion that means something, and the signed-out screen has no state to report, nothing running and one object on it. Bounded so it does not become a licence — signed-out mark only, up and down and nothing else, never before the drawing finishes, stopping at rest under reduced motion, and on the one curve from the tokens. Put the animation on the crop rather than the artwork, because moving the artwork inside a wrapper that clips it shears the arc's apex off at the peak of every bounce; moving the wrapper takes the window and its contents together, measured at 14.2px of travel with the arc's clearance constant throughout. The lens highlight is unchanged.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-08-21 | Made each skill in §17's Skills band a link to its own page in the registry, opened in a new tab and labelled `name@version`. A session is a live thing somebody is working in, so the link opens away from it rather than navigating it off the surface they were using, and `target="_blank"` carries `rel="noreferrer noopener"` so the opened page holds no handle back. The version sits with the name as one label — "this session has nyxid" is not actionable without knowing which nyxid — and is omitted rather than drawn as an empty `@` when the registry reported none. Underlined on hover only, so a band of entries does not read as a paragraph of links, and the version is tabular and quieter than the name so a column of them lines up.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 2026-08-22 | Added §13's context fullness and summary boundaries. The footer now distinguishes four facts instead of turning absence into zero: no context line where usage was never granted, `Context · waiting` before a granted session reports, a bare reading with no track where no ceiling was stated, and a reading against its stated ceiling with a neutral 2px progress hairline and complete progressbar semantics. Fixed the track at no more than 240px so the bounded reading stays a quiet instrument beneath the 748px conversation column rather than becoming a second page-width progress surface. A fall in the reading is now drawn where it happened as a neutral transcript boundary, in a sentence that says the agent summarised the conversation to make room and always carries both the previous and current readings; the ceiling is included only where known. Reserved warning, failure, and session-state colours from that boundary because making room is not a fault, and prohibited the bare internal word "compaction" as an explanation to the owner.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-08-23 | Added §19's platform-settings editor before implementing #450. Settings is the fifth capability-gated operations view and the only one that edits rather than lists. It is an unframed form capped at 920px, split into full-width sections by hairline, title and rhythm rather than nested cards. The standing-instructions document stays visibly assembled: repeatable prose blocks before and after, four individual fields around the generated grants, and a fixed generated-grants row with no control capable of removing it. Required the notice to say both facts the form's shape cannot — only Codex reads the document today, and clearing platform prose removes the already-authenticated and `nyxid login` guidance. Reset is one action and briefing-only; loading the previous object changes only the draft. Optimistic conflicts keep that draft, read the new revision, and explicitly offer loading current values or reviewing and repeating the write, never an automatic last-write-wins retry.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| 2026-08-23 | Added §13's quiet, dismissible runtime-loss notice. It sits at the head of the reading column above any refusal it may explain and above the message stream, and it uses status semantics and the quiet notice rather than failure ink because reconciliation starts a replacement and §16 says a session being restored is not a failure. Required the time to say when the service noticed the loss rather than claiming when the runtime died, with no time invented where none was recorded, and required the notice to say plainly that the agent conversation does not cross the loss and begins again from what was captured.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 2026-08-23 | Added §12.5 for a dialog that asks in steps. The card replaces its own body instead of opening another dialog, keeps one width throughout, uses a secondary Back key and the ordinary primary action to proceed, and states an accepted rate without inventing a projected total.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 2026-08-23 | Added §13a for a session's live resources and memory pressure. Readings sit at the foot of the conversation column beside usage, while an actionable pressure episode sits at its head as a still, dismissible failure notice. Memory and CPU receive a neutral 2px progress hairline only when the service supplies a ceiling; unbounded network figures and bounded readings without ceilings carry no track, so shape preserves the difference rather than inventing a limit. Fixed the exact language for an ungranted capability, an older runtime that cannot report, a reading still awaited, a missing figure, and the first CPU rate that is still measuring, with absence never rendered as zero. Required progressbar semantics and visible pressure words so colour is never the only account, and allowed one fade at final position while forbidding flashing, pulsing, keyframes, and loops because urgency is already carried by placement, words, and failure ink.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 2026-08-23 | Replaced the Harness visual language with Inkwell for light and Lumen for dark, taking the palette, type voice, warm-paper depth and bounded lime glow from `/Users/chronoai/code/work/inkwell-ui.html` and `/Users/chronoai/code/work/lumen-ui.html`. Reversed five doctrines deliberately: primary controls now use the theme accent instead of a neutral; shared controls adopt 40/32px heights and 10/6px radii; the unchanged CMA geometry is re-inked blue or lime with the appearance instead of fixed blue; self-hosted interface webfonts return as Familjen Grotesk/IBM Plex Mono and Archivo/JetBrains Mono; and success is asymmetric, moss green in light while Lumen's lime is both accent and live green. Kept the three-tier token architecture and every consumed alias and specific name while replacing their values, because stable role names let the palette land before each surface restyle without a second migration vocabulary; retired only the malformed, grep-proven unused `brand-primary-new-colorprimary-new-color` alias and retained the deepseek-harness licence for the still-vendored icons.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-23 | Clarified §4's code fallback stacks while implementing #527: light keeps JetBrains Mono and Fira Code behind IBM Plex Mono and SF Mono, and dark keeps Fira Code behind JetBrains Mono and SF Mono. These are fallbacks rather than additional theme faces; preserving them keeps the existing platform coverage while the leading family remains authoritative for each appearance.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 2026-08-23 | Corrected §20's wordmark and mark-depth contract while implementing #534. The themed display convention already fixed in §4 had not reached §20's table: the live wordmark now states Familjen's 700 in Inkwell and Archivo's 900 with `--ds-font-variation-display` in Lumen, while retaining its existing size and tracking. Named the light mark's warm depth as `--dsw-shadow-lv1-blur` and made the already-specified dark `--dsw-glow-accent` treatment affirmative, so the implementation chooses no unstated shadow value.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-24 | Added §12.4's treatment for a deployment-capacity refusal: the new-session dialog uses its existing alert seat, states requested and free CPU and memory, and leaves Start available because shared headroom is learned only by asking and a disabled or retained capacity state would be stale.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-08-24 | Renamed the frontend design authority from chrono-bot to CMA and aligned its machine-name examples with `cma`; compatibility-bound browser storage keys remain unchanged until their read-old/write-new migration.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| 2026-08-24 | Completed #477's browser-state migration without resetting returning users: five preferences and pending content now write only `cma:*` keys, valid historical `chrono-bot:*` values are read canonical-first and copied forward, the pre-paint appearance path follows the same rule, and the product terminal ids migrate from `chronobot-*` to `cma-*`. Pending content remains per-tab and clears both names when consumed so an old prompt cannot replay.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 2026-08-24 | Replaced §20's large animated SVG seat with the supplied live Three.js character while keeping the prompt interactive on the first frame. Defined a stable unframed square, theme-derived body, lens, studio and shadow colours, a still theme-inked fallback when WebGL cannot start, live appearance updates, visibility pausing and complete disposal. Bounded its float, pointer look, trailing hem, lens glint, contact shadow and press-to-spin under §14a, with reduced motion making every part still. Moved the existing animated SVG into the authenticated sidebar: it draws once when that shell mounts and keeps its quiet lens sweep, while its wrapper never inherits the former landing bob and session or view changes never remount it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| 2026-08-24 | Added §19a for the signed-in Developer surface. Admitted Developer as the third transient view without adding a route; fixed its account-menu return wording, session-selection return, header, scroll ownership, 200px desktop document rail, 748px unframed reading axis, 48rem narrow selector and two-part SDK control. Defined the deployment preface as the only non-repository prose, naming the current same-origin API base and the stated operator or identity-administrator path while credential issuance is not self-service. Required both eight-guide SDK sets to be bundled and validated at build time, with symmetric filenames, unique nav metadata and leading H1s. Reused the conversation markdown pipeline under a document policy with true heading ranks, stable duplicate-safe anchors, GFM tables, existing code treatment, safe external links, bundled-document switching, no raw HTML or images, and no runtime empty state for an invalid bundle.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-08-27 | Added §13b and amended §§1 and 9 for the CMA Trigger workflow. The binding route set now covers public preview, authenticated review, durable Launch progress, contribution review, and session deep links without making navigation a side effect. Defined the repository/progress/diff/attribution/PR bands, closed progress and mergeability language, workflow acknowledgement, provider-metadata preservation, offline cursor recovery, mobile and assistive-technology behavior, and the public-interface split: AG-UI read/replay companion projections, REST/SDK mutations through the shared idempotency store, and explicit absence from PTY, MCP, and A2A.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 2026-08-27 | Extended the Developer documentation bundle from eight to nine symmetric guides so the public Trigger and Launch SDK workflow is available in both TypeScript and Rust. The existing build-time rules remain unchanged: matching filenames, one leading H1, one nav record, and unique order and label metadata are still required before the frontend can build.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 2026-08-27 | Added §19b and amended §§1 and 10 for the authenticated GitHub Trigger publisher. Admitted Publisher as the sixth view without adding a persistent route; defined deep-route return, the account-menu action, desktop and narrow draft navigation, exact NyxID UserService selection, bounded popup OAuth preflight, canonical repository verification, typed unframed policy editing, fixed MVP controls, optimistic concurrency and idempotent reconciliation, immutable publish confirmation, revision copying and retirement, retained failure state, untrusted-text rendering, and keyboard, focus, live-region, reduced-motion and narrow-target behavior.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-08-27 | Amended §§12.4 and 13b for explicit ambiguous initial-task recovery. Bounded the control to the exact Launch, blocked-reason, delivery-state, and retained-payload conjunction; required plain disclosure that CMA cannot establish whether the previous task started and that another dispatch may execute it twice; reused the acknowledgement safeguard as the one non-loss exception; and fixed ambiguous browser outcomes to a mounted, non-dismissible, same-key reconciliation state with frozen revision and payload authority.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 2026-08-28 | Amended §13b to record the existing owner-only `/launches/:launchId/recover` route in the binding route vocabulary. The route restores retained GitHub authorization state after sign-in and does not itself perform a provider write.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| 2026-08-29 | Amended §13a so sustained CPU pressure receives the same durable, still warning treatment as memory pressure. Kept the episodes independently dismissible and allowed only the affected track to spend failure ink, so simultaneous pressure remains explicit without turning an absent CPU rate or limit into a healthy zero or an invented percentage.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 2026-08-31 | Amended §12.2 for environment-profile session sizing. Kept the deployment reservation as an explicit checked default while leaving disabled CPU and memory fields visible, named session resources in profile summaries, enabled both figures only as one complete custom pair, retained that pair across an accidental toggle, and fixed the fields side by side until the existing 30rem compact breakpoint stacks them.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 2026-08-31 | Amended §§13 and 13a for compact live session instrumentation. Fixed tokens and context into one two-column row, fixed memory, CPU, and network into the three columns below it, kept CPU in millicores and bounded memory in MiB so both limits use the same vocabulary as session sizing, and retained wrapping inside each column at compact widths rather than stacking five full-width readings beneath the composer.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 2026-08-31 | Amended §19 for the separate operator Chat settings view. Added capability-gated access through the public admin settings contract; fixed the seven-section unframed form, ordered message and setup editors, retained deployment-default host sizing, mirrored client validation, revision-CAS conflict recovery, and the shipped-off master switch. Clarified that the platform reset restores shipped briefing prose and removes the optional Chat object while preserving all sealed and capacity policy.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| 2026-09-01 | Amended §10 for the capability-gated Chat / Agent surface switch. Fixed its two-segment geometry and distinguished it from a session plane change; made the brand and primary create actions follow the selected surface; specified direct New Chat creation; and defined the Active / Paused chat rail, generated-title fallback, last-activity line, and strict separation from the unchanged Agent rail.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| 2026-09-01 | Amended §19 for administrative chat-host inventory and retirement. Added the capability-gated Chat hosts record-list view; fixed each row to age, owner, written lifecycle, retained thread count, and host identity; kept retirement visible but separately gated by cross-owner control; required the shared inline risk confirmation and a post-action reread; isolated host confirmation and busy state from session controls; and made the seven-tab strip scroll while compact rows and confirmation controls stack without widening the viewport.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 2026-09-01 | Amended §13 for the Responses-backed Chat reduction. Kept the ordinary message stream while removing usage, context, resources, pressure notices, agent plans and options, top-level tool rows, and slash commands; placed live reasoning and durable tool activity together in one Thinking fold per turn with quiet kind-target-status-duration lines and disclosed bodies; retained neutral history boundaries and approvals; reduced the empty state to its title; fixed the header to Details alone; and fixed the chat drawer to a state-and-title band plus one Worktree band containing only the name and copyable local path.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| 2026-09-01 | Amended §13 for paused Chat. Defined the exact idle-plus-idle-timeout state; kept durable history and the composer available; placed the verbatim server-composed pause sentence in a persistent quiet status notice at the head of the reading column; fixed the paused placeholder; retained ordinary Responses admission; and required the existing Starting treatment to hold the draft until streaming progress or preserve it with the coded provisioning-timeout failure.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 2026-09-01 | Amended §19 for workspace setup guidance and operator-visible host failures. Fixed Setup guidance to uid 10001 without root, idempotent workspace-local installation, and the exact Lean fixture; added the retained typed failure phase and zero-based command index to chat-host rows; and placed the bounded output tail in a fixed-height preformatted scroll region while keeping configured commands and excerpts out of activity. |
| 2026-09-06 | Replaced retired Chat operator and GitHub contribution publisher contracts in §19 and §19b with capability-gated Sandbox/Profile/Activity inventory, revision-CAS platform settings, persistent CMA Trigger routes, the five-step profile/reuse/presentation/placement wizard, and descriptor-owned embed tabs. Retained existing design tokens and accessibility primitives. |
| 2026-09-06 | Amended §§1, 9–11, 12.2, 13–14, 16–18 and 19a for the Sandbox tree, Agent conversation and details, durable creation and public visitor journey, resource actions, versioned Profiles and Marketplace, persistent Developer route, and removal of terminal/session/canvas contracts. Public retry controls now require an actual supported operation. Historical rows and established palette tokens are unchanged. |
| 2026-09-07 | Aligned the remaining typography, motion, accessibility, authentication-gate and invariant wording with the implemented Agent client. Retired terminal fonts, simulated introductions, the removed 3D authentication scene and automatic first-message claims; preserved the static brand gate, durable-history equivalence, explicit visitor consent and historical changelog. |
| 2026-09-07 | Added four fixed light/dark badge-preview aliases in §2 using the established paper, ink and coal palette, so the publisher can inspect both appearances without switching the application theme or introducing component-level raw colours. |
| 2026-09-07 | Restored the pre-refactor Three.js mark, Thinking container, circular composer actions and compact public resource instrumentation on the current CMA hierarchy. Added state-coloured 3D provisioning marks and bounded external-click entry consent for no-review Triggers; preserved existing shared controls, durable history and append-only style history. |
| 2026-09-08 | Distinguished observed resource ceilings from explicitly labelled configured capacities resolved through the exact pinned Sandbox Profile revision. Added layout-aware conversation following that preserves manual reading position and removes completed preparation from the conversation surface. |
| 2026-09-08 | Kept the Agent grid within the available center width for long titles and Markdown tables, and enforced the established 34px/44px square composer control against shared icon-button width rules. |
| 2026-09-08 | Preserved an authorized transcript when initial event-history loading fails transiently. Reused the status band for incomplete activity and retry; retained drafts while holding commands and replay until all event pages are recovered, without masking authentication or permission denials. |
| 2026-09-08 | Bound Sandbox page and tree Pause/Resume controls to public lifecycle eligibility. Reused status seats for actual saving phases and start time, added visible nonoverlapping refresh while Active, and preserved the hierarchy with readable unavailable reasons after a failed refresh. Workspace and Agent creation rules remain unchanged. |
| 2026-09-08 | Added public Sandbox capture observation to the Agent composer. Preserved drafts and history while holding new messages and approval decisions during saving or unavailable lifecycle reads, kept ordinary paused-message revival, and distinguished pending admission from a durably active turn before offering Stop response. |
| 2026-09-08 | Made long Agent history progressively readable after the first authorized activity page. Reused the status band and preserved editable drafts while holding commands and replay until all history pages load, without skipping tool or approval activity. |
| 2026-09-08 | Bounded sidebar refresh fan-out by reusing unchanged retired Sandbox subtrees between minute rechecks. Kept authorized inventory and mutable descendants fresh, invalidated removed or changed resources, and cleared cached hierarchy on authentication or visibility denial. |
| 2026-09-08 | Corrected conversation geometry against the actual pre-refactor baseline: restored the composer's 12px internal gap, both metrics rows' 12px gaps, Thinking's original spacing, and 34px mobile Send/Stop circles within transparent 44px hit areas. Explicitly retained readable caption ink for Thinking and metrics and applied it to the placeholder rather than restoring low-contrast tertiary/dimmed text. |
| 2026-09-08 | Browser contrast measurement found caption ink on Thinking's preserved light layer-2 ground is only 4.29:1. Used existing secondary ink for the body and tool details, preserving the original ground and geometry while bringing that text to 5.92:1; retained caption on the composer's lighter input ground and base-ground metrics. |
| 2026-09-08 | Shared the Agent's fresh Sandbox lifecycle observation with live resource measurements. Stopped runtime-resource polling before active-runtime confirmation and while paused, retained usage and editable drafts, showed explicit paused metrics instead of a misleading unsupported error, and reset sampling on activation or runtime-version changes. |
| 2026-09-08 | Made inactive Sandbox hierarchy unavailable until activation, with state and Resume in its detail panel. Added persistent deletion progress and same-request recovery, explicit deletion states, terminal sidebar removal, and written deletion prerequisites while preserving retained history URLs. |
| 2026-09-08 | Made the sidebar brand a Home link, separated the New agent tool, moved the build identity into a compact adjacent badge, and gave the resource tree all remaining column height. Home clears details and shows Welcome to CMA with the existing 3D mark rotating continuously while honoring reduced motion and visibility. |
| 2026-09-08 | Applied shared step-character motion across the product: yellow pending turns once per second, green completed jumps once every two seconds, with stable geometry, visibility pausing, still fallback and reduced-motion equivalence. |
| 2026-09-08 | Simplified the owner Sandbox page around status, contents and agent access; moved technical facts and event reads into Advanced. Standardized original-name profile, NyxID service and Ornn skill links with owner-scoped resolution and canonical destinations. |
| 2026-09-08 | Made the signed-out logo the Get Started control after one lens sweep, with a complete turn and opacity handover into sign-in; renamed its heading ChronoAI Managed Agent and removed the standalone sign-in button. Preserved all signed-in character behavior and accessible reduced-motion and static fallbacks. |
| 2026-09-08 | Exposed shared plain-language Sandbox operation progress on the Trigger journey and Sandbox page, with recorded-stage activity, elapsed time, last successful check and explicit missing file-level telemetry. Kept technical events in Advanced and prohibited timer-driven completion or estimates. |
| 2026-09-08 | Replaced signed-out glasses lettering with a cloud invitation on hover, focus or five-second idle timeout; added a scoped character bump and self-hosted bold Shrikhand title. Preserved the single-spin sign-in handover, reduced-motion accessibility, and all signed-in character behavior. |
| 2026-09-08 | Added the global snapshot interval in minutes, defaulting to thirty; exposed the latest completed snapshot timestamp and owner-authenticated archive download in Advanced. Added a distinct Canceling pause phase which keeps children unavailable until writer release is confirmed. |
| 2026-09-08 | Protected chat drafts from IME confirmation Enter using composition lifecycle and native keyboard signals, including Safari key code 229. Kept ordinary Enter submission and Shift+Enter newlines. |
| 2026-09-08 | Added independently collapsible Live/Paused sidebar groups with counts and owner-scoped persistence. Replaced generic resource states with Sandbox and Agent vocabulary; background capture no longer replaces Live/Pausing. Removed Workspace expansion and inert label buttons until Ready. |
| 2026-09-08 | Clarified New Agent location, profile, title and first-message fields. Added anchored, bounded profile hover details using pinned public settings and original-name links, and removed deleting/deleted resources from location choices. |
| 2026-09-08 | Confined profile hover previews vertically between the creation dialog header and footer, including short mobile viewports. Selecting a profile dismisses its open or pending preview so creation and close controls remain reachable. |
| 2026-09-08 | Enabled Workspace deletion with live Agents. Documented durable child stop and removal before Workspace files, refreshed cleanup stages, and public Sandbox event history in the persistent deletion dialog. |
| 2026-09-08 | Unified the selected Agent status across header, details and sidebar using timestamped resource and live observations. Older replay frames cannot overwrite a fresher state; navigation updates retain conversation history and drafts. |
| 2026-09-09 | Removed the false Agent run failed notice for normal owner/operator stops and Sandbox retirement while preserving actual provider errors and the recorded Stopped state. |
| 2026-09-09 | Unified Profiles and Marketplace cards using Ornn's title, badges, description and aligned metadata order with CMA tokens. All three profile kinds retain native full-card links, original names, revision/visibility/retirement facts, usage and timestamps, with responsive tracks and no additional per-card reads. |
| 2026-09-09 | Resolved Marketplace authors through the retained public identity directory for list and detail views. Missing names are explicitly unavailable; account IDs remain ownership authority and are never used as display-name fallbacks. |
| 2026-09-09 | Added bounded, timestamped Live activity to lifecycle and deletion surfaces, with allowlisted plain-language entries, visible-only incremental reads, reconnect and access-loss handling, and manual-scroll preservation. |
| 2026-09-09 | Removed deleted Sandbox detail and descendant reads from sidebar loading and refresh. Kept authorized inventory, mutable state, access-denial handling and direct conversation history independent of the hidden deleted hierarchy. |
| 2026-09-09 | Added written Agent creation provenance and publisher-defined quick messages above the composer, with current published content, hover/focus descriptions, draft-preserving guarded submission and repeated authoring fields using existing controls. |
| 2026-09-09 | Bounded the quick-message row independently from chat after mobile stress verification; portalled descriptions through the existing viewport-clamped menu placement and hover-exit delay so long descriptions remain readable. |
| 2026-09-09 | Kept Connect available during pending NyxID setup, reused current hosted links on reopen, and made Verify an immediate check with explicit still-not-connected feedback. Preserved popup blocking, same-tab recovery and credential-free storage. |
| 2026-09-09 | Named connection-only dialogs Connect required accounts with Verify connections as the combined check, keeping permission setup distinct from publisher-required review. |
| 2026-09-09 | Show retained public names for Operations owners, activity actors and user targets, with explicit unavailable names instead of subject IDs. Added Owner to the ordinary Sandbox information page and grouped the admin Agent inventory under its named Workspaces. |
| 2026-09-09 | Read effective NyxID services from the Sandbox's public service list, including explicit owner grants, while preserving Agent lineage as an older-server fallback and the existing named service links. |
| 2026-09-09 | Keep the signed-out ChronoAI Managed Agent title on one responsive line. Widen the landing lockup, enlarge the cloud invitation to 264px by 88px, and use matching bold Shrikhand at 24px inside it, preserving the existing activation and signed-in character behavior. |
| 2026-09-09 | Add shared showing and dismissal fades for notification messages and dialogs, retaining only an inert exit frame, releasing focus immediately, cancelling stale dismissal on reopen, and honoring reduced motion without changing message semantics or geometry. |
| 2026-09-09 | Add the Sandbox Profile Availability band with public-only warmup, a one-to-five target and Always keep alive, using existing fields and checkbox rows. Clarify the editable private-copy Marketplace action and its fresh Sandbox provisioning behavior. |
| 2026-09-14 | Keep active Thinking visible above the composer with the rotating compact CMA character and elapsed time restored from the recorded turn start. Limit its disclosure to live reasoning and pending tools; show each settled tool in its own chronological container, preserving replay identity, unknown outcomes and accessible bounded details. |
| 2026-09-15 | Display immutable Agent creation categories independently of published Trigger shortcuts, add creation facts to Details, and use existing tree metadata and native-select patterns for per-Agent categories, complete child summaries and current-tree origin filtering. Clear optional facts on changed view/access context and keep private source references out of tree rows. |
| 2026-09-16 | Document authenticated per-tab Agent transport selection, explicit retained-request recovery, response action delivery status, account-change clearing and live-gap handling using the existing Agent status band, Button and composer patterns. |
| 2026-09-16 | Add explicit cancellation and exact recovery for the latest locally accepted queued message using the existing Agent status band and Button. Preserve active-response controls, pending input, account isolation and editable drafts. |
| 2026-09-16 | Offer ordinary queued input beside Stop response in Responses v2 tabs, with distinct accessible labels, existing composer controls and draft-preserving admission recovery. |
| 2026-09-16 | Keep Queue message and Stop response aligned beside the input at every supported viewport, using explicit composer grid columns and existing control sizes and gaps. |
| 2026-09-17 | Dismiss an open menu before its enclosing dialog on Escape from a row or trigger, preserving the selected value and returning focus to the trigger. |
| 2026-09-23 | Moved Bot settings into the bottom-left account menu, retaining capability checks and the canonical `/bots` route. |
| 2026-09-23 | Move the navigation scope Picker between the header build badge and New agent, keep the compact mark and usable controls at narrow sidebar widths, and remove both sidebar Agent filters while retaining resource facts. |
| 2026-09-23 | Remove the same two sidebar filters from Organization and Shared with me trees, preserving authorized hierarchy expansion and pagination. |
| 2026-09-23 | Retire the old bridge management routes and pending bridge-message panel; Bot settings remains the supported channel-management entry in the account menu. |
| 2026-09-25 | Reuse Personal hierarchy rows, page bands, Agent header and Details for organization/shared resources. Restore scope from authorized ownership on deep links, retain drafts during explicit scope changes, and omit unsupported personal creation in organization scope. |

### Organization and shared navigation

Organization and Shared with me use the Personal tree row, status dot, keyboard
navigation, canonical resource links and selected-row treatment. Organization
Sandboxes use the same Live/Paused groups and expose independently authorized
Workspace and Agent children. Collapsing a branch survives access refreshes;
revoked children disappear without reopening a denied branch automatically.
Compact access metadata uses the existing tree truncation treatment.

Sandbox and Workspace pages reuse the Personal page header and information
bands. Agent conversations reuse the Personal header, message stream, Thinking
dock, composer and right-side Details panel, including the compact overlay and
focus restoration. Details contains access, channel activity and authorized
artifacts. An Agent-only share never fetches private ancestry to fill a breadcrumb.
Current capabilities decide the available actions. Personal creation controls
are absent in organization/shared scope while organization creation is unsupported.

Resource deep links and reloads derive their navigation scope from current
authorized ownership. Explicit scope selection preserves the open conversation
and its draft. Changing viewers discards scope state.

### Approved Agent artifacts

Personal Agent Details offers **Artifacts** under current artifact visibility;
current sharing managers may publish a selected file after explicit exposure
acknowledgment. Shared Agents offer the same lazy artifact entry in Details.
No parent, repository, credential or runtime files are requested by this view.
Use the standard Modal and Button primitives, safe display names, byte counts,
independent paging and clear empty/denied states. Downloads recheck access before
and after byte buffering and verify the immutable size/hash before creating a
local file. Content is downloaded as opaque bytes and never executed or rendered.

Keep publication key and body after an uncertain response, including dialog
close/reopen and Agent navigation within the mounted viewer Console. A retry cannot swap
selected content. New content requires a new deliberate selection and exposure
acknowledgment. Current scope/access loss clears artifact rows, disables
publication and cancels pending downloads while preserving independent personal
conversation work. Explain that published content is retained with the Agent
and existing downloads cannot be recalled.

### Channel activity and management history

Show channel activity independently of immutable creation origin. Personal and
shared Agent Details distinguish current linkage, past linkage and absent history. Inventory and personal tree filters combine creation origin with current linkage,
any history, detached history or no history. Tree facts use at most four optional
reads concurrently and expire under the existing bounded read policy. Sandbox
and Workspace channel counts require facts for every represented child; an
unknown child suppresses the summary instead of relabeling its siblings. Use the existing Picker and page controls;
changing a filter resets pagination. A filtered page can be empty with a next page.
Rows and JSON exports include only the safe active/history flags and recorded
first/last accepted-message times. An older server that omits these facts shows
activity unavailable rather than guessing absence.

Managers can open **Channel history** in the standard Modal. Load history lazily,
page immutable events, label the actor and time, and place before/after binding
references inside an expandable event detail. Attachment alone shows no accepted
message time. Keep raw external conversation identifiers out of the presentation.
Check current management access before and after each buffered page and retain
the initial authority deadline. Changed scope, revoked authority or an expired
read clears private history. Viewing history preserves the personal composer and
draft. Summary readers never receive the management history control.

| 2026-09-23 | Merge the Profiles and Marketplace account entries into one Profile library, with My profiles, Marketplace and Event workers navigation. Add versioned Event Worker Profile composition, retirement and Bot selection using existing fields, pickers, cards and editor bands. |

| 2026-09-23 | Retain Event Worker Profile save requests in owner-scoped browser session storage across reloads, require explicit same-request retry, and keep saved workers readable and retireable when source catalogue entries are removed. |

| 2026-09-23 | Align the view inventory with the unified Profile library and owner Bots; remove the retired Bridge setup specification while preserving historical changelog entries and Agent provenance vocabulary. |

| 2026-10-14 | Make consumer review fact labels wrap within their existing grid column, matching values and preserving accurate wording without overlap at narrow widths. |

| 2026-10-14 | Acknowledge Pause and Resume immediately, distinguish durable acceptance from observed completion, and briefly accelerate targeted progress reads. Remove accepted Sandbox deletions from Live/Paused navigation while retaining a separate deletion-progress disclosure and same-request recovery. |

| 2026-10-14 | Preserve navigation to retained Agent conversations and history while Sandboxes pause, remain paused or resume. Keep actual lifecycle labels and closed progress disclosures, retain drafts, and hold pending-operation input and child runtime mutations until authoritative readiness. |

| 2026-10-14 | Open actual admitted Agents while Starting after lineage and parent checks; allow durable v2 queued input with receipt-based draft acknowledgement. Retain creation setup through early navigation and offer explicit same-request continuation when a worker rechecks connections. Preserve existing transports and hold input/recovery during uncertain access or parent lifecycle work. |

| 2026-10-14 | Load independent Responses v2 Agent projections and the first activity page concurrently, while retaining the account check after all required reads and every history, Sandbox and connection barrier before Send. |

| 2026-10-14 | Prepare transport for the actual created Agent during lineage reads, hand it to one navigation load with fresh entry authorization, and overlap exact-Agent connection verification with authorized conversation loading. Preserve retained transport choices, cancel stale account/target generations, and keep all history, parent and click-time Send checks. |

| 2026-10-14 | Open actual Starting Agents during their own lifecycle bootstrap only with exact server-proven lineage and current owner/parent checks. Fresh Agent and parent reads permit durable v2 queued input while that parent is Provisioning or Installing; withdrawn proof, lifecycle barriers and AG-UI remain held. Preserve unresolved Worktree state and distinguish queued acceptance from runtime readiness. |

| 2026-10-15 | During an explicit Sandbox revival, permit owner messages through the retained durable Responses v2 transport after fresh matching Agent, parent, owner and Worktree checks. Acknowledge the durable receipt while retaining actual Resuming state and inactive runtime resources; dispatch waits for backend readiness. Stale observations, checkpoints, other lifecycle operations and AG-UI keep input held. |

| 2026-10-15 | Retry an explicit Responses v2 admission conflict up to three times while keeping the exact retained message and request key. Recheck account authority and the retained pending intent before each retry, honor navigation cancellation, and acknowledge only a durable receipt. A received receipt, uncertain network outcome, different refusal or exhausted retry budget retains the existing explicit recovery behavior. |

| 2026-10-15 | Keep accepted Pause/Resume feedback concise and underlying waits in a closed Operation progress disclosure. Dismissal preserves observation and target locking; failures remain visible. Check the exact owner's connections silently before Resume and open connection assistance only when required. |
| 2026-10-15 | Prepare exact-owner Agent connections during creation and transfer the result into its authorized personal view. Permit one immediate v2 send to consume that report within one second; scope, settings, grants and visibility changes invalidate it. Preserve normal per-send recovery and all client/server authorization checks. |
| 2026-10-15 | Use only current personal-access lineage IDs to overlap fresh Workspace and Agent reads and start parent Sandbox observation during conversation loading. Keep entry/final account checks, full-history input gating, fresh projection identity checks and all observation invalidation rules. |
| 2026-10-15 | Display outgoing messages immediately in the existing owner message flow with caption delivery status. Keep drafts until authorized acceptance, reconcile by exact command/run identity, preserve same-request recovery, and prevent stale account or access callbacks from clearing drafts. |

| 2026-10-15 | Unify dialogs at centered 60vw/60dvh bounds; replace creation steps with confirmed circular progress; move the sole brand/build to the global home link; use runtime directory and description labels; fix scope/skill row heights; retain Agent navigation history and cursor replay; move machine metrics to Sandbox details and show cumulative/current-turn tokens with bounded observed-count animation. |

| 2026-10-15 | Accept pasted images and dropped or selected files in the message composer. Show fixed-height upload records, image thumbnails, bounded two-file concurrency, individual retry/removal, and compact sent-file records. Wait for persistent upload receipts before sending exact sandbox paths. Retain up to six viewer-scoped drafts and ready attachments across navigation; abort in-flight uploads on departure and keep explicit same-key retry. |

| 2026-10-15 | Show explicit unavailable input/output when a turn has no reported token counts; explain retained Agents that report only completed-turn totals. Animate real cumulative footer updates as well as current-turn observations, preserving actual initial totals and settling hidden/reduced-motion views without waiting for an animation frame. |

| 2026-10-15 | Retain each completed turn's observed token counts under its final Agent reply using durable replay. Keep brief reconnects quiet, present live-fragment refresh as a neutral caption, and clear it on matching recovered observations while preserving confirmed history gaps and failures. |

| 2026-10-15 | Compact the composer to one centered row with fixed Send/Stop seats, expand long drafts upward on focus or hover, and keep attachment previews above the row. Read screenshot clipboard file items when FileList is empty, avoid duplicate uploads, and preserve plain text paste and accepted-message clearing. |

| 2026-10-15 | Add consistent avatar and display-name rows to durable, streaming and optimistic messages. Use the existing 3D character's static front-face capture for Chrono Agent, current personal account identity for user messages, and black-circle initials when an avatar is absent or fails. Keep unknown shared authors distinct from the viewer and preserve narrow-layout wrapping and accessible names. |
| 2026-10-15 | Show clearly labeled approximate visible output tokens while a live turn awaits provider usage. Count current reply and visible reasoning with a lazily loaded o200k_base worker, throttle to five updates per second, cache bounded chunks and label the bounded text cap. Authoritative reports replace estimates and anchor their covered text; later model calls show only newly visible estimates beside actual counts. Preserve replay boundaries and cumulative totals. |
| 2026-10-15 | Let the whole modal card scroll at viewport widths or heights of 480px or less. Preserve the shared centered geometry and 20% margins while keeping fields, Close and footer actions reachable instead of collapsing the body to zero height; let shared input wrappers shrink within the narrow content width. |
| 2026-10-15 | Share Agent display-title selection across header, browser title and sidebar. Keep explicit native titles, omit only previews positively identified as automatic setup, and fall back to the configured name. Preserve unknown legacy descriptions and title provenance through live updates, hierarchy refresh and replay without selecting a message by role or position. |

- 2026-10-15 — Apply the existing shared opacity presence to dropdown menus as well as dialogs: closing releases keyboard/pointer actions and focus immediately while its inert frame fades; quick reopening cancels stale removal. Use the existing duration/easing for composer drop-up max-height and menu hover tint, with reduced motion removing both transitions. No palette, geometry, menu action, or message admission changes.
| 2026-10-15 | Hand visible text and ready attachments to the outgoing message immediately on valid Send. Retain the original snapshot until authorized acceptance, restore untouched input on refusal or uncertainty, and preserve newly typed text and later attachments. Keep exact request recovery, navigation drafts, IME and separate queue/Stop behavior; authoritative v2 refusals show Not sent. |

- 2026-10-15 — Add shared opacity arrivals for stable route surfaces and retained service re-entry (300 ms), and newly sent/live assistant messages (200 ms). Preserve drafts, focus, scroll, controller identity and history; streaming updates do not restart an arrival. Cancel stale transitions and honor live reduced-motion preference changes.

- 2026-10-15 — Preserve displayed opacity when menu, dialog or notification presence reverses mid-transition. Use shared opacity transitions with starting styles for first appearance; retain immediate inert exits, focus behavior, drafts and reduced-motion removal.

- 2026-10-15 — Reuse already resolved profile/service/skill labels synchronously when a view returns within the same authenticated reference provider. Keep existing link identities and error handling, and discard names immediately when the client/account changes so loading text does not flash in place of known names.
| 2026-10-15 | Preserve recent owner Sandbox content and immutable Agent creation labels across route re-entry within a bounded account/client scope. Revalidate access and lifecycle state before controls become usable; discard denied, missing, shared and retired projections, reject late replies, and retain no privileged provenance facts. |

- 2026-10-15 — Keep New agent admission copy in its starting state while the request is in flight; retain same-request recovery wording after an unresolved outcome, avoiding a recovery flash during healthy creation.


## 25. Unified service navigation and system text (2026-10-16)

> Superseded in part by §10 (2026-10-21): the three service segments, the brand title and build
> badge in the first row, and the labelled New agent trigger row are replaced by the service menu,
> the action row and the Triggers navigation row. System text and the composer command drop-up
> below are unchanged.

The sidebar owns the single home brand link and build badge. Its first row holds a 20px mark,
ChronoAI Managed Agent title, compact revision badge, and collapse control. The following row
has three equal segments in this order: Agent Manager, Event Gateway, Image Manager. The active
segment uses the existing selected navigation fill and a continuous sliding highlight; Image
Manager remains visibly disabled and cannot open through a direct route. There is no global
top service bar or duplicate current-service label. Account controls remain at the sidebar foot
across services. Collapsed and mobile navigation retain a reachable brand, service switch and
account control. Existing motion easing/duration and live reduced-motion preference apply.

Agent Manager begins with labeled New agent and New agent trigger rows, then the
Sandbox/Agent view switch and grouped permission-scope/status filters. Organization choices append `(Organization)`. Agent view
starts at Personal and All statuses. Tree groups retain their existing machine/workspace/Agent
structure. Ready Agents use neutral indicators; only actively Working Agents use a green light.
Live Sandbox indicators remain green. Event Gateway lists channels in this same navigation area,
with the selected channel's details and edit/delete/test actions in the main surface.

The account menu includes Docs above admin-only Operations, Appearance, System text size, and
Sign out. Docs contains Event Gateway Protocol, ported from the Gateway documentation. Agents
and trigger creation no longer appear in the account menu. System text uses explicit Small,
Medium and Large choices at 90%, 100% and 115% of the established ramp. Font sizes and fixed
text line heights scale at their consuming element; spacing, icons and viewport geometry do
not scale. The conversation message stream, including user/Agent messages, tools and Markdown,
keeps its existing text sizes. Preferences apply before first paint, persist when storage is
available, and stay consistent across retained service surfaces and browser tabs. Controls
reflow or truncate within their rows and remain usable at 320px phone, 13/16-inch laptop and
desktop widths for all three sizes.

The composer opens a bounded, scrollable command drop-up when its draft starts with `/`. It
uses the current session's native available-command inventory and installed skills, supports
filtering, arrow keys, Enter/Tab selection and Escape, and retains focus in the composer.
Commands retain their real runtime semantics. Presence and selection transitions use existing
motion tokens, reverse without jumping and stop under reduced motion.

- 2026-10-16 — Unify service navigation and account controls inside the sidebar, add labeled
  creation and permission/view filters, expose native session commands in the composer, port
  Gateway documentation, and add independent system-text sizing with responsive layouts.

## 26. Composer steering and individual queued messages (2026-10-16)

> Superseded in part by §13 (2026-10-21): Context and Total tokens moved into the composer's
> context ring, and the steering menu became a chip inside the composer box shown only while a
> turn runs. The queue and its editing below are unchanged.

Context and cumulative Total tokens share the composer's input column, aligning to
its left and right edges respectively. The adjacent Send/Stop group has a stable
seat and a compact steering-mode menu directly below it. Its choices are Interrupt
and Queue. Queue preserves ordered ordinary input; Interrupt cancels the active turn and admits the new input as a distinct
replacement turn ahead of ordinary queued messages, preserving the remaining queue. Outside an active turn,
both modes send ordinary input. Stop remains a separate cancellation action. Use the
shared Menu, existing 8px gaps, 32px controls (44px touch targets), caption typography,
neutral surfaces and motion tokens. Preserve draft content on refusal or uncertainty.

Queued messages appear in the main conversation in a neutral, bordered queue group,
replacing the separate yellow queued-message notice. Each row has the user's identity,
its content and attachment records, a queued label and independent edit/remove actions.
Clicking the content or choosing Edit opens an inline editor with normal paste, drop,
file-picker and per-file removal. Existing uploaded files retain their exact paths;
new uploads must finish before Save. Save preserves the original queue position.
Cancel editing preserves the admitted message. A dispatch race refuses the edit/removal
without changing a different entry. Never delete or replace the entire queue to edit
one message. Keep uncertain requests recoverable using their original identity/body.
Queue status comes from public authoritative reads; it survives reload and navigation.

If an entry leaves the queue while its editor is open, keep the unsaved text and
attachments in that editor as a retained draft. Explain that the original message
can no longer be edited; offer Copy draft and Dismiss without implying it is still
queued. Copy includes every confirmed attachment reference and explicitly identifies
unfinished uploads. These local recovery actions remain available when sending is
unavailable; network upload retries still respect current availability.

Use existing presence transitions for row/editor entry and exit, existing fast easing
for hover/focus, and no motion under reduced-motion preference. Queue rows wrap within
the conversation width; actions remain reachable at phone widths and all text sizes.

- 2026-10-16 — Align usage to the input, add active-turn steering selection below the
  composer controls, and replace the latest-message banner with an individually
  editable/removable durable queue including attachment editing.

At phone widths, Context and Total tokens may occupy two successive rows in the
input column while retaining their respective left and right alignment. Numeric
groups remain whole rather than wrapping digit by digit. The action column reserves
80px on desktop and 96px for two touch targets; its mode menu stays centered beneath.

- 2026-10-16 — Retain unsaved queued-message edits and attachments after dispatch or
  cancellation, with explicit local copy/dismiss recovery and no stale queue actions.


## 27. Grouped filters and Gateway consumers (2026-10-16)

> Superseded in part by §10 (2026-10-21): the scope and status pickers now sit behind one Filter
> button with a summary line. The Event Gateway navigation below is unchanged.

Agent Manager places the permission-scope and status dropdowns in one horizontal
filter group below the Agent/Sandbox switch. Both controls share equal space,
existing sidebar typography and an 8px gap. Long labels truncate inside their
control while the menu exposes the full value. Keep 44px touch targets on mobile.

Event Gateway navigation begins with Channels and, for authorized owners, Consumers.
Use existing sidebar selected fills, focus rings, spacing and 44px navigation rows.
Consumer list, detail and creation routes select Event Gateway in the service switch.
Channel editors and consumer forms retain their DOM and in-flight work when switching
sections. Account preferences no longer contain Consumers or Developer; Docs remains.
Existing hover/presence motion and reduced-motion behavior apply.

Channels opens on a §12.6 table of every channel — Name, Provider (the mono slug),
Level, Sources (a count) and Version — under the *Event Gateway / Channels* header;
the sidebar's New channel stays the one create action. Nothing is selected until somebody
chooses a channel, from the table or from the sidebar list, which keeps its rows and
selection; the detail's *Channels* breadcrumb returns to the table. Empty says *No
channels yet* and *Connect an event source to your agent.* In a channel each
endpoint is a row: its name, a one-line purpose, the URL in the code face, and a
compact **Copy** button that says *Copied* politely. When the clipboard refuses,
the row says so and selects the URL so it can be copied by hand.

Signed out and not permitted are different pages. A 401 keeps the *Sign in with
NyxID* link; a 403 says *Your account doesn't have Event Gateway permissions.* with
a Check again action and never offers sign-in, which would only return the same
account to the same refusal.

Consumers is a §12.6 table of Name, State, Provider level, Last test (*Not tested*
or *Accepted after <n> ms*) and Updated. State is a §12.1 badge: Creating and
Deleting active, Active neutral, Needs attention error, Disabled and Deleted muted.
The consumer page carries the same badge beside its title, and its failures are
§12.1 alerts.

- 2026-10-16 — Group permission/status dropdowns on one row, place Consumers in
  Event Gateway navigation with retained section state, and remove Developer from
  the account menu.


Successful response-action delivery clears its temporary status automatically;
the conversation shows the resulting turn state. Keep actionable failure and
uncertain-request recovery visible without a persistent success banner.

- 2026-10-16 — Clear completed response-action notices automatically, preserving
  failure and recovery controls. Render command-help placeholders as inert text.


- 2026-10-16 — Make Interrupt mode stop the current turn and start a distinct new
  message next, including its attachments; keep Stop as cancellation only. Preserve
  exact admission recovery, other queued messages and actionable failure states.

- 2026-10-16 — Start each home visit with collapsed navigation and expand it when
  entering an app or service. Preserve manual toggles across service navigation
  and keep the narrow-screen automatic collapse rule.


An ordinary Send while the Agent is idle remains a normal message while dispatch
is prepared. Queue discovery does not turn that message into a queue card. Keep
known waiting messages and uncertain queue edits/removals visible; match accepted
messages by command identity, never by matching text. A starting queued message
moves into the ordinary conversation without a duplicate or empty gap.

Attachment selection notices use the existing compact caption/error seat and a
labeled Close button. Dismiss after five seconds, restart that interval for each
invalid selection, and clear after a valid selection. Use the existing presence
fade and reduced-motion behavior. Dismissal never removes attachments or conceals
per-file upload failure and Retry controls. The notice leaves no empty space after
exit; retain the existing 44px mobile touch target.

- 2026-10-16 — Keep idle Send dispatch preparation in its ordinary message bubble,
  retain genuine queue/recovery cards, and make attachment selection notices
  dismissible with a five-second lifetime.

- 2026-10-16 — Check Agent connections quietly while opening or switching
  conversations. Keep sending gated until verification, preserve actual lifecycle
  notices, and show connection assistance only for confirmed missing services or
  actionable verification failures with Retry.
| 2026-10-16 | Added evidence-based memory interruption and recovery notices, recorded-stage character animation, bounded completion feedback and retained interruption details. |
| 2026-10-16 | Unknown Sandbox interruptions display Internal error while recorded recovery progress and any failure to finish remain explicit; historical interruption details do not imply ongoing recovery. |
| 2026-10-16 | Move the single transient Thinking and turn-token row below the current Agent reply inside the conversation, align it with message content, and preserve elapsed/token state, reduced motion, smooth appearance/removal and reader-controlled scrolling. |
| 2026-10-19 | Align Thinking with the Agent avatar and widen the conversation and composer to 70% of the available Agent panel, with matching 24px/14px gutters in narrow containers. Preserve the existing transient-status animation and message behavior. |
| 2026-10-19 | Add the owner Runtime band with exact current/latest image references, idle daily updates at 02:00 SGT, safe manual retries, recorded update states and existing character animation; keep history readable through runtime rotation. |


### Operations profile inventory retirement (2026-10-19)

Operations offers Sandboxes & usage, Warm pools, Platform settings and Activity, with
their existing capability gates and keyboard navigation. It has no Profiles tab,
inventory or retirement action. This supersedes the Profiles inventory in §19.
Immutable profile records and historical public APIs remain available to runtime
recovery and compatibility; this navigation change does not modify them. Native
warm-pool controls and current platform settings remain in their own views.

| 2026-10-19 | Remove the obsolete Profiles inventory and retirement panel from Operations while retaining internal immutable profiles, historical APIs and current configuration controls. |

### Current runtime warm pools (2026-10-19)

Warm pools shows the current runtime image and its pools first. An exact match to
its published default configuration hash receives a Current default configuration
label; other resource sizes for the current image remain configurable. The full
recorded configuration hash is available through Pool configuration disclosure.

Previous runtime pools use the existing bordered disclosure style and start
collapsed only when their targets and observed ready capacity are zero with no
error. Positive targets, remaining ready capacity, failed observation or errors
keep the previous-pool disclosure open and show a notice outside it. Older images
are no longer prewarmed: their controls allow Disable and conflict recovery reads,
never an increase. Disabling spare capacity does not interrupt claimed Sandboxes.

| 2026-10-19 | Prioritize current-runtime warm pools, mark the exact default configuration, and collapse drained historical images while keeping unresolved capacity visible with disable-only controls. |

### Composer settings and message feedback (2026-10-20)

> Superseded in part by §13 (2026-10-21): model and effort became one chip with a popover card and
> a discrete effort slider, and permission became a chip, all inside the composer box; there is no
> settings row. Timestamps and message feedback below are unchanged.

Model, reasoning effort and permission mode belong with the composer, in a
dedicated row directly below its text input at every width, rather than in Agent
Details. Use the
shared small value pickers, with the selected value visible and full accessible
labels. Arrange them in a compact horizontal group with existing 4px gaps;
truncate long model names within 120px. Keep the current owner, runtime capability,
turn-idle, explicit Apply and same-request recovery rules. Only a changed draft
or retained update shows Apply/Retry; changes never submit conversation input.
The row sits before usage, leaving Send/Stop and steering aligned in their action
column, so the draft always keeps the full flexible column. At 30rem or less, retain 44px touch targets and let the
three controls share the full input row. Preserve typed drafts, attachments,
queue/interrupt behavior, and input-aligned Context/Total tokens.
The permission picker reads **Guarded** and **Autonomous**, in the compact
composer and the full settings alike; the wire values `guarded` and `autonomous`
are unchanged. No other name for the unrestricted mode appears in the UI.

Each normal message carries its recorded timestamp as a semantic time in
Asia/Singapore, with an explicit SGT suffix, using existing 12/18 caption text
and tabular numerals. Never substitute render time for a missing historical
timestamp: state that its time is unavailable until an authoritative timestamp
arrives. An outgoing local message uses its actual local submission time; live
messages use the source event timestamp. The durable message timestamp replaces
that provisional observation when the recorded message arrives.

Reserve a compact footer for the timestamp and Copy content, Upvote and Downvote, using the existing
copy/like/dislike glyphs on 28px keys (44px on a coarse pointer), because the footer
is reserved under every message and its height sets the conversation's rhythm. At rest a message shows only its
content. Fade the timestamp and these controls in on message
hover or keyboard focus over the shared fast duration, with no movement or layout
shift; keep them visible on touch devices. A copy, vote or feedback status stays
visible while it is shown. A selected vote stays visible and uses
the existing business-primary ink and a filled glyph. Copy reports a brief local
confirmation and failures remain actionable. Votes toggle off or switch sides,
persist for the authenticated owner and exact recorded message, and never create
an Agent turn. Until a streamed message is recorded its voting controls explain
why voting is unavailable. Failed writes preserve the last confirmed vote with a
retry action; no success is inferred from a click. Shared readers retain copy and
timestamps but cannot vote as the owner. Honor reduced motion and preserve focus.

| 2026-10-20 | Move model, effort and permission controls beside the composer with responsive wrapping; add always-visible SGT message timestamps and accessible hover/focus/touch copy and durable owner feedback controls. |

| 2026-10-20 | Request negotiated server live-fragment observation timestamps for streaming message footers, preserve the first observation during text growth, and replace it with recorded message time on commit. Existing strict clients keep their prior wire shape; older servers without timestamps retain the explicit unavailable-time fallback. |

The composer settings row uses the available input width, so catalog errors and
long values wrap below the controls without narrowing the message field. In the smallest layout the settings row follows
input-aligned usage, allowing all three controls their full touch targets.

| 2026-10-20 | Extend the full-width narrow composer through 320px conversation columns so 360px phones retain a readable draft; verify cold idle and active layouts at 320px, 360px and 390px viewport widths. |
| 2026-10-21 | Moved both appearances onto a shadcn/ui-based neutral system: one `zinc` ramp for grounds, borders and labels, Chrono lime kept as the brand and accent (`lime 800` in light, where the bright step cannot carry text), a `zinc 900` light primary and lime dark primary, neutral low-alpha light shadows, inverted tooltips, and one interface/code pairing, Geist and Geist Mono, in both appearances. Kept every alias and specific name and the three-tier architecture so every surface re-inks without a migration vocabulary; the earlier ramps and faces stay declared only for remaining static and brand-asset consumers. Regenerated the §2 alias contract from the shipped sheet so the tables cannot drift from it. |
| 2026-10-21 | Let the signed-out product name wrap with balanced lines when its column is narrower than the name. The unbroken line overflowed its 360px grid column between 761px and about 1040px, where the services cards covered it, and widened a phone's grid past the viewport. Restored the lockup's documented 144px top reservation so the cloud invitation no longer clips above the viewport. |
| 2026-10-21 | Rebuilt the signed-out home around one visible primary action. The character had been the only way in, its Get Started name never appeared on screen and its invitation waited five seconds, so §20 now specifies a hero with the product name as `h1`, a one-sentence value line, Get Started, a NyxID caption that announces progress, three How it works steps and service cards that return to their service after sign-in. Image Manager is marked Coming soon until its route exists. The character stays as a companion without the cloud. Copy never names the agent provider. |
| 2026-10-21 | Moved the composer settings into their own row under the input at every width. Beside the input, an `auto` grid track grew to the longest settings explanation and squeezed a 1440px desktop draft to about 120px; below the input the draft keeps the full flexible column, as narrow layouts already did. |
| 2026-10-21 | Rebuilt the sidebar around getting into a conversation. The three service segments became one service menu (Image Manager listed, disabled with its reason); the build revision moved into the account menu as a copyable row; New agent became the sidebar's primary button and is disabled with a reason instead of disappearing in Shared or Organization scope; Search and a ⌘K command menu over the already-loaded hierarchy were added; Triggers became a navigation row instead of a creation row; scope and status moved behind one Filter button with a summary line; the Agents view groups Agents by Workspace and Sandbox in order of recent activity; Sandbox rows use the Sandbox's own name. Phones lost the permanent rail: a 48px top bar opens the sidebar as a sheet over a scrim. `/developer` now opens Docs, and the unreachable-service page explains itself and offers Reload. Supersedes the matching parts of §25 and §27 and retires §19a. |
| 2026-10-21 | Gave every centre view one page header: a breadcrumb of real links ending at the current page, the title as the only `h1` with its status and short code-face facts, an optional description, and at most one primary, one secondary and a More actions menu. The Agent breadcrumb now names its Sandbox and Workspace as links instead of a path and the literal word "Agent", shows the Worktree branch, and moves Stop, Export history and Delete into the header with their reasons. Browser tabs are named after the view on every route, and a hidden service no longer leaves its name on the tab. Removed the agent provider's name from the remaining UI copy (subscription sign-in, model access, composer commands and the public Trigger details). |
| 2026-10-21 | Gave every status word one tone everywhere (active, neutral, warning, error, muted) with a neutral dot for Live and Ready instead of the success colour, which in dark matched the accent. Added the status badge, a severity-toned alert that replaces the danger-tinted notice and stacks in one place per view (on the Agent surface in one grid row, so no message can fall below the composer), and skeleton loading for navigation, the Agent, Sandbox, Workspace and inventory, and the console outline while identity is checked. Retired the 3D character from in-app rows: Thinking, connection rows, setup steps, recovery and runtime notices use a status dot; the character remains on the signed-out home and the brand. Supersedes the step-character and loading-character paragraphs of §23. |
| 2026-10-21 | Gave dialogs on a phone the full width less a 16px gutter instead of 60% of the viewport, which left a 192px card whose footer overflowed at 320px. Stopped the frame from ever scrolling sideways: as a clipping box it no longer shifts when a closed details column's content would otherwise count toward its width, which put a phone's page off its left edge. |
| 2026-10-21 | Made the conversation say what the Agent is doing and what it needs. Consecutive tool calls form one group headed "<n> tool calls" with the latest call's summary, and every row carries a state dot with a plain word (In progress, Completed, Failed, Cancelled, or Outcome not reported) instead of the raw status; the "Duration not reported" filler is gone because no duration is reported. Groups with a failure open themselves. Plans reported through the public stream render as a per-turn Plan card with Done, In progress and Waiting badges. The approval card gained a one-line caption under Deny, Abort and Allow. The permission picker reads Guarded and Autonomous. |
| 2026-10-21 | Made New agent one step that creates the Agent where the owner asked. A Workspace's New agent used to create another Workspace in its Sandbox, so a Workspace made with New workspace could never receive an Agent; it now opens the dialog on that Workspace and sends its `workspace_id`. §12.2 specifies the Where it works choice (New Sandbox, a Sandbox's new workspace, or a Workspace; unusable rows `aria-disabled` with their reasons), repository validation on blur and submit instead of on every keystroke, the read-only Agent settings summary and the access note. The preparation section replaces the percentage circle, which read 80% with every listed step Ready, with a four-step stepper (Sandbox, Workspace, Service access, Agent startup) that is also the setup band above the conversation. |
| 2026-10-21 | Rebuilt the owner Sandbox page around one primary action and tabs. The header keeps New agent as primary and Pause or Resume as secondary, with its lifecycle reason line, and moves New workspace, Sharing, Technical details (formerly Advanced) and Delete Sandbox into More actions with their reasons. Every Sandbox message now sits in one toned alert stack. Overview, Workspaces, Access and Activity tabs keep their selection in the `tab` query and stay mounted while hidden. The Workspaces table's New agent here now starts an Agent in that Workspace instead of a new Workspace. The NyxID service scope editor moved from Agent Details to the Access tab, where the Sandbox-wide key belongs; Details keeps a read-only summary with Manage on Sandbox. Owners get a real Workspace page with Sandbox / Workspace breadcrumb, facts, an Agents and Worktrees table, New agent and Delete workspace; shared readers keep the shared projection. Supersedes the band layout, the closed Sandbox operation progress disclosure and the snapshot paragraph of §17. |
| 2026-10-21 | Added one shared table (§12.6): a bordered box at radius 12 with a 40px header row, 52px rows on hairlines, a right-aligned actions column with a visually hidden header, note rows, skeleton loading and an empty row, scrolling inside its own box on narrow screens. Event Gateway channels and consumers and the Operations Sandboxes, Warm pools and Activity lists moved onto it, with status and outcome badges and SGT-labelled time columns; Operations says a missing cross-owner capability once for the page instead of on every row. Channels open on the table, endpoints gained Copy, and a 403 now says the account lacks Event Gateway permissions instead of offering sign-in again. Triggers carry their Draft, Published or Retired badge in the rail and header. Launch progress rows use state dots instead of the character, with the overall state as a badge and failures as alerts. |
| 2026-10-21 | Rebuilt the composer as one rounded box with its controls inside: Attach files, a permission chip and, only while a turn runs, the steering chip on the left; one model and effort chip, a context ring and Send on the right. The chip opens a card with the effort word, the model list and a keyboard-operable effort slider with a reset to the confirmed settings; the explicit Apply rule is unchanged and its status sits inside the box. Context and Total tokens moved from a text row into the ring's tooltip, and a missing figure still reads not reported. The Chrono Agent avatar became the inline CMA mark on a themed circle, so light no longer shows a black square. The Agent header became one line that collapses to a 28px strip, reveals itself on hover or focus without moving the conversation and can be pinned per viewer; the Details toggle, creation origin and subscription sign-in moved into More Agent actions. |

### Agent files and share links (2026-10-22)

Agent Details offers Files for the mounted `/home/` directory. A Files dialog
reuses the centered Modal and its 60vw/60dvh bounds, focus trap, dismissal and
presence transitions. Its desktop body has a 200px directory rail and a flexible
preview/editor; below 48rem the rail and editor stack. Directories expand on
request, with fixed 32px rows, existing folder/code icons and ellipsis for long
names. The path remains copyable. Show hidden defaults off and reveals dotfiles
and hidden directories. Refresh is explicit; no background directory polling.

Text is rendered as text, with a code-face editor for supported text files.
Raster images use an isolated image preview. Other binary formats offer download.
File content never executes scripts, HTML or SVG. Save uses the last read file
version; a conflict retains the draft and asks the user to reload explicitly.
New file, New folder and Upload use existing buttons and fields. Delete confirms
inline, never with a second dialog. Unsaved changes are retained until saved or
explicitly discarded. Read-only access hides mutations while preserving browsing
and downloads. Requests and previews are cleared when Agent or authorization
identity changes. Existing motion durations and reduced-motion behavior apply.

Agent Details also offers Share Agent. It creates multiple independent links,
each with View only or View and write and an audience of anyone with the link,
an organization, or selected people. Links are accepted after sign-in and add
only that Agent to Shared with me in Agents view. The recipient sees Shared by
with the sharer's display name and the effective permission. View-only disables
message input, settings and file changes; view-and-write enables those Agent
operations, never Sandbox lifecycle or sharing management. Revoking a link
removes access granted through it; other independently valid links remain valid.
The sharing dialog states that Files exposes the shared Sandbox's full `/home/`
so the owner can make an informed sharing decision. Secrets stay in URL fragments
and are removed from the address after redemption.

Returning to an Agent shows its latest authorized transcript at the conversation
tail. Historical event pages rebuild rich state without presenting past turns as
live Thinking or animating old messages as new arrivals. Agents navigation omits
Workspace rows; Sandbox navigation retains them. Operations Actor is a named
selector instead of a free-text identity field.

| 2026-10-22 | Add lazy mounted-home Files browsing/editing and scoped Agent share links using existing Modal, controls, tokens and transitions; show latest conversation on return, simplify Agents navigation and select named Operations actors. |


### Agent navigation, loading, and text preferences (2026-10-22)

Agents view is a flat list of individual Agent cards, ordered by recent activity,
without Sandbox or Workspace group headings. Sandbox view retains its hierarchy
and contextual three-dot actions. The global New agent action always starts a
new Sandbox and offers no destination chooser. Creating an Agent in an existing
Sandbox uses that Sandbox's three-dot action; an explicit scoped action shows
its fixed destination as a description. Sandbox Details keeps Pause/Resume in
the header and places New agent in More actions, rather than a second primary
creation button.

Operations Owner starts at Everyone. Its report-scoped subject-prefix search
returns named choices with bounded server paging across the authorized cohort.
Choices remain available across table filtering and pagination, with cancellation,
loading and explicit retry on failure. Existing explicit filter application is retained.

The landing route keeps the sidebar collapsed and has no expand, drawer, drag,
or keyboard expansion affordance. Service routes retain normal sidebar controls.
The system text-size preference applies equally to conversation body, Markdown,
code, and tool content; it changes typography without scaling control geometry.
Only Chrono Agent replies have upvote/downvote controls. User messages retain
copy and timestamp controls. The browser favicon uses the current lime
`#C3FD43` mark in both OS appearances, with the visible glyph centered in its
square viewport; PNG fallbacks use the same mark.

Loading conversation history does not block sending a new message once the
current identity, Agent access, and Sandbox input availability are verified.
History-dependent approvals and turn controls still require their exact recorded
targets. The latest transcript is displayed without waiting for older rich event
pages, which hydrate offscreen without replaying historical Thinking. Returning
to an authorized cached Agent preserves its latest visible conversation while
metadata refreshes. Account/access changes clear or suspend the relevant state.
History completion must preserve input receipts, newer messages and active live
updates; it must never clear the user's next draft or submit a duplicate message.
Shared Agent access enables interaction independently of transcript and rich
history loading. Unchanged periodic access verification preserves the composer
and stream without rereading the transcript; actual stream disconnection repairs
the durable view before reconnecting.

| 2026-10-22 | Flatten Agents navigation, fix global New agent to a new Sandbox, select Operations owners by name, lock the landing sidebar, apply text preferences to conversations, limit voting to Agent replies, center the green favicon, and decouple input/latest-message display from historical activity loading. |


### Agent details panel (2026-10-22)

The Agent details panel is a calm side panel, not a settings form. Below its
header it scrolls as one region with a 16px inset; no list, catalogue or section
inside it has a scroll area of its own. Sections are separated by 24px of space,
never by hairlines, and appear in the order Files, Sharing, Service access and
Skills, followed by the Sandbox operation progress action.

Section headings are sentence case at 13px, weight 500, in
`--dsw-alias-label-primary`, without uppercase or the code face. A section's
explanatory sentence is 13px in `--dsw-alias-label-secondary`; code-face values
such as `/home/` stay code. Every text action in the panel (Browse files, Manage
share links, Manage on Sandbox, Sandbox operation progress, Refresh skill status,
Retry installation, Retry Ornn catalog and More Ornn skills) uses one quiet-link
style: 13px text in `--dsw-alias-label-secondary` with no fill, border, icon or
indent, starting flush with the text above it, underlined on hover, with the
shared `--focus-ring` on keyboard focus and the existing disabled treatment. Each
keeps its accessible name and role: navigation stays a link, commands stay
buttons.

Service access lists each effective service as a plain row: its catalog name in
the primary label colour, underlined on hover where it links, and, for
chrono-llm-public and ornn-api, the word Required after it in
`--dsw-alias-label-tertiary` rather than a bordered badge. Manage on Sandbox
remains the single action.

Skills lists the Agent's installed skills as plain rows: the name in the primary
label colour, then its version and installation state (Installed, Installing…
or Installation failed) in the tertiary colour, with an installation error on its
own line beneath. Pending, unavailable and busy explanations and the install
error with Refresh skill status remain directly under the list. The available
Ornn catalogue sits behind a native **Add skills** disclosure, collapsed by
default. Opening it reveals the search field, the catalogue rows with their
version picker and Install button, the catalogue loading, empty and error
states, Retry Ornn catalog and More Ornn skills. Catalogue rows grow with the
panel and are separated by 12px of space, without a bordered box. Installing,
version choice, same-key retry and lifecycle gating are unchanged.

| 2026-10-22 | Calmed the Agent details panel. Section headings became sentence case at 13px weight 500 in the primary label colour (Files, Sharing, Service access, Skills; "Session sharing" became Sharing), sections are separated by 24px of space instead of hairlines, and every text action shares one quiet-link style flush with the text above it. Required services read as tertiary text instead of bordered badges. Installed skills are plain name and version rows, and the Ornn catalogue moved behind a collapsed Add skills disclosure that holds its error and retry states, so the panel scrolls as one region without nested scroll areas. |
| 2026-10-22 | Quiet the Agent conversation so reply text leads: identity once per run of one author's messages, the timestamp revealed with the message actions on hover or focus (always on touch), no usage line for a turn without a report, lighter inline code, and tool calls as frameless lines whose group header reads "<n> tool calls" plus only the running call or failure count. |
| 2026-10-22 | Quiet the Agent Manager sidebar: New agent, Search and Triggers become plain 32px rows with no fill or border at rest; Agents \| Sandboxes becomes text tabs sharing one row with an icon-only Filter key that carries an accent dot, and a summary line naming only non-default filters appears only when one is applied; personal Agent rows become one 32px line with a state dot only for Starting, Working, Needs approval and Needs attention, the status word kept in the accessible name, and age and ⋯ shown only on hover, focus or the current row. |
| 2026-10-22 | Cap the Agent conversation at a centred 768px reading axis shared by the history and the composer, replacing the 15%-per-side rule that let replies run past 900px on wide screens; the cap is in pixels so text size scales the text, not the measure. |

### Live replies during conversation loading (2026-10-22)

An authorized live Agent head starts real-time observation independently of
transcript, rich event history and usage loading. A new message shows its usual
Thinking, streaming reply and message arrival motion while older history loads.
Historical rows remain static. Late history enriches the conversation without
clearing new text, restoring a completed turn, duplicating messages or restarting
a healthy live connection. An account, access or conversation identity change
still cancels observation and prevents stale output from entering another view.

| 2026-10-22 | Start live Agent replies independently of conversation history loading and preserve current Thinking, replies and arrival motion when historical records finish loading. |


### Anonymous editorial home and fixed CMA identity (2026-10-22)

Authenticated `/` immediately renders Agent Manager and replaces the home history
entry with `/agent-manager`. No authenticated landing is painted. Public Trigger,
Launch and connection-return routes retain their own behavior. Anonymous home
retains its collapsed navigation and explicit NyxID sign-in.

The anonymous landing uses an editorial composition inspired by Anthropic's
public site: generous space, a large restrained headline, a character companion,
and thin horizontal divisions rather than stacked rounded cards. It remains CMA,
with Geist, the existing palette aliases and the existing 3D mark geometry.
The content width is at most 1200px, with 48px total side clearance, 64px top and
80px bottom padding and 80px between sections. The hero uses columns 1.2fr/1fr,
a 48px gap and a 480px minimum height. Product eyebrow is 13/20 medium; display
copy is clamp(40px,5.5vw,72px), weight 500, 1.04 leading, -0.045em tracking.
Lede is 18/28, max 560px. Steps and services are three plain columns with 24px
gaps and 24px top padding over a border-l2 rule. Step numbers are 12/20 code,
section headings 32/40 medium, and body 15/24. At 900px the hero stacks with
character after text and capped at 280px; at 760px the grids stack, side clearance
is 32px, section gap 56px, padding 40px/56px, and section headings are 24/32.
The hero arrives with opacity and 8px translate over the existing slow duration;
reduced motion removes both entrance and sign-in transitions. Existing single
sign-in admission, 200ms fade, failure retry and service return routes remain.

The brand character has the same fixed studio palette in both themes: lime-400
body and fill; zinc-950 black lenses; zinc-50 sky/key/glint; zinc-800 ground;
zinc-400 rim; lime-400 shadow. Operational character body colors remain semantic.
Its WebGL fallback uses the same lime mark and black lenses. The favicon uses
exactly the conversation avatar paths, a lime #c3fd43 mark in a 20px inner square
on a 32px #18181b dark circle with its quiet #27272a 1px outline; PNG and apple
icons are rasterizations of that SVG, not crops of a screenshot.

The browser document title is exactly `ChronoAI Managed Agent (CMA)` on every
route. In-page headings continue to identify the current view. The portal's
upper-left mark is 40px rather than 20px, within a 44px brand control. Collapsed
navigation retains its 56px rail, with 6px inline padding to fit the control.

### Compact Agent toolbar and file split (2026-10-22)

Replace §13's collapsed/pinned Agent banner with one always-visible compact toolbar. It uses 48px minimum height, 8px 20px padding, base ground and no banner/breadcrumb/status strip. The 14/20 semibold Agent title is a truncating button with a down chevron and the shared accessible action menu. The heading stays focusable. Rename, Update system prompt, Update skills installed, existing provenance/details and permitted lifecycle actions use public APIs and existing authorization. The title's tooltip includes the current status.

A 32px computer control follows the title. Hover opens a 360px-wide Sandbox preview constrained to 12px viewport margins without moving keyboard focus; a 150ms pointer-exit grace period allows reaching its portalled contents. Keyboard activation and touch press open the same card, Escape/outside press dismiss it. Show the real Sandbox ID, live CPU/RAM/network readings and the currently enabled agent-key scope; show unavailable/measuring states instead of invented zero values. Resource reads run only while open. Share uses the existing Agent share dialog; Files is immediately to its right.

Files opens in the frame's right column, reusing its 300–520px width bounds and 640px center floor; on narrow screens it uses the existing right overlay. Header: 48px minimum with 8px 16px padding and Close files. Body: 12px padding and gaps, full-height scrolling; persistent /home/ tree, 40px rows (44px on phones), truncating names, existing file actions and a preview beneath the tree only when selected. Close/open keeps owner file selection and unsaved draft; shared file access is independently revalidated. Losing authorization clears access. The chat remains mounted and retains draft/live history when opening, closing, switching or resizing panels.

The vertical separator remains 8px with pointer capture and frame-coalesced movement. It is keyboard focusable, announces its current/minimum/maximum pixel width, ArrowLeft/Right changes width by 16px (48px with Shift), Home/End selects the bounds. Pointer cancellation ends dragging cleanly. Keep existing eased grid transitions except while dragging and honor reduced motion. At narrow conversation widths toolbar padding is 8px, title target 44px, and existing mobile button targets remain 44px.

New local browser acceptance runs at 1600px and 390px, light and dark, with only public HTTP fixtures; captures file-panel screenshots, checks drag/keyboard resize, viewport bounds and preservation of the exact chat input DOM node/draft across Files toggles. No production records are touched.

| 2026-10-22 | Route signed-in home directly to Agent Manager; introduce the editorial anonymous Landing, consistent lime character, dark circular avatar favicon and exact CMA tab title; double the portal logo. Replace the Agent banner with a compact toolbar, accessible Sandbox preview, real Share action and adjustable file panel while retaining the conversation and draft. |


### Document Artifacts

The authenticated Artifacts navigation entry opens `/artifacts`. Its All, Yours, and Shared with you filters and debounced search call the public document registry with cursor pagination. A Docs tile creates a document through the same public API. Cards use authoritative title, access, and update metadata; they do not fetch every source or invent document previews. Opening a document uses `/artifacts/doc_<32 lowercase hex>`, including links emitted inside Agent conversations, without a full page reload.

The document workspace has an editable owner title, explicit Save state, Share, and Comments controls. A compact formatting toolbar sits above an approximately 864 px centered document. The paper scrolls independently from the header. The right Comments column becomes an overlay within the workspace on narrow screens. Shared readers receive no editing toolbar or Save/Share controls; comment permission enables comment posting only. Shared Modal, Button, Picker, Menu, Alert, focus handling, tokens, and reduced-motion conventions apply.

The rich editor is lazy-loaded Tiptap 3.32.0 with Markdown as durable source. It supports headings, bold, italic, underline, lists/checklists, tables with row/column controls, images by URL or bounded raster paste/drop/upload, code, quotes, divider, date, dropdown details, fonts, and Mermaid fenced diagrams. Mermaid is lazy, strict, bounded, and SVG-sanitized; executable image URLs and SVG data images are rejected. A document is at most 1 MiB. Embedded image uploads are at most 600 KiB to leave room for Markdown/base64 overhead.

Standard formatting uses normal Markdown/GFM. Additional formatting uses these explicit source forms:

- Font: `[font family="serif"]Text[/font]`; families are only `sans`, `serif`, `mono`.
- Date: `[date value="2026-10-22"]`.
- Underline: the Tiptap Markdown extension’s supported inline underline HTML.
- Dropdown details: nested Tiptap block directives, for example:

````markdown
:::details {open=true}

:::detailsSummary

More information

:::

:::detailsContent

A paragraph, list or table inside the dropdown.

:::

:::
````

Save uses an exact revision compare-and-swap and idempotency key. Uncertain saves keep the original request body/key, including while navigating within this tab. Unsaved drafts are bounded, memory-only, viewer/document-scoped, and reauthorized before restoration. A newer source never silently changes the draft's original baseline or enables overwriting another editor's work; a conflict retains the draft and offers explicit download/discard/reload. Metadata changes (comments/sharing) retain the loaded source unless their source revision changed, which triggers the same conflict protection.

Owners grant view/comment access by NyxID subject or create revocable view/comment links. Link authority is generated in-browser, appears only in URL fragments, and is sent only in `X-CMA-Document-Share-Token`. A tab-local, expiring session record preserves fragment authority through sign-in while OAuth return URLs contain only the document path. Existing Agent recipient search requires an Agent ID/owner authority, so standalone documents do not call it with an invented Agent.


| 2026-10-22 | Add the persistent document Artifact registry, rich Markdown editor, revision-safe drafts, secure diagram/image rendering, direct sharing and authenticated share links, and comments using the common controls and motion system. |


The title menu also exposes owner-only Rename Agent and Update system prompt when the public preferences client is available. Rename remains available during work and updates the current header/sidebar through the controller without reloading or remounting the conversation. System prompt changes require an idle Agent and a runtime reporting real support; older runtimes explain the unavailable capability. Both forms use the shared Modal and public revision CAS. Uncertain replies retain the original body/idempotency key for exact retry; conflicts retain the draft until an explicit reload. A real browser check keeps the same composer DOM node and its unsent text through both successful changes.


| 2026-10-22 | Preserve the live composer through revision-safe Agent rename and owner system-prompt dialogs, retaining uncertain edits for exact retry and explaining unavailable native capabilities. |


### Quiet document workspace (2026-10-23)

Artifacts uses an editorial 1184px maximum gallery, 48px/32px page insets,
32px/40px weight-500 title with -0.03em tracking and a 14px secondary introduction.
The New document action is the single primary. A compact 280px creation tile
uses a 40px icon seat, 16px padding, 12px radius and neutral hover; it does not
pretend to preview source. Registry cards use 16px gaps, 240px minimum columns,
12px radii, a quiet layer-2 preview, 20px/28px title and 16px metadata insets.
Hover changes border and ground only, never position. Empty results and loading
keep the same content geometry, with the shared skeleton for loading.

The document header uses 56px minimum height, 12px/20px insets, a 14px medium
title and a compact readable save status. Edit and Preview are named pressed
buttons in one neutral 8px-radius group; preview changes only presentation and
keeps the same editor, undo history, draft and explicit Save. Shared readers
remain read-only. The 48px formatting row groups paragraph/font, emphasis,
insert and history controls with quiet vertical separators. The document lives
on a layer-1 sheet on a layer-2 canvas: 864px maximum width, 24px margin, 48px
insets, radius 12 and a border-l1 edge. Body is 16px/28px; headings use 32/40,
24/32 and 20/28 with weights 600, 600 and 500. Comments are a 300px layer-1
column with plain 13px/20px text, 16px insets and 12px-radius comment surfaces.
Sharing retains the shared centered Modal and all existing consent controls.

On this page opens a quiet 180px outline beside the paper on wide documents,
with 12px/18px rows and 12px nested heading indentation. On narrow surfaces it
stacks above the document. It derives up to 64 actual headings from editor
transactions, without polling or extra API calls; selecting one scrolls to that
heading without changing the document or editor selection. Empty outlines say
that headings will appear there. Preview, outline and comment changes retain
the editor DOM. Shared 200ms opacity/grid transitions and reduced-motion rules
apply; content text and hover positions never animate. Below 640px, document
insets reduce to 20px/16px, controls wrap within the viewport, and phone targets
retain the shared 44px minimum.

| 2026-10-23 | Refine Artifacts into a calm document workspace with an editorial gallery, grouped formatting toolbar, readable paper surface, draft-preserving Preview, event-driven heading outline, quieter comments and shared light/dark motion. |

### Legible browser favicon (2026-10-23)

Keep the exact conversation-avatar geometry, lime #C3FD43 mark, #18181B
circular tile and #27272A outline. The mark viewport is `12 7.25 76 76`, filling
the 32px canvas with an approximately 30px visible glyph; this supersedes the
20px inner-square treatment. The 16, 32, 48, 64, 180 and 512px raster fallbacks
derive from that SVG, and content-hashed URLs refresh retained browser icons.

| 2026-10-23 | Enlarge the existing favicon glyph within its circular tile for legibility in browser tabs while preserving its geometry and fixed CMA palette. |


| 2026-10-23 | Render Mermaid diagrams with the current CMA alias palette, including connector and edge-label contrast, and re-render on appearance changes without changing source. Keep narrow-canvas outline destinations below wrapped formatting controls with a 128px scroll margin. |

### Event Gateway delivery map (2026-10-26)

A selected channel opens on a readable, interactive delivery map: every configured
producer on the left, the channel in the middle, and the implicit default plus
all recorded additional consumers on the right. Keep the existing channel table,
sidebar, breadcrumbs and retained channel instances. The map describes configured
connections, not live delivery or health. Connector lines are static; no animated
traffic or green health claim is inferred from configuration. A pending consumer
is explicitly labelled "Binding pending" and an unattached relay "No routes".

The detail has a 1440px maximum width. At 1080px of available detail width,
the map and inspector sit alongside each other in minmax(0,1fr)/360px columns
with a 24px gap; below that they stack without changing component identity. The map uses a layer-2 ground, 1px border-l2,
16px radius, 24px insets and a 24px dotted grid in border-l1. Its header uses
16/24 medium text and a 13/20 secondary explanation. Inside, three columns use
minmax(0,1fr), minmax(140px,.8fr), minmax(0,1fr), with 40px connection gutters.
Each column has a 12/18 medium title and a readable count, and its node list is
vertically centred with 12px gaps. Nodes are full-width real buttons, at least
88px high, 12px insets, 12px radius and a layer-1 fill; names are 14/20 medium,
metadata 12/18 secondary, and the 32px icon seat has an 8px radius and layer-3
fill. Long names and metadata truncate, with their full value in a title.
Selected nodes use the business-primary border and accent wash; hover changes
only border and ground. The central channel has 16px insets and a business-tone
icon seat. Decorative 1px border-l3 connectors join the node edges through a
shared vertical rail; the producer/channel/consumer order conveys direction.
The map has accessible stage labels, natural producer/channel/consumer tab order,
named pressed-state node controls and the existing focus ring. It is not a
canvas requiring pointer gestures.

Below 600px of available map width, the map becomes a vertical roadmap in
producer/channel/consumer reading order with a quiet vertical connector and no
horizontal overflow. Its insets are 16px, every action remains at least 44px, and
node text stays within the viewport. This layout follows the detail's width via
a container query, so an expanded navigation rail cannot squeeze the map.

Selecting a node opens its inspector alongside the map (below on narrower surfaces): a layer-1 surface with a
1px border-l2 edge, radius 12, 20px insets, a 16/24 medium title and 13/20 secondary
purpose. Read-only facts are a definition list with two equal columns, 16px gaps,
12/18 secondary labels and 14/22 primary values; opaque IDs wrap. At 640px these
facts stack. Render only named public configuration fields, never a raw JSON dump
or credential. The channel inspector contains endpoints and the recorded routing
summary; producers show their platform, routes and admission policy; consumers
show their binding, target, reply policy and pending state. Tests identify their
actual target and side effects. A provider test is never described as proof of
platform ingress or message delivery. An uncertain outcome stays uncertain and
never triggers an automatic retry. Tests and edits retain state across node and
channel navigation; opening a node sends no mutation or extra resource request.

Each node is the only entry to its own settings and tests. There is no whole-channel
configuration form or editor toggle in channel detail. Creation retains its initial
setup form. The structured editor reuses service-form and shared Input, Picker and Button.
`.channel-form-section` is a min-width-zero fieldset with a 1px border-l2 edge,
12px radius, 16px insets and 16px gaps; its legend is 14/22 medium.
`.channel-form-grid` is two equal minmax-zero columns with 16px gaps, stacking at
640px. `.channel-form-row` wraps aligned controls with an 8px gap. A
`.channel-value-field` uses 14/22 text and wraps opaque IDs; `.channel-field-hint`
is 12/18 secondary text with 4px top spacing. The editor stays mounted while
changing the inspected node, retaining the exact draft, original revision and
uncertain request. Read-only permissions remove mutation controls, not context.

Node selection and inspector changes use the shared 200ms opacity and
border/background transitions. Neither selection nor hover moves content;
`prefers-reduced-motion: reduce` removes every map transition and entrance.
No timers, polling or graph library are introduced for this projection.

| 2026-10-26 | Replace the channel JSON disclosure with an interactive producer → channel → consumer delivery map and scoped readable inspectors, preserving channel navigation, drafts, permission checks and honest test outcomes. Establish responsive map geometry, structured field groups and reduced-motion selection transitions. |

Map nodes carry their actual platform/provider identity in the existing 32px icon
seat. Telegram, Discord and WhatsApp use locally bundled CC0 Simple Icons paths;
Lark/Feishu reuse the user-supplied bird geometry already used in NyxID, with
Feishu's outline variant. CMA consumers reuse the existing CMA face SVG; Nyxbot
uses the existing NyxID identity glyph. Monochrome paths use currentColor and
existing aliases; the CMA face retains its approved fixed palette. The mark is
20px and decorative because the node names its platform/provider in text.
Unknown providers retain a generic Agent glyph and their literal provider name.
No logo fetch, dependency, palette or network-derived health indicator is added.

| 2026-10-26 | Identify producer and consumer nodes with local platform and provider glyphs; make each node the sole entry to its own settings and real targeted tests in a retained adjacent inspector. |

For channel editors with update permission, Add producer is a shared quiet button
with 12px top spacing in the producer column. It opens an explicitly unsaved
producer in the same inspector; tests become available only after save. Existing
producer and consumer nodes continue to describe saved configuration. On stacked
layouts, selecting a node brings the inspector into view with smooth scrolling
and a 20px top scroll margin; reduced motion uses immediate scrolling. The
scroll responds only to the user's selection and does not poll or change drafts.

| 2026-10-26 | Keep a scoped unsaved producer in the map inspector until its versioned save and bring selected details into view on narrow layouts, preserving reduced-motion behavior. |

Inside the node inspector, the form's outer fieldset and nested fieldsets keep
their semantic legends but omit their extra borders and padding, with 16px block
spacing. The inspector supplies the enclosing edge, avoiding repeated boxes
around a single setting group. Test outcomes retained after a configuration
change explicitly say that settings changed since that test was requested.

| 2026-10-26 | Keep scoped setting groups visually quiet within the inspector and distinguish retained test results from verification of subsequently changed settings. |


### Relay onboarding and capability discovery (2026-10-26)

The structured producer editor keeps the same Section, Picker, Input and Button
primitives. Verification choices that this gateway cannot currently write stay
visible and disabled with an inline reason. Saved and drafted choices remain
visible; loading capabilities never rewrites them or selects normalized trust.
Explain the platform's NyxID setup and exact route identifiers beneath its
fields. A failed capability check offers one explicit retry, with no polling.
All editors share discovery within the current owner/permission context.

Creation offers an explicit existing CMA-managed Telegram connection choice.
Its owner-scoped list loads only when selected, supports manual pagination,
and labels unavailable connection states. Switching back restores manual
producer drafts and credentials. Admission starts with an empty allowlist.
NyxID relay routes are never labelled as CMA-managed connection references,
and platform verification support is never presented as native bot creation.

| 2026-10-26 | Add capability-aware relay verification and platform setup guidance, preserving drafts and exact retries; distinguish owned Telegram connection setup from NyxID relay onboarding with explicit admission and no background polling. |


### Owner-initiated channel messages (2026-10-26)

The existing producer inspector may include a structured message composer only
when the owner has update permission and the gateway advertises initiated sends.
Use the existing Choice, TextField, Button, Alert and Presence primitives and
inspector section styling. Show saved route choices only; never ask for arbitrary
recipient metadata or credentials. Preparing retains the exact destination and
message for review before the separate Send action. Keep that snapshot and its
idempotency key while changing inspected nodes or recovering an uncertain request.
No polling or automatic resend is permitted. Confirmed, cancelled and unknown
outcomes use plain language; unknown offers an explicit status check and retains
its delivery reference. The existing Presence transition respects reduced motion.

| 2026-10-26 | Add explicit prepare, review, send, cancel and status recovery in the existing producer inspector for owner-initiated messages, retaining uncertain operations and their exact destination. |


Owner-initiated message recovery retains the exact request, key and receipt in
per-tab session storage scoped to the current owner and stable channel record.
Save before network effects. A reload offers an explicit status check, never an
automatic send, and account changes reset the visible composer. Storage failure
blocks sending with an inline explanation. Use “Review message”, “Send message”
and “Cancel” for the explicit actions; avoid internal preparation terminology.

| 2026-10-26 | Retain owner/channel message recovery across reloads with original keys, explicit status checks and plain review/send labels. |

### Initiated-message native media (2026-10-26)

The producer inspector's existing message composer may show Media attachments
when the gateway explicitly advertises native-media writes for a supported relay
platform. An absent capability keeps new media controls hidden. Reuse Section,
Choice, TextField and Button, the existing channel form grid, row and hint styles;
introduce no new colors, spacing or motion. Each of at most four entries has a
stable identity, an Image/File/Audio/Video picker and a required HTTPS URL, with
optional filename, MIME type and caption. Remove affects only that entry. Show
an inline validation reason and prevent review while any attachment is invalid.
Message text is optional with valid media. Do not expose raw JSON or arbitrary
recipient fields, and do not fetch previews when the inspector opens.

Review shows every media kind, exact URL, optional metadata and saved destination
as readable wrapping text. The separate Send action submits the exact reviewed
body. Opening, returning or reloading never sends automatically. Preserve version
1 text journals without reshaping requests; new media journals use version 2 and
retain their exact body, idempotency key and uncertain receipt. Gate changes keep
saved review, Cancel and Check delivery status available while blocking new
media sends. Unsaved media fields are retained when temporarily hidden. Media
send requests allow the gateway's longer upload budget before showing uncertainty;
review, cancellation and status requests retain the normal short budget. Reuse
the existing Presence notice transition and reduced-motion behavior.

| 2026-10-26 | Add typed URL-based native media to initiated channel messages with up to four image/file/audio/video entries, explicit review/send, media-only support, validation and exact versioned recovery. Preserve saved media and status access when write capabilities change. |

| 2026-10-26 | Keep Check saved request available after an uncertain initiated-message preparation when either write capability disappears. Recover only the retained exact request; new review and Send remain gated, while recovered status and cancellation stay available. |

| 2026-10-26 | Return an initiated-message draft to editing after a definite first HTTP 413 preparation rejection. Preserve any earlier uncertain request and its key when a recovery attempt receives the same rejection. |

### Event Gateway flow workspace (2026-10-27)

The Event Gateway uses the Agent Manager's shared PortalFrame, sidebar rows,
Geist typography, Button, Input, Picker, Modal, Alert, Presence and theme aliases.
The following replaces the delivery map's adjacent inspector, table-first entry
and prohibition on a graph library. It introduces no separate visual system.

Channels and Consumers are expandable sidebar groups. Their retained lists sit
directly beneath the group control and use the shared quiet navigation treatment.
Rows show the authoritative name, creation time and creator; unavailable metadata
reads "Not reported" instead of borrowing the current viewer's identity or time.
The selected row remains visible through details and editor navigation. Lists
retain loading, empty, error, explicit retry and bounded pagination controls.
Before selection the main area contains one New channel or New consumer action
and an illustrative React Flow diagram. Its copy clearly describes a possible
event journey; it never represents sample activity as the owner's real data.

A selected Channel's React Flow canvas fills the available main area below its
compact PageHeader, without a reading-width cap. Producers occupy the left,
the Channel the middle and Consumers the right. The canvas has the existing
24px dotted ground and 24px edge clearance. Cards retain 12px radius, 12px
insets, 32px icon seats, 14/20 medium names and 12/18 secondary metadata.
They are 240px wide with an 88px minimum height; horizontal layout steps are
360px and vertical steps 120px. Existing aliases supply all fills, borders,
handles and connector colors. Fit, zoom and pan are accessible controls; nodes
remain keyboard-activatable and connections are not editable by dragging.
An accessible reading-order list exposes the same node actions on small screens
and to assistive technology. Pan/zoom and selection never mutate configuration.

Node activation opens the shared centered Modal. The configuration shell is
fixed at min(960px,60vw) by 60dvh and never exceeds 60vw or 60dvh, including
on narrow screens, preserving at least 20% page clearance on every edge.
Its header and close control remain fixed. A 144px left section-title sidebar
provides jump links to Overview, Configuration, Send message, Tests, Activity
and Delete as applicable. The content scrolls internally; at 720px viewport
width the section links wrap above the scroll area. Selected-node changes and
closing/reopening the popup preserve drafts, exact uncertain requests, results
and revisions. Hidden retained dialogs are inert, excluded from accessibility
and release their focus trap and page scroll lock. Only one popup is interactive.

Use the existing 200ms opacity/8px arrival motion, quiet hover background and
border transitions, and shared easing. Illustrative connections may animate a
slow 3s dash progression; real configuration connections remain static because
they do not represent observed traffic. Reduced motion removes flow animation,
viewport transitions, entrance transforms and smooth section scrolling.

Channel Activity shows only public API evidence and receives live changes from
the authenticated public event subscription. Initial history and live updates
merge by event identity and sequence without periodic polling. Show recorded
source receipt, target dispatch, producing agent/time and recipient delivery
separately. Queued, accepted, delivered, failed and unknown retain their actual
meaning; an absent delivery receipt is never displayed as sent. Live status uses
the shared quiet status treatment, with explicit retry for a disconnected feed.
Changing channel or identity closes the previous subscription and rejects its
late responses. Opening Activity never sends an event or starts agent work.

| 2026-10-27 | Standardize Event Gateway on Agent Manager primitives; introduce a full-area accessible React Flow workspace, expandable resource navigation, illustrative entry screens, retained centered node configuration with section navigation, and evidence-based live Channel Activity. |

### Selected-channel Activity preview (2026-10-27)

Keep the illustrative welcome diagram until a channel is explicitly selected.
Selection opens one authenticated Activity subscription for that active channel;
inactive retained channel views close their subscriptions. Returning starts a new
subscription and receives its complete current snapshot. The map and retained
Activity dialog share that feed and snapshot. Opening or closing the dialog
never opens another subscription or starts work.

Below the map heading, show a compact Latest activity region with connection
status and the newest receipt from the authoritative snapshot: source, actor,
received timestamp and each reported consumer state. With no consumer record,
label the event Received. Do not synthesize a combined lifecycle state, traffic
animation, event count or receipt time. Waiting for a snapshot differs from an
authoritative empty history. Keep the last valid evidence visible during a
reconnect, beside the current connection status. Native recoverable reconnects
have no application retry limit; a terminal closed stream or malformed snapshot
requires the existing explicit Reconnect activity action.

The preview uses existing theme aliases, a level-2 border, 12px corner radius,
12px insets, 8px wrapping gaps and 24px horizontal map margins (16px at narrow
viewport widths). Use 13/20 primary text, medium headings and 12/18 secondary
metadata. Each newly reported event uses the shared Presence arrival transition
and reduced-motion behavior; updates to the same event do not restart it.

| 2026-10-27 | Start one Activity stream on explicit channel selection; share its authoritative latest receipt with the map and retained dialog, close inactive streams and preserve native recoverable reconnects without an arbitrary error cap. |

### Event Gateway configuration clarity and toolsets (2026-10-27)

Node configuration retains the shared centered 60vw × 60dvh Modal. Inside, a
160px section navigation column uses layer 2, 16px vertical/12px horizontal
padding, 32px minimum section buttons and the existing accent selected state.
The independently scrolling content uses 24px padding. The navigation lists
actual top-level settings (connection, admission, grouping, replies, routing,
model, permissions and credentials), in document order; conditional fields
update this list without polling. Selecting a section scrolls and focuses it.
Scrolling updates the highlighted section. At 720px it becomes one horizontal
scrolling row of 44px touch targets and the content padding becomes 16px.

Forms use one vertical reading rhythm: 8px label/control gaps, 16px field gaps,
24px section spacing and thin border-l2 section divisions. Short explanatory
copy uses 13px/20px; hints use 12px/18px secondary ink. Two-column groups collapse
below a 560px content container. Repeated field/remove rows align at their lower
edge. Checkboxes align with the first text line and can carry a subordinate
explanation. Ownership, model/behavior, services/instructions, skills and extra
MCP servers have distinct section destinations. Unknown wire values remain
visible and retained. UI labels may explain enum values without changing them.

An Agent toolsets node describes the gateway's existing event tools and connects
to consumers with a static dashed association. Its inspector separates core
event tools, optional media/message effects, and the connected consumers. Tool
rows show a plain-language action, exact tool name and its restrictions. It does
not imply that a tool is installed in an existing Agent, that a platform supports
every action, or that an optional server gate is enabled. No discovery requests,
permission changes or installations are triggered by inspecting it. Opening
Toolsets preserves the existing node editor and unsaved drafts. Section movement
uses existing smooth motion and honors reduced motion.

| Date | Change |
| --- | --- |
| 2026-10-27 | Organize node configuration by actual settings with aligned fields, contextual explanations and active section navigation; expose a truthful Agent toolsets catalog while preserving node drafts. |

### Shared Gateway navigation and map controls (2026-10-27)

Channels and Consumers now use the Agent Manager's same text view tabs and
single-line TreeRow navigation, including quiet 32px rows (44px on narrow
screens), selected fill, keyboard navigation, and metadata revealed on focus,
hover or selection. Creation uses the shared top sidebar action, with the
existing Channel branch or Consumer Agent glyph. Retained content and drafts
survive section changes. The prior expandable group layout is superseded.

The map's Activity and Add producer actions float at the top right beside a
compact preview of the latest three authoritative Activity receipts. The preview
shares the selected channel's existing feed and preserves per-consumer outcomes,
connection, waiting and empty states. Its 12px insets, radius and secondary 12/18
metadata reuse existing tokens. Fit and resize reserve space for this overlay.
The bottom Browse delivery nodes disclosure is removed; canvas node buttons
retain their names, pressed states and ordinary keyboard activation.
Below 640px of canvas width, nodes stack in reading order with top-to-bottom
arrows; the canvas grows to preserve readable node size below its controls.

Configured event edges have arrowheads and a slow three-second directional dash
progression. This indicates routing direction, not live traffic; Activity is the
evidence of events. Reduced motion leaves static arrows and removes viewport
transitions. The optional Agent toolsets node sits below the Channel; its dashed,
static, labeled Tool access associations point to Consumers and do not imply
that tools publish traffic. All controls reuse shared hover and focus treatment.

| 2026-10-27 | Reuse Agent Manager resource tabs, single-line navigation and top creation actions in Event Gateway; float map actions beside three actual Activity receipts, remove the duplicate node browser, and show routing direction with reduced-motion-aware arrows and separate static tool-access associations. |

The detail header uses 24px top and side padding so breadcrumbs never touch the
viewport edge. The latest-activity preview scrolls within 320px or 40dvh, whichever
is smaller, so a large recipient list cannot consume the map. Add producer remains
visible but disabled at the channel's four-producer limit, with an explanation.
Send message and Tests section destinations appear only when their controls are
available; their retained components keep pending operations while hidden.

| 2026-10-27 | Finish Gateway detail spacing, bound the activity preview, and keep configuration navigation limited to available controls without discarding pending sends. |

Configuration add/remove and save actions keep intrinsic widths and align to the
start of their section; they do not stretch across the form's grid columns.

Delivery map edges use a 1.5px stroke in `--dsw-alias-label-tertiary` so the
direction remains legible in both themes. Arrowheads use the same ink. The
built-page style check verifies these values on the actual rendered paths.

| 2026-10-27 | Record the delivery map's directional edge weight and paint, and align built-page verification with shared sidebar rows. |

### Lark and Feishu resource connection selection (2026-10-27)

The existing embedded-media field group offers **Load my bot connections** using
the shared Button and Picker. Discovery begins only after this explicit action;
refresh/retry is explicit, with no polling. Options name the connection and its
exact ID and Personal/Organization provenance so identical labels remain distinguishable. Selecting an option sets
only the connection/catalog pair. Receiving app ID and independently confirmed
Bot ID remain separate fields; selection is not described as granting access.
Use existing channel-field-hint typography for loading, unavailable and empty
states. The manual-ID toggle preserves existing and unavailable bindings without
rewriting them. Eligible allowed organization connections are included; selection
never claims that a channel key is granted access. Capability-off bindings stay read-only with
removal available. Owner/permission changes unmount the discovery context, and
region changes discard in-flight results. No new palette, spacing or popup is added.

| 2026-10-27 | Add explicit current-subject Lark/Feishu connection discovery with the shared Picker, retaining manual bindings, receiving identities and owner/region isolation without polling or inferred grants. |

Configuration section navigation keeps the explicitly selected destination
highlighted during smooth movement and when the final destination cannot align
with the content's top edge. Wheel, touch, scrollbar or keyboard scrolling
returns highlighting to the visible section; the last section is current at the
bottom only when the content actually overflows. A panel that fits opens on its
first section and retains explicit section selections without requiring scroll.
The existing smooth and reduced-motion navigation behaviors remain unchanged.

| 2026-10-27 | Keep configuration navigation highlights on the requested section through bottom-clamped scrolling, resume position tracking on user scroll, and preserve first/explicit selections when all content fits. |

Channel Activity and its map preview label the recorded event category using
the existing heading and metadata styles: Message received, Message edited,
Message deleted, Reaction updated, Interaction received or Command received.
Unknown or historical categories use Event received. An unknown actor reads
Actor not reported. Event receipts show Source event time only from the explicit
source timestamp, otherwise Not reported; the separate Received timestamp always
means gateway receipt. Historical and ordinary receipt timestamps retain the
Recorded event time label without claiming their source. These labels share the
existing Presence animation and do not introduce another feed or polling.

| 2026-10-27 | Distinguish message changes from new messages in Activity and its preview, retaining unknown actors and source times without inferring them from gateway receipt. |

Direct WhatsApp setup uses the existing producer configuration popup and shared
Section, TextField, Choice and endpoint-copy components. Account identifiers and
private credentials are separate titled groups within that producer. All three
credentials are masked and rotate together; blank fields preserve saved values,
and confirmed saves clear the local secret inputs. Private-chat admission hides
irrelevant group and bot-mention controls. Only credentials for selected producer
types appear during channel creation. Gateway discovery explicitly enables new
setup; unavailable support preserves existing sources and drafts. The Meta
callback appears under its own WhatsApp webhook label. No new modal, palette,
spacing, motion or background request is introduced.

| 2026-10-27 | Add structured direct WhatsApp account and atomic credential controls to the producer inspector, preserve disabled/saved configurations, and show the exact Meta callback with shared endpoint-copy UI. |

The Agent toolsets inspector now lists only tools advertised by the gateway's
shared owner-scoped capability discovery. Tool names and descriptions come from
that catalog; known names may retain their plain-language labels. Read-only and
destructive annotations use the existing secondary description text. An absent
or null catalog means availability is not reported; an explicit empty catalog
means no tools are advertised. Loading and failure use the shared field hint and
explicit retry Button. Catalog rows use the existing Presence opacity transition
and reduced-motion behavior. This supersedes the static core/optional tool list:
opening the inspector adds no discovery request or polling, and catalog entries
do not establish platform permissions or installation in connected agents.

| 2026-10-27 | Render the authoritative advertised tool catalog in the existing inspector, distinguish unknown/empty/loading/error states, and preserve shared discovery, agent-installation caveats and reduced-motion arrival. |

Completed Agent turns show **Worked for 2 hours 3 minutes 4 seconds.** below
their final reply, before token usage and the message footer. Use the existing
12/18 caption typography without a background, border or new spacing. This
duration stays visible at rest. Omit unused leading hours/minutes, use singular
units for one, and retain hours beyond 24. Derive elapsed time from recorded turn
start and terminal timestamps; approval pauses, reconnects and route changes do
not restart the clock. An approval wait is not completion. Unknown or invalid
timestamps show no duration; history replay restores the recorded value.

| 2026-10-28 | Retain recorded elapsed time below a completed turn's final reply, including long turns, replay and approval/reconnect continuity. |

Agent replies display Markdown images and linked image, audio and video files
within the existing message column. Images/video keep their natural aspect ratio
and never exceed the column width; audio/video use native keyboard-accessible
controls without autoplay. Reuse the image's 8px radius and the existing 4px
caption gap. Loading and failed previews use 12/18 caption text and the compact
Retry preview Button. File previews require current file-view access and load
through the public Agent file API when scrolled into view. Navigation or access
loss cancels pending reads and releases previews. Unsafe URLs and raw HTML remain
inert, and unavailable files keep their visible label instead of disappearing.

| 2026-10-28 | Render authorized local and remote reply media with bounded layout, native playback, lazy file reads, and visible preview recovery. |

| 2026-10-28 | Distinguish WhatsApp API acceptance from authenticated sent, delivered, read, played and failed receipts in the existing animated Channel Activity rows. Preserve provider timestamps, original recipients and independent failure evidence; add no polling, new layout or notifications. |

New channel uses one 24px page inset for its header, fields and actions, reduced
to 16px below 600px, matching the existing service-page spacing. The delivery map
retains its full-width workspace.

Agent toolsets describes the gateway's advertised catalog and current channel
source setup. Known Discord thread/history and WhatsApp form preparation tools
whose direct source or saved consent is absent remain available in the existing
native disclosure pattern, **Other advertised tools**, with concise setup hints.
Matching source settings do not promise event support, permissions or agent
installation. Other tools, including future advertised names and historical form
receipt reads, remain conditional on the event and retain their server text.
Catalog associations are static dashed **Catalog reference** lines without
arrowheads; pending consumers have no catalog association. This supersedes the
earlier **Tool access** label and arrowheads while retaining routing arrows.

| 2026-10-28 | Align New channel header, form and actions with shared page spacing; show source-specific setup hints for advertised tools, preserve the complete catalog, and distinguish catalog references from confirmed tool access. |

The tool node and inspector use the plain title **Event tools** with **Tools and
channel setup** on the map. Static dashed associations are labeled **Tools**.
Tools with missing source settings remain under **Tools requiring setup** with
the existing concise source hints. The inspector says that available tools depend
on the event, platform and permissions. Connected agents state pending, existing
setup or requested tools directly; internal installation and receipt caveats stay
in implementation documentation. These labels supersede the catalog wording above.

| 2026-10-28 | Use plain Event tools, Tools and Tools requiring setup labels; keep configuration states concise while preserving conditional availability and the complete server catalog. |

The tool inspector keeps its catalog section's accessible name without a visible
parent heading above the Event tools fieldset. The fieldset retains its sidebar
anchor and the single availability explanation; Overview describes the tools'
purpose without repeating that explanation.

| 2026-10-28 | Remove the redundant tool catalog heading and overview availability copy while retaining the Event tools section anchor and accessible catalog region. |

Answer feedback appears only below a recorded final Assistant answer in a
successfully completed turn, identified by its durable turn and message sequence.
Intermediate replies, active turns, errors, reasoning and tools retain no vote
entry. Shared and read-only readers retain Copy content without voting controls.
Use **Helpful** and **Needs improvement** for the existing like/dislike keys.
Clicking the selected key removes the vote; clicking the other changes it.
Keep the last confirmed selection while saving, disable both keys for that
answer, and announce Saving vote, Vote recorded or Vote removed in a polite
status. A failed or uncertain save retains its exact requested vote and offers
**Retry vote** before another choice can be submitted. A failed initial read
offers **Retry feedback**. Changing the viewer or Agent hides old state immediately.
Reuse the footer's 28px keys, 4px gaps, 12/18 caption text, filled selected glyphs,
business-primary ink, fast opacity transition and reduced-motion rules. Reveal
actions on hover or keyboard focus; selected votes and saving/error/success
statuses remain visible. At widths up to 30rem or on coarse pointers, keep all
actions visible with at least 44px targets. Negative-reason details are separate.

| 2026-11-03 | Restrict owner feedback to completed final answers; specify confirmed toggle/undo, exact retry, read recovery, permission isolation and accessible desktop/mobile status controls for ticket 09. |

### Task navigation and environment settings (2026-11-03)

Agent Manager navigation defaults to **Tasks**, without an Agents/Sandboxes
switch. The sidebar **Settings** menu contains **Manage environments**, opening
the existing Sandbox hierarchy. **Back to tasks** restores the task selection,
scope, status filter, scroll position and keyboard focus. Environment navigation
retains its own status filter and expansion. Existing resource links and scoped
creation actions keep their public routes and lifecycle eligibility. Switching
navigation modes does not replace the current conversation or its draft.

Task rows use the existing stacked tree-row geometry: a truncated title with
its full hover text, then a visible status word and the existing state dot.
Manual title, eligible runtime description and configured title keep their
existing priority; an empty title reads **Untitled task**, never an internal ID.
Tasks use **Ready to start**, **Completed**, **Working**, **Waiting for approval**,
**Preparing**, **Paused**, **Stopped**, and **Failed**. Ready to start requires
confirmed absence of task turns. A completed first message explicitly recorded
as setup, matched by turn sequence, still reads Ready to start. The same first
turn with unknown legacy purpose reads Status unavailable; a later successful
task turn reads Completed. Message text and title are never evidence of purpose.
Completed requires a durable successful last task turn
and no active, queued or admitted work. Missing evidence reads **Status
unavailable**. Failure, interruption and parent lifecycle barriers take priority
over an old success. These are task facts; resource lifecycle states are unchanged.
Completed and Ready to start use the existing neutral tone, working/preparation
the active tone, approval the warning tone, failure the error tone, and paused/
stopped the muted tone. Color never replaces the visible word.

The Settings and Back to tasks controls reuse quiet sidebar rows and shared
menus, 32px on desktop and at least 44px below 30rem. The Tasks heading uses the
existing 13px secondary filter label. Personal, Organization and Shared remain
in Filter alongside task status. Long titles never displace status. Narrow
screens retain the navigation sheet, Escape handling and focus return. Empty
lists state that no tasks match; failed refreshes retain already shown tasks
with an alert and retry. Stale scoped rows remain readable but cannot navigate
or act until access is checked again; denied access removes them. Stale rows omit
the state dot and keep the last known state word alongside the refresh warning.

| 2026-11-03 | Make task navigation the default, move Sandbox browsing under Settings, display authorized last-turn state with visible words, and preserve navigation position and lifecycle barriers. |

| 2026-11-03 | Distinguish completed setup from completed tasks using recorded first-message purpose and turn identity; leave unknown legacy first-turn purpose unavailable. |

### Task home and durable initial task (2026-11-03)

Signed-in Home and ordinary New agent actions open a task composer. Use one
20/28 heading, a labeled multiline field, a primary **Start task** action and
editable Research, Spreadsheet, Document, Website and Code templates. Templates
only replace the unsent draft. Preserve the existing anonymous sign-in surface.
Use existing field, button, secondary text and status tokens; the task column is
at most 48rem wide, with 24px inset and 16px gaps, reduced to 16px inset below
600px. The textarea has a 160px minimum height; template and action hit areas
are at least 44px. Contextual creation names its fixed Workspace or Sandbox.

Freeze exact task text and creation context before sending. Retain a separate
editable next draft, scoped to authenticated viewer, origin and creation context
in tab session storage. Refresh restores it without submitting. Sign-out/account
change clears task drafts and creation intents. Unavailable storage blocks a
new submission with a visible explanation and leaves selectable task text.
Whitespace-only input and text above 262144 UTF-8 bytes are visible errors.

Show the retained task separately from the next draft. Receipt means accepted,
not completed; its identity is an operation, never an Agent. Uncertain requests
offer **Recover original task** with the original body and key. Accepted requests
read public status; only a verified Agent identity opens the preparing conversation.
Preparation, required connections, task delivery failure and uncertain recovery
retain distinct states. A user initial task uses the durable first-message task
purpose after environment preparation; it replaces the automatic readiness turn.
Clients omitting a task retain setup behavior. Unsupported services show a clear
unavailable explanation and never fall back to an unrecorded send.

| 2026-11-03 | Define task-first Home and contextual creation, editable templates, exact durable initial-task admission, tab draft isolation and honest recovery states. |

### Reported plan progress and turn outcomes (2026-11-03)

The existing turn-scoped Plan card also shows **Completed 1/3 steps** and
**Current step 2: <reported title>**. Count only exact `completed` entries and
choose the first exact `in_progress` entry. A known plan with at most one current
step has an accessible progress bar labelled **Reported plan progress**, with
the completed count, total and equivalent text. Unknown or malformed statuses
and multiple current steps keep their reported text, say **Progress unknown**,
and omit the determinate bar. Empty reports clear the card. A replacement plan
uses its new order, without inventing stable step identities. No plan means the
existing Thinking/current tool activity, with no denominator or estimate.

Use the existing card ground, hairline, radius 8 and padding 12; gaps are 8px.
The summary and current step use 13/20 primary/secondary ink. The bar is 4px high,
radius 8, on bg-layer-3 with business-primary fill; it has no animation. Each
step has a decorative existing check, running, waiting or question glyph beside
its explicit status word. The plan disclosure uses a chevron and a 44px target,
inherits typography, and has the existing hover wash and focus outline. Titles
wrap at mobile widths. Do not announce token-by-token updates as plan changes.

On a recorded successful turn, fold the step list behind **View plan and
operations**. Say **Completed all N steps** only if every reported step completed;
otherwise say **Response finished · Plan completion not fully reported**. Failed
and cancelled turns say **Response failed** and **Response cancelled** and retain
the reported step states. Missing terminal evidence says **Outcome unknown**.
The disclosure reopens the plan; adjacent operation disclosures retain their
counts and visible failure summaries. A single operation also starts folded.
Approvals and recovery notices remain independent and visible. Never mark
pending work complete because a response ended.

The compact toolbar has one optional progress slot between identity and actions,
using 12/18 secondary ink, minimum width zero and a 44px link target. It describes
only the active turn (**1/3 steps · Step 2**, or **Progress unknown**), links to that
turn's Plan card, and disappears when the turn has no plan or ends. At narrow
widths it may wrap onto its own row. Ticket 02 owns elapsed time and settings;
its timer may share this slot without repeating plan or activity summaries.
Plan changes never override the conversation's existing unpinned scroll position.
Current activity remains the existing safe tool title/Thinking and never claims
association with a plan step. There is no reliable ETA in the current public
contract, so no estimated remaining time is rendered.

| 2026-11-03 | Add factual turn-scoped plan counts, accessible progress, ambiguous-state fallback, terminal process disclosure and the shared compact header slot for ticket 06; keep approvals visible and omit unsupported ETA. |

Plan counts, current-step text and terminal summaries share one atomic polite
status region, using the card's existing 8px column gap. Only plan or turn-state
changes update it; streamed message tokens do not rerender this region.

| 2026-11-03 | Make reported plan updates and terminal summaries one atomic polite status announcement for ticket 06. |

### Suggested follow-up questions (2026-11-03)

The latest durable, successfully completed answer may have one **Next you can ask**
region after its answer and artifacts. Its source is explicitly **Scenario templates**
until contextual generation is available. An explicit comparison request is the
initial bounded scenario; unrelated, unfinished, failed and superseded answers
have no region. Read-only and shared views have no sending entry point.

The region contains zero to three full-text buttons in rank order. Use existing
secondary text, layer-1 background, border-l1, radius 8, 13/20 type, 8px gaps and
12px padding. Buttons have a minimum 44px touch target, wrap without truncation,
and use the standard focus ring. The region has no fixed height or horizontal
overflow on narrow screens. A caption reports Sending, Not sent, or Awaiting
confirmation through a polite status region. Template attribution uses 12/18
secondary text. No generated-intelligence claim accompanies templates.

An explicit click sends exactly the displayed text through the selected normal
conversation controller and connection gate. Preserve the manual composer draft
and attachments. Disable the whole suggestion group synchronously while sending;
collapse it only on authoritative acceptance or when its answer loses eligibility.
An uncertain request retains its original text, scope and idempotency identity;
explicit recovery verifies that same request. Definite refusal permits retry.
Account, Agent, permission and answer changes invalidate late callbacks. Feedback
actions never submit suggestions. Editing and contextual generation are separate
extensions of the same viewer/Agent/turn/answer/source-version/item contract.

| 2026-11-03 | Specify bounded comparison-template suggestions, exact-text sending, retained manual drafts, group pending/recovery states, answer scoping and accessible wrapping controls for ticket 11. |

### Compact input settings and confirmed completion time (2026-11-03)

The composer defaults to one **Agent settings** disclosure, summarizing the
model catalog label, reasoning effort and permission mode separated by middle
dots. Reuse the quiet composer chip, 12/18 secondary ink, 8px radius and 4px gap.
The summary truncates on one line and exposes the full text as its accessible
name and title. It occupies the flexible space between attachment/steering and
context/send/stop controls. Opening it reveals the existing model/effort and
permission controls in a wrapping row inside the composer, with 4px gaps and
8px top spacing. This supersedes the always-visible separate settings chips.
The disclosure announces its expanded state and controls its named settings
region. It never replaces the composer, draft, attachments or selected transport.
Closing it retains unapplied changes. Apply, recovery, pending confirmation and
errors stay visible below the controls even when the settings row is collapsed.
Existing capability checks, idle-only application and exact request recovery
remain authoritative. Settings changes never send a conversation message.
Below 30rem, the summary gets its own full-width row above the action row;
settings and sending controls retain 44px touch targets and keyboard focus rings.
Long labels cannot displace Send or Stop. Reduced-motion behavior stays unchanged.

Only Responses tabs offer Queue and Interrupt during a running turn. **Queue**
says **Process after the current turn finishes.** **Interrupt** says **Stop the
current turn, then start a new turn with this message.** Acceptance or delivery
is not completion. AG-UI retains ordinary idle Send and active Stop; it must not
advertise unsupported queue or interruption-with-message actions. Existing
approval, uncertain send and control recovery remain independently actionable.

The header has a separate completion slot, independent of plan/activity progress.
It displays **Completed in 4 minutes** for a recorded start at 10:00 and successful
end at 10:04. Use 12/18 caption ink and tabular numerals, with a polite atomic
status announcement. Durations below one second read **Completed in less than
1 second**. Longer durations show whole minutes and remaining whole seconds.
Missing, invalid or reversed timestamps omit the duration; zero is valid only
when both recorded timestamps exist. Never use mount time, last-updated time or
the current clock to fill missing history. A new active turn, failure or
interruption removes the previous completion duration. Loading incomplete history
cannot claim completion. The slot wraps within the header at narrow widths and
does not duplicate task status, plan progress or the active Thinking clock.

| 2026-11-03 | Collapse model, reasoning and permission into one reversible settings disclosure; preserve all input/recovery controls, describe transport-specific queue/interruption truthfully, and show only confirmed successful last-turn elapsed time in an independent header slot for ticket 02. |

The completion display shares the header's existing **progress** slot with the
current-turn plan link. The plan link stays memoized and available while a turn
is active; completion occupies that same space only after confirmed success.
No second activity summary or independent timing authority is introduced.

| 2026-11-03 | Combine ticket 02 elapsed time with ticket 06's existing header progress slot, preserve its current-plan link and reuse recorded turn outcomes. |

### Durable task names (2026-11-03)

Task navigation, conversation headings and the active browser tab use the same
name: owner override, valid provider title, first real task preview, configured
name, then **Untitled task**. Automatic setup previews never supply a task name.
Preserve legacy descriptions with unknown provenance as bounded fallback text;
do not infer purpose from wording. Automatic labels are single-line and bounded
to 64 Unicode characters, with task previews limited to 48. Keep full owner
names within the existing rename limit and existing truncation/hover treatment.

On an Agent page the browser title is **<task name> · CMA**. Loading, denied and
non-Agent pages retain **ChronoAI Managed Agent (CMA)**. Hidden retained services
cannot change the active tab title. Rename and delayed provider metadata update
only presentation: preserve the mounted conversation, composer, draft and focus.
Reload and navigation restore names from authorized persisted resource metadata.

| 2026-11-03 | Unify persisted real-task and owner names across navigation, conversation and browser tabs, with bounded fallback and setup exclusion. |

The Agent computer is a retained sibling of the conversation. On layouts with
room for the existing 640px center and 300px details floor it opens by default;
an explicit collapse is retained. Narrow layouts start closed and offer Show
Agent computer. The existing right overlay has Back to conversation and Escape,
restores focus to its opener, and keeps the composer and reading position mounted.
Details and computer occupy the same selection slot. Files, Operation output and
Browser are named keyboard-operable tabs with 44px targets. The header uses the
existing 48px height, 8px/16px padding and 14/20 title; bodies use 12px gaps/insets,
13/20 secondary explanations and existing code typography. No new colors or motion.

Files retains the authorized explorer and unsaved edits when tabs or panels close.
The existing file API grants mounted-home access; label that scope explicitly and
do not describe it as an isolated Agent filesystem. Operation output is read-only,
grouped by recorded turn and stable tool identity, in event order. It uses the
conversation's existing observation, never a second event subscription. Recorded
status words remain authoritative; missing output says Output not reported,
bounded output says Output truncated. Render output as plain preformatted text.
Browser, replay and control remain unavailable until public capability evidence
supports them; there is no illustrative browser frame, Live label, command input
or PTY. Loading, unsupported, not enabled, forbidden and temporary failure remain
distinct. Authority changes invalidate cached content and late callbacks.

| 2026-11-03 | Define the retained Agent computer, responsive default and return focus, shared selection, authorized mounted-home files, and read-only recorded operation output with explicit missing/truncated evidence and unavailable browser capabilities. |

The Agent toolbar names its retained panel **Computer** (accessible name **Show
Agent computer** / **Hide Agent computer**). Compact overlays trap focus in the
visible panel, make background columns inert, and return focus to that control on
Back or Escape. Hidden retained tabs are excluded from keyboard focus. A wide
default panel does not take focus from the conversation.

Computer availability comes from the authenticated Agent environment observation.
Show checking, unsupported, not enabled, permission denied, and temporarily
unavailable states distinctly. Older servers that do not report the capability
show **Environment capabilities not reported**. The files surface continues to
label the authorized mounted home; an unavailable capability never invents a
browser image, live indicator, replay timeline, or interactive terminal. Readable
file drafts survive changes to write authority and remain read-only while saving
is unavailable. Shared operation output uses the same authorized observation as
the conversation.

| 2026-11-03 | Name the Computer entry, define compact keyboard containment and return focus, and render scoped public capability observations with distinct unavailable states. |

Operation output displays at most 50 operations per page, in their recorded order,
with Older/Newer controls and a visible range. It initially shows the newest page.
Hidden output does not render historical operations. Assistant text streaming does
not rebuild operation groups or replace the visible output DOM.

| 2026-11-03 | Bound operation output to 50 visible records, retain chronological paging, and isolate hidden history from unrelated conversation text rendering. |

Environment read failures use **Environment capabilities are unavailable. Try
again.** Keep protocol parser details out of the computer panel and retain its
explicit retry action.

| 2026-11-03 | Keep computer capability failures readable without exposing transport parser diagnostics. |

The Files scope label occupies its own row above the flexible explorer. The
explorer and its actions remain within the viewport in both the details column
and compact overlay; inner content scrolls without extending the panel.

| 2026-11-03 | Reserve the mounted-home scope row without overflowing the retained file explorer. |

Shared operation output requires a current successful read-authority observation.
Withdraw already displayed output when that authority expires or renewal fails,
including while the next check is pending. Show **Checking operation output
access…** during checking and **Operation output access could not be checked. Try
again.** after failure, with **Check output access again**. Explicit read denial
or a refreshed environment output prohibition replaces existing output with its
permission message. A still-valid authority lease may remain visible during its
background refresh. These changes do not discard a file draft whose read
authority remains valid.

| 2026-11-03 | Gate existing shared operation output on current read authority, distinguish renewal checking/failure, and provide scoped retry. |

The computer Files scope reads **Authorized Sandbox files (/home); private runtime
state excluded.** Workspace source and worktree files remain intentionally shared
within the authorized Sandbox file surface. The tree omits supervisor control
state and other Agents' upload namespaces; the selected Agent's uploaded files
retain preview, download and authorized edit behavior. An Agent route does not
make the Sandbox workspace an Agent-private filesystem. File-operation errors
retain the existing draft and retry behavior.

| 2026-11-03 | Distinguish authorized shared Sandbox files from protected runtime state and Agent-scoped uploads in the computer panel. |

### Optional repository task context (2026-11-03)

Home places an optional **GitHub repository** URL field and **Repository ref**
field below the task textarea, using the existing labeled Input controls and
16px form gap. The fields stack at every width. Clearing repository context
preserves the task text. An omitted ref means the repository's default branch;
an entered ref is a request, never a verified commit. Existing Workspace or
Sandbox creation names its inherited context and offers no repository override.
Services that do not advertise repository-task support show its unavailability.

Draft repository options share the task's viewer, tab, origin and context scope.
Submission freezes the exact task, URL and ref in the existing creation intent.
Show retained repository options separately from the editable next draft, with
**Requested ref** and **Checkout commit** labels. Only publicly recorded runtime
facts may fill the commit label; until then say **Not prepared yet**. Source
archive checkouts name both the upstream source commit and actual local checkout
commit, without implying imported Git history. Long URLs and hashes wrap.

Required connections use the existing connection dialog and explicit
**Continue original task** action. Dismissing the dialog or returning from OAuth
does not submit a task. Connection recovery retains the original body and key.
Repository access, missing ref, preparation and task execution failures keep
their distinct explanations. Failed preparation never implies task execution;
editing a failed intent uses **Start another task**, while uncertain requests
use **Recover original task**. Existing files are never replaced by an option.

| 2026-11-03 | Define optional repository/ref task context, immutable checkout facts, inherited-context restrictions and identity-preserving connection recovery for ticket 04. |

Agent Details shows repository provenance in its existing band and fact-list
styles: repository URL, requested ref, source commit and checkout commit. Use
the existing wrapping monospace fact value for hashes. Empty workspaces omit
this band. Preparation failures remain in the existing provisioning surface.

| 2026-11-03 | Place repository provenance in the existing Agent Details fact band. |

### Answer deliverables (2026-11-03)

Only a completed turn's final recorded Agent answer may show its explicitly
referenced deliverables. The public answer query verifies structured references
against the resource's own authority. Ordinary file names, code spans, directory
contents and another answer's resources never create cards. The machine reference
marker is hidden from rendered prose; quoted examples remain ordinary text.
Streaming answers also hide an unfinished standalone protocol marker once its
reserved prefix is present. Streaming and intermediate messages never show cards.

Place a semantic list named **Answer deliverables** immediately after the answer
body and before its existing duration, usage and footer. Use a vertical stack with
8px gaps and 12px top margin. Each row has 12px padding, a 1px border-l2 edge,
radius 8, the layer-1 ground, a wrapping 14/20 medium name and 12/18 caption
metadata. A 16px existing file glyph is decorative. Metadata names **Published
artifact**, **Live file** or **Document revision N**, followed by the reported
media type and actual byte count; an absent count says **Size unknown**. Document
identity never becomes a file path, and a live file is never called a snapshot.

Use the existing compact outline Button for **Download <name>** and compact ghost
Button for **Retry <name>**. On screens below 30rem, controls have a 44px minimum
height and wrap below the name rather than widening the reading axis. A pending
download disables only that row. Errors remain in the row in caption ink with a
polite status; they never replace the answer, draft or another card. Distinguish
**File changed since this answer**, **File deleted**, **Document has a newer
revision; download keeps this revision**, **Not ready**, **Access unavailable**,
**Exceeds the resource limit**, and **Download failed; retry**. Changed live bytes
cannot be downloaded as the old result. Document revision changes retain the
original revision download if current document permission still allows it.

An answer with no structured references renders no loading box or empty space.
Known references reserve compact loading rows while their public read is pending.
Only visible answers with explicit reference markers request delivery metadata.
Immutable resources use authorized metadata reads. A live-file row initially says
**Verify on download** beside its answer-time size; listing cards never downloads
all referenced files. Download verifies that row's actual bytes, and Retry checks
only the selected resource. Narrow conversation shares remain distinct from the
existing explicitly accepted full-home file grants.
Late metadata keeps stable resource keys and the existing conversation scroll
anchor; there is no focus transfer or scroll-to-result. Reads and downloads abort
on Agent, viewer, client or authorization changes, and late results are discarded.
Downloads use opaque bytes, verify bounded size and SHA-256 before saving, and
recheck current access after buffering. They never render executable content or
publish a private file. Ticket 08 owns preview; the card has no preview action here.

| 2026-11-03 | Add answer-scoped verified deliverable rows, independent download/retry states, explicit live-file versus immutable-version semantics, and mobile controls while preserving reading position and the existing answer footer. |

### Edit suggested follow-ups (2026-11-03)

Each suggested reply pairs its existing full-text send button with an **Edit before
sending** button, named with its one-based suggestion position for assistive
technology. Reuse the suggestion region's 8px gaps, 44px minimum targets, layer-1
ground, border-l1, radius 8 and standard focus ring. Actions wrap within the
available width on desktop and narrow screens.

Editing fills the real composer with the complete text and focuses it without
sending. Existing text opens the standard Modal, **Keep your draft?**, explaining
that attachments remain. Its explicit actions are **Cancel**, **Replace text** and
**Append to draft**. Escape and dismiss act as Cancel. Closing returns focus to the
composer; appending separates the existing text and full suggestion with a newline.
Replacing offers **Undo replacement** in a polite composer status row using the
existing notice typography and quiet Button. Undo restores the replaced text;
all attachments remain. Later manual edits or submission retire this undo action.

Editing never acknowledges a send or rewrites a retained request. Failed sends
keep the edited draft; uncertain sends retain the original request for recovery.
Confirmed receipts cannot clear later input. An expired source removes only the
suggestion association; the text remains an ordinary draft and its next send
requires normal current authority. Account and Agent changes cannot transfer an
edit, dialog or undo into another conversation. Settings stay compact and the
existing header progress/completion slot is unchanged.

| 2026-11-03 | Specify suggestion edit actions, explicit draft conflict choices, attachment retention, replacement undo, composer focus and ordinary send recovery for ticket 12. |

### Negative feedback reasons (2026-11-03)

Choosing **Needs improvement** immediately saves the vote and opens an inline,
non-modal **Reasons for improvement** panel below that answer's existing footer.
Keep the answer and composer usable. Reasons save independently, only after a
confirmed down vote and a current feedback read. The selected vote key still
removes the vote; **Edit reasons** reopens the panel without changing the vote.

Offer six native multi-select checkboxes: **Inaccurate content**, **Did not answer
the question**, **Steps do not work**, **Incomplete answer**, **Unclear wording**,
and **Other**. **Additional details (optional)** uses a full-width textarea with
a visible Unicode code-point counter, **0 / 500**. Do not truncate text or use
UTF-16 maxlength. Over 500 shows an associated error and disables save.

Use **Save reasons**, **Close**, **Retry reasons**, and **Reload feedback** actions.
Announce **Saving reasons…**, **Reasons saved**, and actionable failures politely.
Close only collapses; preserve drafts by viewer, Agent and canonical answer in
page memory, including navigation back. Failed/uncertain saves retain the exact
request for retry, independently of vote recovery. A version conflict keeps the
draft and requires a fresh read before a new save. Remove/up atomically clears
saved reasons; switching back down starts empty. Account or authority changes
clear prior drafts and hide their controls immediately. Never imply training or
send input as a feedback side effect.

Use the existing surface/background, border, business-primary and secondary-text
tokens, 1px border, 8px radius, 12px padding and gap, and 12/18 caption text. Stack
checkbox labels and textarea in normal flow; controls wrap with an 8px gap.
Textarea minimum height is 80px, vertically resizable, border-box, with inherited
font. Below 30rem, all buttons and checkbox labels have at least 44px hit targets.
Focus uses existing controls' outlines; no animation or overlay is added.

| 2026-11-03 | Add independent, version-bound negative reasons with six choices, a 500-code-point optional note, retained scoped drafts, exact recovery and an accessible inline mobile panel for ticket 10. |

## Contextual suggestions after a confirmed answer

Contextual follow-ups use the existing suggested-reply region and controls. A
confirmed answer and the composer remain usable while generation is pending.
Read saved suggestions for the current viewer, Agent and canonical final answer;
show **Suggested follow-ups** only for a valid saved generated set. Pending,
empty, failed, expired and unauthorized results leave the region absent. Do not
show a loading skeleton, retry-generation button or model error in the answer.

Refresh reads the same saved identity, order and exact text. Bound background
reads to the active answer and stop after a terminal result, a short deadline or
any viewer, permission, Agent or answer change. A late response cannot reopen an
old group. Normal conditional send, retained draft, uncertain-request recovery,
keyboard focus and narrow-screen wrapping follow ticket 11. The same set and item
identity remains available to the separate suggestion-edit extension.

| 2026-11-03 | Define saved contextual follow-ups, unobtrusive asynchronous delivery, bounded reads and stale-result suppression for ticket 13. |

### Answer deliverable preview (2026-11-03)

Answer delivery names are keyboard and touch buttons opening **Deliverable preview**
in the existing conversation right column (300–520px), or its existing narrow
overlay. The selected button uses the accent label and border, `aria-pressed`,
the standard focus ring and a 44px minimum target. Selection includes Agent,
answer and delivery identity; it never resolves an arbitrary prose path.

The panel uses the existing file header (48px minimum), 16px padding, 8px gaps,
layer-1 ground, border-l1 and normal body typography. It shows the resource name,
revision/status, **Download**, **Retry preview**, and **Close preview**. Opening
focuses Close preview; Escape and close return focus to the exact originating
card without moving its conversation scroll. Keep history, composer, attachments
and the visited file editor mounted while switching panels, including pending saves.

Preview is read-only. Reuse the existing safe text and raster file renderer and
the existing Markdown renderer in inert-link/media mode. Use the existing 1 MiB
file preview bound before reading bytes; larger resources retain their separately
bounded authorized download. Raw HTML, SVG and executable documents are shown as
plain text when safe to decode; untrusted links and embedded media are inert.
PDF and other unsupported formats explain **Preview is not supported; download
to view this file**; no same-origin document frame or active page is introduced.

Loading, failed reads, preview size limits, deleted files, changed live files,
newer document revisions and revoked permission have distinct status text.
An older document revision stays explicit and exact. Switching resources cancels
the prior read. Every read/download uses current independent resource authority;
recheck visible previews on focus and periodically, clearing content while access
is uncertain or revoked. Conversation sharing does not grant file/document access.

| 2026-11-03 | Add answer-bound safe read-only previews, bounded rendering, resource access renewal, accessible return focus and persistent conversation/file editing for ticket 08. |

### Operation record replay (2026-11-03)

Operation output gains **Operation record replay** only when the public replay
contract is available. Reuse the computer panel's spacing, typography and plain
preformatted output. A labelled navigation group offers **Previous operation**,
**Next operation**, and **Return Live**, using existing small buttons and visible
focus. Left/Right arrows navigate when this group has focus; End returns Live.
Show the selected durable cursor and turn, recorded command, output, status,
timestamp, and the plan recorded at that point. Missing fields say **Not
recorded**; truncated output, gaps and expired history have explicit text.
There are no historical browser images or file contents without snapshots.

Live events continue receiving while a historical cursor stays selected. Live
shows the latest durable operation, and Return Live reads the latest public
cursor. Reload keeps only the scoped cursor and obtains fresh authorized data.
History is read-only: command entry, approvals, feedback, suggestions and control
are unavailable in the replay view. Completed turns and plan cards can open
**View plan and replay** for their actual turn, including turns without plans.
Permission changes clear the selected content before another authorized read.

| 2026-11-03 | Define durable operation record replay, keyboard navigation, exact Live return, recorded-only snapshots, scoped reload and read-only history for ticket 18. |

### Agent browser observation (2026-11-03)

An explicitly enabled Agent browser selects Browser on the first Computer opening,
unless the user has selected another tab. The panel remains observation only. Use
the existing tabs, caption typography, secondary text and semantic state tokens.
Show the bounded real image at its 16:9 aspect ratio with contained scaling; never
draw a replacement browser, an illustrative page, or a command field.

The image caption contains the sanitized location and actual capture time. **Live**
requires a decoded frame captured within three seconds; duplicates never renew its
age and lower sequences never replace it. A failed read or expired frame changes
the caption to **Last frame · Reconnecting…**. Loading, not enabled, unsupported,
paused, forbidden and disconnected remain distinct. Owner-only **Enable browser**
is an explicit command, with visible pending and failure states. Closing or hiding
observation releases it without navigating, reopening a task or resetting its page.
Agent, account, runtime or permission changes remove the old image immediately.

| 2026-11-03 | Define bounded real browser observation, fresh decoded Live state, explicit enablement and scoped withdrawal for ticket 15. |

Reopening Computer for the same Agent retains the selected tab. Explicitly
choosing Files or Operation output takes precedence over the Browser default.

| 2026-11-03 | Retain the selected Computer tab across closing and reopening observation. |

### 2026-11-03 — Browser default and explicit operation replay

An explicitly selected operation record keeps the Computer on Operation output, including after closing and reopening. The enabled Browser default applies only when no prior selection exists and the generic Computer opens Files. Browser observation and operation history remain independent sources; replay never substitutes a historical browser image.

### Optional desktop during task creation (2026-11-03)

Home places **Enable desktop** beneath the task text, using a native checkbox in
a 44px minimum-height labelled row. It starts unchecked. Adjacent secondary text
explains that preparation takes extra time and memory and that the browser is
bound to this Agent. Capability checking and unavailable reasons remain visible;
an unavailable option is disabled without preventing ordinary tasks. Selecting it
does not create resources. The choice and exact task survive refresh together.
After submission the checkbox is frozen with the submitted request. A new choice
requires an explicit new task intent; recovery never changes the accepted choice.
Desktop preparation is a separate reported milestone. Entering the conversation
may precede its completion. Only a fresh decoded browser frame displays Live;
preparation or observation never repeats the first task or restarts a Sandbox.

| 2026-11-03 | Define default-off Home desktop selection, capability refusal, durable frozen recovery and truthful preparation for ticket 16. |

### Exclusive browser control (2026-11-03)

When the public environment reports verified writable control and the current
viewer owns the Agent, show **Take control** beside the existing browser status.
Keep the original real image and its aspect ratio. A receipt is required before
**You are controlling** appears; pending handoffs say **Transferring control…**.
Agent control, another client's control, unknown state and expired authority
remain distinct. Unknown state exposes **Check original operation** and, after
an unresolved query, **Retry exact operation**. Never infer success from time.

The same image becomes a focusable browser-input surface only under a current
lease and fresh observation. Clicks use its intrinsic viewport coordinates;
typing and supported editing keys go through the bounded public browser-input
contract. Browser chrome shortcuts and terminal input are not offered. Keep
**Return to Agent** outside that input surface, reachable by normal keyboard
navigation. Returning is a control operation and never a conversation message.
Provide a labelled **Text to type in browser** field and **Type text** action for
composition, paste and mobile use; retain its text after an uncertain response.

Use existing small buttons, secondary caption text, 8px control gaps, 12px/18px
audit text and the existing focus ring. The control group wraps within the
panel at mobile widths. A compact **Control log** details element shows bounded
metadata, timestamps and a visible truncation notice, never typed contents or
lease tokens. Losing visibility, account, permission, current context or a
fresh live image immediately disables input. Stop lease renewal while hidden
or offline; show the runtime-confirmed expiry/return on reconnect. Closing the
panel attempts exact return without resetting its browser or conversation draft.

| 2026-11-03 | Define receipt-confirmed exclusive browser input, exact recovery, same-context return, bounded control audit and accessible desktop/mobile input for ticket 17. |

### Task experience availability and retained work (ticket 19 policy component)

Task Home, suggested follow-up admission, browser enablement, takeover and replay
presentation follow the current authenticated public eligibility response.
Refresh it on identity changes, focus and reconnect. While unavailable or unknown,
disable new entry and offer the ordinary **New agent** path and existing manual
conversation. Do not expose operator cohorts or configuration terminology.
Keep an already submitted task and its **Recover original task** action mounted;
never reset its exact intent, draft, transport or accepted Agent on availability changes.
Suggestions already edited into the composer keep their text and answer context;
show **Suggested follow-ups are unavailable. Your draft is kept.** when new scoped
submission is unavailable. Original uncertain requests retain explicit recovery.

Browser eligibility is separate from live observation and ownership. Closing
new takeover/input/renewal keeps the mounted control status, **Return to Agent**,
and original receipt recovery visible. It does not claim release or clear the
browser context. Keep disabled controls and status readable using existing
button, caption, wrapping and focus patterns; no new visual tokens. Replay is
presentation only: closing it returns to ordinary operation output without
clearing retained replay positions or changing history access.

| 2026-11-03 | Define default-off task entry, manual fallback, preserved drafts/recovery, and mounted browser return when task experience availability closes. |

### Runtime readiness during task preparation (ticket 19 policy component)

Keep stored Agent settings and the conversation mounted during preparation.
Read the runtime model catalog only once the current Agent is ready, working,
or waiting for approval; ordinary task activity is not runtime unavailability.
Files wait for current environment availability before their first read. An
explicit older-service environment 404 or a connection without that capability
retains the existing file path. Keep loaded file selection and drafts across
authorized computer-tab changes; current authorization and identity still fence
every read. Use existing preparation/unavailable status text and retry actions,
without inventing readiness or suppressing actual service errors.

| 2026-11-03 | Defer runtime-dependent file/model reads during preparation while retaining conversation, settings and authorized tab state. |

Model discovery is also available before Agent readiness when the current Sandbox
is active or its recorded credential-delivery step completed. Previous readiness,
including working, approval, paused or stopped Agents, keeps corrective model
selection reachable. Do not make a failed initial model choice prevent recovery.

| 2026-11-03 | Ground model discovery in delivered Sandbox credentials or prior initialization, not only an idle Agent status. |

During a temporary file-authority refresh failure or expiry, keep an already
loaded unsaved draft in its exact account/Agent/connection component, hide the
file contents and actions, and show **Checking file access. Your unsaved changes
are kept.** Resume only after current authorization succeeds. Initial unknown
availability never starts a file read; revoked authority or an identity change
withdraws that component. A retained draft is not permission to perform file IO.

| 2026-11-03 | Preserve quarantined file drafts through temporary readiness/authority outages without retaining stale read or write authority. |

### Composed task preview and touch controls (2026-11-03)

When an answer preview cannot fit in the right column, place it in a separate
30dvh row above the conversation, with the remaining height assigned to the
conversation and composer. This supersedes the narrow modal preview overlay.
The preview remains a named complementary region; it does not make the center
inert or trap focus. Keep the existing 48px header, 16px padding, scrollable body,
layer-1 background and border-l1 separator. Opening focuses Close preview;
Escape returns focus to the selected answer card. Drafts, uploaded attachments
and conversation scroll position remain intact.

In this compact preview layout, cap the draft textarea at the smaller of160px
and20dvh so a long draft retains usable preview and conversation space. Outside
the layout its existing height limits remain. Attachment item actions have at
least44px width and height on coarse pointers or below30rem, including Remove
and Retry. Preserve their labels, keyboard focus rings and reduced-motion rules.

| 2026-11-03 | Keep the composer reachable beside a compact answer preview and expand attachment touch actions, based on ticket19 actual390px coarse-pointer geometry. |

### Browser observation release ordering (2026-11-03)

Closing or hiding the browser withdraws its frame immediately. Serialize its
observation and observer release requests, including a rapid reopen, so cleanup
waits for an already running bounded read before using another IO permit. Stale
responses never restore a closed or different Agent's frame. Keep the existing
four-second read deadline and public capacity limits.

| 2026-11-03 | Serialize browser observation cleanup with its pending read after the composed close/reload path exposed a third concurrent IO request. |

Environment revalidation and browser control requests for one Agent wait for
each other in the Web public-client binding. Browser observation retains its
separate bounded read. Queued canceled requests do not dispatch; an in-flight
request settles under the SDK deadline before the next one starts, and canceled
read results stay withdrawn. Control bodies, revisions and operation identifiers
remain unchanged; every dispatch uses current public authorization.

| 2026-11-03 | Order environment revalidation and control IO so policy focus refresh cannot crowd out explicit Return to Agent under the unchanged two-permit budget. |

Computer request cancellation withdraws the caller immediately, including while
waiting for an earlier request. A canceled queued mutation never dispatches.
Already dispatched requests retain their serialization slot until settlement,
with a four-second SDK deadline; the caller's total wait is also bounded to four
seconds. A timed-out control mutation remains uncertain and keeps its original
operation identity for an authorized status read. The next dispatched request
resolves current SDK authorization.

| 2026-11-03 | Bound queued computer requests and preserve immediate cancellation and exact uncertain-operation recovery under stalled HTTP responses. |

### Reviewed replay navigation and explicit browser return (2026-11-03)

Next operation considers recorded updates to operations that began before the
selected cursor. Preserve their recorded identity, title and output across that
boundary, keep cursor order and the existing 50-record window, and never expose
future output when restoring an exact historical cursor.

A stale browser frame disables acquisition, input and renewal. It does not
disable explicit **Return to Agent** under a still-current authorized lease.
Return and original receipt recovery remain available when new admission closes;
account, Agent, incarnation, browser context, client and permission fences still
apply. Do not infer release before a receipt or rebuild the browser or task.

| 2026-11-03 | Clarify overlapping-operation navigation and explicit authorized return while browser observation is stale, following final SPEC review. |

### Reviewed preview bounds and incomplete task outcomes (2026-11-03)

Authorize preview metadata before applying the 1 MiB limit. Oversized or
unsupported previews do not request byte-reading verification. Permitted
downloads still verify the exact answer resource, revision, size and digest.

A recorded output gap inside a turn prevents successful completion for that
turn. Retain its messages and reported plan steps, show the existing failed
response outcome, and omit the completion duration. Task navigation uses the
same incomplete outcome. A gap in an earlier turn does not fail a later complete
turn; incomplete answers cannot authorize deliverable reads or downloads.

| 2026-11-03 | Clarify metadata-first preview bounds and consistent current-turn gap outcomes across completion, plans, task navigation and answer deliverables after Standards review. |

Live-file preview downloads bound runtime reads to the saved reference size, so
a file that grows after metadata authorization cannot trigger a larger read or
hash. An empty reference uses the existing minimum one-byte read allowance and
still requires exactly zero returned bytes. Growth or changed content reports
the existing changed-resource outcome; initial path capture keeps its normal cap.

| 2026-11-03 | Close the live-file growth race after preview metadata authorization without extending the preview byte budget. |

The optional **Machine Included** choice on Agent creation uses the existing
selection-card spacing and surface tokens. Desktop-capable owned Agents expose
**Machine desktop** beside the conversation header. Owned Machine conversations
open with chat on the left and the live desktop on the right. The workspace
uses a 2:3 grid split with a 360px chat minimum. At an available workspace width
of 56rem or less, the panes stack, retaining chat above a desktop of
`min(42dvh, 420px)`. Both stay visible and mounted during a turn. Hide/close
removes only the viewer; opening it again preserves the exact chat composer,
draft, transcript and stream. No desktop viewer opens for headless or read-only
Agents. The split follows the available center width, including when Files or
Details takes space.

The desktop is a flat layer-1 pane with a layer-2 divider; it has no viewport
overlay, shadow or corner radius. Its header uses wrapping actions, 12px by
16px padding (8px by 12px in the stacked layout), a standard 14px strong title,
and a visible 12px/18px connection and control status. Gaps are 16px in the
header and 8px between actions, or 8px throughout the compact header. The
bundled canvas fills the remaining space and scales the existing shared desktop
without resizing its resolution during Agent work. Full screen expands the
whole pane, retaining connection status, input controls and an exit action.
**Control desktop** opts into keyboard and pointer input; while the Agent is
working or a message starts, the panel stays view-only and tells the user to
watch its actions. **Reconnect** resets input control.

| 2026-10-29 | Add the Machine Included creation choice and a bundled desktop viewer with explicit input control, connection status, reconnect and full-screen actions. |

| 2026-10-29 | Describe Machine Included as a shared Linux desktop with Chromium; avoid claiming a bundled native computer-use tool or exposing runtime-image implementation details in creation copy. |

| 2026-10-29 | Scope the Machine Included hint alignment to its component class while retaining the shared field hint typography. |

| 2026-10-29 | Apply Machine Included hint alignment through its containing section while using the existing shared field hint class. |

| 2026-10-29 | Show owned Machine desktops beside their conversation by default, stack within narrow workspaces, preserve chat during viewer toggles, and retain controls in full screen without changing the shared desktop resolution. |

Machine desktop loading reserves the screen pane with a quiet desktop-shaped
skeleton: a window inside a frame, using the shared shimmer and running state
dot. The placeholder uses at most 70% of the available width, capped at 360px,
with a 16:10 outline, 12px frame radius, 8px inner window radius, and existing
12/16/24px spacing. Its 14px/20px label names connecting or loading the first
screen. The connection handshake alone does not mean the display is ready;
keep input disabled until actual remote pixels are painted. A 30-second failure
shows a persistent recovery message and leaves Reconnect available. Once the
first remote pixels are painted, reveal the screen and remove the placeholder
immediately without moving chat. The loading surface must not obscure a ready
desktop with an exit animation.

A 22px outlined control label beside Machine desktop states Watching, You
control, or Agent working, using the existing 6px badge radius, 12px/18px text
and state dot. The header divider fades to the business accent during active
user or Agent control. The persistent status announces changes politely.
Handoffs also show a visual notice for five seconds at the bottom of the desktop
pane, inset 16px, with 12px by 16px padding, 12px radius and layer-1 background.
Its 14px/20px words repeat the persistent state without an extra live-region
announcement. Use shared Presence for the 200ms appearance/removal; notices
never intercept input or focus, including in full screen. A newer transition
replaces the previous notice and starts its own five-second lifetime. A turn
starting revokes user input immediately. Completion returns to Watching until
the user explicitly chooses Control desktop again. Reconnect also resets input.
Reduced motion removes fades and shimmer while preserving all status text.

| 2026-10-30 | Animate actual desktop loading and five-second control handoff feedback, keep one persistent accessible status, preserve first-frame readiness, and require explicit input takeover after Agent work or reconnect. |

A connected Machine viewer must retain a visibly sized, painted desktop. Browser
canvas context loss or a persistently transparent/missing display revokes user
input and shows **Restoring the desktop display…** using the existing loading
skeleton. Reconnect only the viewer, at most twice before showing a persistent
recovery message with Reconnect available. Preserve the Machine, Agent turn,
conversation and unsent draft; restored viewers return to Watching. Hidden panes
and browser tabs do not trigger recovery. Opaque black and unchanged desktops
remain valid; a temporary resize may finish repainting before recovery starts.

| 2026-10-30 | Recover lost Machine viewer canvases with bounded retries and the existing loading surface; preserve conversations and require explicit input takeover after restoration. |

Before the owned Sandbox is active, keep the existing desktop
placeholder with **Waiting for your machine to be ready…** and disable input and
Reconnect. Start the stream and its connection timeout only after readiness is
observed. The desktop can connect while Codex is still starting; this does not
mark the Agent ready. Pausing or revival releases the old viewer; readiness opens a fresh
viewer in Watching mode without remounting the conversation. Normal Agent turns
retain the connected display.

| 2026-11-01 | Wait for Machine runtime readiness before connecting the desktop, and reconnect after revival without restoring manual input. |

While an Agent is Starting, observe its public detail immediately after the
initial conversation has loaded, without reloading history on every check.
During the first ten seconds, check at a 100-millisecond cadence including
authorization and read time, without overlapping requests. This keeps the
short runtime Ready window visible before first-message acknowledgement.
Longer preparation returns to the 1.5-second delay after each completed read.
When its status or parent state changes, refresh the complete conversation.
While the first message remains pending, show the fresh Agent state immediately
and retain the current conversation until its history refresh completes. Settled
preparation publishes the full read together. First-message execution retains
normal history refresh.
Failed reads retain the five-second retry delay. Reads never overlap, stop on
departure or settled preparation, and preserve live conversation fragments.
Faster observation never implies readiness by elapsed time.

| 2026-11-01 | Refresh initial Agent preparation promptly, with bounded frequency and the existing slow-preparation and error backoff. |

Selecting Machine Included may preload the bundled desktop viewer code while
the creation dialog is open. This downloads only application assets; desktop
connections still require the owned Agent and active Sandbox. A failed preload
does not block creation, and the viewer retains its normal connection recovery.
The first painted screen appears immediately; control handoff notices retain
their existing Presence fades and five-second lifetime.

| 2026-11-01 | Preload the desktop viewer when Machine Included is selected and reveal the first painted frame immediately, preserving input authorization and control handoff animations. |

| 2026-11-01 | Include completed authorization and read time in the short startup observation cadence, preserving sequential requests, slow-preparation backoff and access checks. |

| 2026-11-01 | Use a 100-millisecond startup observation cadence so a short runtime Ready window is visible before first-message acknowledgement, while retaining sequential reads and the 1.5-second prolonged-preparation delay. |

Preparation refreshes retain the Agent tab's selected conversation transport.
They still check the current account before reading and before exposing history;
refreshing readiness never selects a different transport or resubmits setup work.
When a startup observation triggers a conversation refresh, its fresh entry
account check may also publish that observed preparation state synchronously.
There is no intervening asynchronous work or cached authorization; the completed
history still requires its own account check before publication.

| 2026-11-01 | Refresh preparation through the Agent's existing conversation transport, retaining fresh account checks around history reads. |

After a complete preparation snapshot refresh, restart live replay from that
snapshot's cursor while retaining the same selected transport and input journal.

| 2026-11-01 | Preserve the live replay restart at the new preparation snapshot cursor when reusing the selected transport. |

After an explicitly admitted Machine Included creation names its Agent, the
browser may begin that Agent's owner-authorized desktop connection alongside
navigation reads. The connection transfers once to the matching personal Agent
view, client and owner after its normal access and active-runtime checks. Until
then it sends no desktop input and bounds buffered server greetings by size,
count and a ten-second lifetime. Abandonment, account mismatch, hidden tabs and
connection errors discard it; the viewer retains its normal connection fallback,
first-frame readiness, explicit input control and display recovery.

| 2026-11-01 | Overlap desktop connection setup with admitted Machine creation navigation, preserving owner checks, bounded single-use handoff and explicit desktop input control. |

## 28. The character in the product (2026-11-01)

The CMA character of §23 — its procedural geometry, MeshPhysical body and lenses, studio
lights, lens glint and contact shadow — is the Agent's presence wherever the product lists
or runs Agents. It is never redrawn, simplified or given props to explain a state: every
pose moves only the rig the character already has (head look, float and sway, lift, scale,
tilt, turn, hem breathing, glint and exposure), so the character in a 24px row and on the
520px landing stage are the same object.

**One registry.** `web/src/character/poses.ts` names every pose and is the only place
motion is defined. `characterForStatus` (`status.ts`) maps each recorded status, after any
parent lifecycle (§10), to a pose, a body tone and whether it moves in a list:

| Status words | Pose | Moves in a list | Body |
|---|---|---|---|
| Starting, Updating access, Updating runtime | preparing: one turn per second | yes | brand lime |
| Working | thinking: looks up and to the right, glint every 1.6s | yes | brand lime |
| Needs approval | approval: leans in, larger, two hops every 2.8s | yes | pending amber |
| Pausing | paused, breathing as it settles | yes | brand lime |
| Resuming | resuming: stretches awake, then floats | yes | brand lime |
| Ready, Live | idle: still, facing forward | no | brand lime |
| Paused, Stopped, Deleting | paused: low, head down, dimmed | no | brand lime |
| Needs attention | failed: drooped and tilted | no | failed red |

The body colour only repeats a word that is already present. A pose that plays once (failed)
holds its last frame, and a pose change eases over 300ms instead of jumping. Neighbouring
characters take a stable clock offset from their Agent's identifier, so a list never moves
in lockstep.

**One stage.** Browsers keep roughly sixteen live WebGL contexts per page, so characters
are not given renderers of their own. One shared renderer draws each character into the
corner of its buffer and copies it into that character's own canvas. Frames are scheduled
only for characters that are visible, moving, and on a shown document. Three.js loads on
demand, after the shell is interactive. Each square is reserved before it is drawn, so rows
never reflow when their characters arrive.

**Never the only signal.** Characters are `aria-hidden`. Every placement keeps the status
word in visible text, in an accessible name or in a hover title. Under
`prefers-reduced-motion: reduce` every character draws its pose's still moment and
nothing loops. When WebGL is unavailable or its context is lost, each character becomes
the still §21 mark in its body colour, in the same square.

**Sidebar.** Both navigation views lead every Agent row with its character: 24px in the
Agents view (§10) and 20px in the Sandboxes tree, where it replaces the generic Agent
glyph. Only the rows that show work or ask for a person move.

| 2026-11-01 | Lead every sidebar Agent row with its status-posed 3D character from one pose registry and one shared WebGL stage; only work and approval move, reduced motion and missing WebGL draw still poses or the mark, and the status word stays in the row's accessible name and hover title. |

**Conversation.** The character returns to the conversation where it reports something live,
superseding the 2026-10-21 dot for Thinking:

- The active Thinking control (§13) leads with a 32px character in the avatar column. It
  works while a tool call is pending, replies while reply text streams, and thinks otherwise.
  It leaves with the turn.
- An approval still waiting on a person leads with a 40px approval character in the pending
  amber. A decided approval is history and carries none.
- A conversation with no messages yet shows the Agent's character at 120px with its contact
  shadow, above the one line that says what to do (secondary ink). It is posed by the Agent's
  status: it looks around while Ready, and stays still while Paused or failed.

Message avatars keep the inline §21 mark: transcripts are history, and history does not move.

| 2026-11-01 | Lead the live Thinking row, waiting approvals and empty conversations with the status-posed character; Thinking follows pending tools, streaming replies and reasoning, decided approvals and message history stay still. |

## 29. Interface polish (2026-11-01)

A pass over the whole signed-in product against two checklists: the detail principles
(concentric radii, tabular figures, specific transitions, press feedback, hit areas, image
outlines) and the app-screen principles (colour on data rather than chrome, honest states,
receding navigation). Each change below amends the section it names; this section records
why it was made.

**Readable tertiary ink in dark.** The dark tertiary ink was `zinc 500`, 4.1:1 on the base
and 3.7:1 on layer 2, yet it carried required small text — sidebar ages, field hints, record
metadata and picker reasons. It is now `zinc 450` (`#8A8A93`): 5.8:1 on the base, 5.2:1 on
layer 2 and 4.8:1 on layer 3, still clearly quieter than the caption ink. Fixing the one token
fixes every surface that uses it. Light keeps `zinc 500` (4.8:1).

| 2026-11-01 | Raise the dark tertiary ink to zinc 450 so small tertiary text meets 4.5:1 on every dark layer while staying quieter than caption. |

**One reading size in a reply.** A reply's paragraphs were 16/28 while its list items and tables
inherited the stream's 14px interface size, so any answer with bullets changed size halfway
through. `.agent-content` now sets `--dsw-font-markdown-base` for everything it holds, and
tables use `--dsw-font-markdown-table`. A reply's top heading is `markdown-h3` (20/30), below
the 20px page title rather than above it, and every deeper level shares `markdown-h4` (16/28,
600). Tables are row rules and alignment only: no cell grid, 8px by 12px cells, caption-ink
heads and tabular figures. While a reply streams its paragraphs wrap plainly: pretty wrapping
re-broke the last lines on every token. It returns once the reply settles.

**Links are underlined at rest.** In dark the accent link measured 1.15:1 against the reply
text, so a link told apart only by colour was invisible to many readers. Links are underlined
at 1px, 3px below the baseline, in the link colour at 45%, and darken to the full colour on
hover. This restores §18.12's underline.

| 2026-11-01 | Set one reading size across a reply, cap its headings below the page title, draw tables as row rules with tabular figures, wrap streaming paragraphs plainly, and underline reply links at rest. |

**Press feedback.** Only the primary and ghost buttons answered a press, by a fill change.
Every enabled button now settles to 96% over the fast duration while pressed and returns on
release. That is the one movement a control makes, and it confirms a click landed without
claiming anything about the result. Hover still never moves. Reduced motion drops the scale.

| 2026-11-01 | Settle every enabled button to 96% while pressed, with the scale in the shared transition rule and no movement under reduced motion. |

**The current row is not a hovered row.** §10 gives the current Agent the active-item ground,
but the tree drew it with the 4% hover tint, so the open conversation looked exactly like a
row the pointer happened to rest on. The current tree row now takes
`--dsw-specific-sidebar-nav-item-active`, the ground the action rows already use for the
current page.

| 2026-11-01 | Draw the current sidebar tree row on the active-item ground rather than the hover tint. |

**Steady figures.** Elapsed preparation times, Trigger statistics, tool-call and queued counts,
and sidebar ages changed width as they ticked and nudged the words beside them. Every `<time>`
now takes tabular numerals from the base sheet, and so does each counter. This replaces the
`.tabular` helper, which nothing used.

| 2026-11-01 | Use tabular numerals for every time element and every changing count, and retire the unused tabular helper. |

**Readable placeholders.** Field placeholders used the dimmed ink, about 2.6:1, though a
placeholder is often the only hint of what a field takes. They now use the caption ink, as §23
already did for the composer.

| 2026-11-01 | Draw field placeholders in the caption ink instead of the dimmed ink. |

**Font rendering.** Firefox on macOS now gets the same grayscale smoothing Chromium and Safari
already had. `text-rendering: optimizeLegibility` is gone: Geist already sets its own kerning,
and the property makes long transcripts noticeably slower to lay out.

| 2026-11-01 | Add grayscale smoothing for Firefox on macOS and drop optimizeLegibility from the body. |

**One label voice, in the interface face.** Section labels, field labels, card eyebrows and
pills were 10–11px uppercase mono with 0.14em tracking — four slightly different versions, two
below the 11px floor. Tiny tracked capitals in a code face are the most recognisable mark of a
generated template, and they read as a category tag rather than a label. Every such label is
now `xxs-strong-12`: 12/18 at weight 500, sentence case, no tracking, in the interface face.
Field labels and section eyebrows take the caption ink; tone-bearing eyebrows, such as the
approval card's warning, keep their tone. Mono stays for identifiers, where its meaning — a
literal value — is true.

| 2026-11-01 | Replace the uppercase mono label voice on eyebrows, field labels and pills with sentence-case 12/18 medium in the interface face; keep mono for identifiers. |

**Lime means something.** Every menu row, every outline button and every picker caret turned lime
on hover, so the accent flickered across chrome whenever the pointer moved and stopped meaning
anything. §2 already says neutral interaction stays neutral. Menu rows, outline buttons and picker
carets now hover with the neutral ground, primary ink and the level-4 border. Lime is kept for the
one primary action, selection and the current item, live state, focus and data.

| 2026-11-01 | Make menu-row, outline-button and picker-caret hovers neutral, keeping lime for primary actions, selection, live state, focus and data. |

**Every key answers 32px.** §15 sets a 32px minimum target, but several keys were smaller:
Send and Stop (28px), message footer keys (28px), the modal close (28px, shrunk under the shared
small button), the sidebar row menu (24px), the effort reset (24px), the sidebar view tabs, the
Filter and Clear keys, and the tool-group toggle (28px). Keys whose drawn size is part of the
layout keep it and extend their hit area with a transparent pseudo-element to a 32px square.
Footer keys sit 4px apart, so their squares meet without overlapping. The others now draw at
32px. The modal close is the shared 32px small icon button again.

| 2026-11-01 | Give every desktop key at least a 32px target: Send, footer, row-menu and reset keys extend their hit area, while the modal close, sidebar filter keys and tool-group toggle draw at 32px. |

**Concentric corners and outlined pictures.** The menu's segmented choice track was 8px with
3px padding around 6px choices, so the inner corners looked rounder than the outer ones. It is
now 10px with 4px padding. Images and videos in replies, attachment previews and avatar photos
carry a 1px inset outline in pure black or white at 10% (`--dsw-alias-image-outline`). A tinted
neutral would pick up the ground beneath it and read as dirt on the edge.

| 2026-11-01 | Make the menu's segmented track concentric and outline reply media, attachment previews and avatar photos with a 10% black or white hairline. |

**Approval answers as one choice.** Deny, Abort and Allow were left-aligned inside equal
columns, so their right edges were ragged and the three read as unrelated buttons. Each now
fills its column at every width, as it already did below 30rem.

| 2026-11-01 | Stretch each approval answer across its column at every width. |

**Code you can take away.** Fenced code in a reply had no header and no copy key, so taking a
command meant hand-selecting it. The intent of §18.11 had been lost. Each block now carries its
fence's info string verbatim in 12px mono caption ink at the top left (no label when there is
none, and no language is ever inferred). At the top right is a 32px **Copy code** key that
writes the code exactly as fenced and shows a check for two seconds. Both sit inside the
block's one frame above the first line. While the reply is still streaming the key is
disabled, with the reason as its title, because half a command pasted into a shell is worse
than none.

| 2026-11-01 | Give fenced reply code its verbatim language label and a Copy code key that is disabled while the reply streams. |
### Plain-language failures (2026-10-31)

A failed read or command shows a sentence a person can act on. A service detail
written for people is shown as written. A server-side failure (5xx), a response
that carried no detail, or a lost connection is replaced by a plain sentence:
*Your sign-in has expired. Reload the page to sign in again.* (401), *Your
account does not have access to this.* (403), *This could not be found. It may
have been deleted or moved.* (404), *CMA is receiving too many requests. Wait a
moment, then try again.* (429), *CMA had a problem on its side. Try again in a
moment.* (5xx) and *CMA could not be reached. Check your connection, then try
again.* Protocol wording such as "problem document" never reaches the page.

| 2026-10-31 | Replace protocol and server-failure wording in resource, navigation and Agent errors with plain sentences that say what to do next. |
| 2026-10-31 | Give the shared Workspace read failure the same titled *Workspace unavailable* section, gutter and action placement as the owner Sandbox and Workspace failures. |
| 2026-10-31 | Rewrite New agent copy for people who do not write code: the new-Sandbox description explains a Sandbox as the Agent's private computer, the repository hint says when to paste a link, the Machine Included choice reads *Include a shared desktop* with what it is for, Agent settings explain Autonomous and where to change it, and the footer names ChronoAI models and the Ornn skill library instead of service ids. |

### Saying what an Agent is for (2026-10-31)

Agent Manager home keeps *Welcome to CMA* and adds one secondary line beneath
it: *An Agent is an AI assistant with its own private computer. Describe a task such
as research, writing, data work or code, and it works on it, asking you when it needs
a decision.* Paragraphs in the centred plain states use 8px top margin and secondary
label colour, and their action row sits 20px below. An owner's empty conversation
reads *Tell the Agent what you need in your own words, for example “Compare three
project management tools for a small team.”* in place of *Start the conversation.*;
a shared reader's empty history still reads *No messages yet.*

| 2026-10-31 | Add a one-line explanation of Agents to Agent Manager home, secondary paragraph and action spacing to the centred plain states, and an example request to the owner's empty conversation. |

### Unavailable views give a reason (2026-10-31)

A centre view that cannot open for this caller keeps its *<View> unavailable*
heading and adds one secondary sentence saying why and who can help, followed by
**Return to Agent Manager**. The inventory, Operations, Trigger publishing and
Agent sharing views each name their own reason; none is shown as an error.

| 2026-10-31 | Give every unavailable centre view a reason sentence and a Return to Agent Manager link. |
| 2026-10-31 | Let phone dialogs (480px wide or less) use the full height less a 16px gutter so New agent's Create agent key is visible without scrolling the card. |

### Explaining machine resources (2026-10-31)

The Sandbox Overview's **Machine resources** heading is followed by one secondary
line, *How much memory, processing power and network the Agent’s computer is
using.*, 8px closer to the heading than an ordinary paragraph. A read that fails
before any reading says *Usage readings are unavailable right now. Your Agent keeps
working.* and a runtime that cannot report says *This Sandbox does not report usage
readings.*; the distinct written states of §13a are otherwise unchanged.

| 2026-10-31 | Explain Machine resources in one line and reword its unavailable notices so they say the Agent is unaffected. |
| 2026-10-31 | Reword the Agents inventory description and its first filter choices (*Created any way*, *With or without a chat channel*) so the filters read as questions a person asks rather than record fields; filter names are unchanged. |
| 2026-10-31 | Cap the compact home mark at 134px, the size it rendered at beside the heading alone, so the new home explanation does not enlarge it. |

| 2026-11-01 | Use the conversation refresh's fresh entry account check to publish pending preparation synchronously, avoiding a duplicate account round trip while retaining authorization before reads and history publication. |

### Sandbox colours (2026-11-01)

Amends §28's *Body* column. Each Sandbox has one colour, and every Agent in it wears that
colour as its character's body wherever the brand lime used to stand: both sidebar views,
the live Thinking row and the empty conversation. Pending amber and failed red still replace
it for Needs approval and Needs attention, so status is read before ownership, and the
status word stays where §28 puts it.

The server assigns the colour (`SandboxView.color`) when an owner first sees the Sandbox and
keeps it for the Sandbox's life. It is drawn at random from bodies that hold up on both
themes (OKLab lightness 0.62–0.86, chroma at least 0.10), kept clear of the two status
colours, and chosen to sit as far as possible from the owner's other live Sandboxes. A
Sandbox without a colour (shared with the viewer, or while colours cannot be read) wears the
brand lime.

The colour is data, not a design token: it only ever paints the character's body and the
still mark's `currentColor`, applied inline. It never colours text, rows, borders or
controls, and no stylesheet refers to it.

| 2026-11-01 | Dress every Agent's character in its Sandbox's server-assigned colour in the sidebar, Thinking row and empty conversation, with approval amber and failure red still taking over and brand lime when a Sandbox has no colour. |

### 2026-11-03 — Sandbox usage

Account Usage now uses the popup specified below. Legacy Settings addresses
recover there; Appearance is canonical in the account menu.
Operations combines inventory and measured reporting in the independently
capability-gated Sandboxes & usage view with server-side owner filtering and paging. CPU core-hours, RAM GiB-hours, disk
GiB-hours, uptime and tracking coverage use explicit units. Partial histories and
failed reads are stated; unavailable values never become zero.

### 2026-11-03 — Operations usage controls

Operations now uses the shared measured report controls specified below. Sorting
and owner discovery run on the server across the frozen report cohort. The
previous client inventory scan and standalone Usage tab are retired.

| 2026-11-03 | Add User/status/Sandbox ID filters and accessible ascending/descending column sorting to Operations Usage using the existing form, Picker, Button and Table primitives. |

### 2026-11-04 — Task navigation and machine desktop integration

Task rows retain the stacked title and visible task state specified above. The
existing 24px Agent character accompanies that information, using the parent-aware
resource state and Sandbox colour. Its resource tooltip does not replace the task
state: a ready resource alone still does not prove a completed task.

An Agent with machine desktop access retains its desktop workspace and open/close
controls beside the conversation. Task plans, suggestions, feedback, deliverable
previews and Agent computer activity remain available in that same conversation.
Opening or closing either environment view preserves the current composer draft.

| 2026-11-04 | Combine task navigation and conversation features with the current Agent characters and machine desktop layout, preserving visible task facts and composer continuity. |

### 2026-11-04 — Independent desktop choices in Home

Home also exposes **Include a shared desktop**, using the existing native checkbox
and 44px labelled row. It starts unchecked and retains the existing creation
description: a Linux desktop with Chromium that the owner and Agent can both see
and control. It requires permission to create a new Sandbox; contextual creation
retains the parent's original setup. An unavailable retained draft may clear this
choice without losing its task text.

This choice is independent of **Enable desktop**, which prepares the Agent-bound
browser. Either or both may be selected. The server checks the selected image's
capabilities and reports unsupported combinations without changing either choice
or substituting an image. Neither selecting a checkbox nor refreshing admits work.
Both choices and the exact task survive draft refresh. After an uncertain or
accepted submission they remain frozen with the original body and key through
recovery and connection cancellation. A definite pre-admission rejection keeps
the draft editable; another explicit submission starts a new intent.

| 2026-11-04 | Preserve independent shared Linux desktop and Agent-browser choices in the task journey with frozen request recovery. |

### 2026-11-04 — Restore the task design hierarchy

The task journey follows the original CMA interface proposal, figures 3, 5 and 7.
Home has one compact task composer, optional repository and desktop disclosures,
an inline submit control and task suggestion chips. Repository/ref inputs and the
two independently authorized desktop choices expand only when requested. Existing
draft, admission and recovery semantics remain authoritative.

Tasks have one conversation column and one computer or deliverable column. The
computer and preview receive approximately half the available content width, with
a 360px floor per pane; narrower screens show the selected panel with a clear
return to conversation. Settings retain their narrow details column. The task
sidebar starts at 264px. Panel changes preserve the conversation and composer.

The computer has Browser, Terminal and Files tabs. The shared machine desktop is
rendered inside this same computer region, identified as Shared desktop; when an
Agent browser is also available an explicit source selector distinguishes them.
Switching away releases desktop input/connection. Existing owner, readiness and
running-task control restrictions remain in force. Terminal is read-only recorded
command output, with historical navigation secondary to live work.

Running plans show a compact Plan/count header, a thin lime progress bar, leading
step status icons and muted pending steps. Completed plans collapse to a summary.
No estimated time or activity is invented. Deliverables use compact responsive
file cards with a selected outline, beside a wide document preview. Recoverable
history-read failures use a concise local status; uncertain message admission
continues to receive an explicit recovery notice.

Confirmed completion uses a compact elapsed label, such as **Done in 4m**, in the
header. That latest answer does not repeat its duration below its files. Completed
operation groups collapse even if an individual operation failed; the failed count
remains visible, and the reader can expand the details. Current-turn failures still
open their group until the reader chooses otherwise.

| 2026-11-04 | Restore compact task entry, one wide computer/preview pane, leading plan status icons and compact deliverable cards from the original design. |

### 2026-11-04 — Finish task entry and recorded-operation disclosure

Home uses the original subtitle: **It gets its own computer, works through the task
step by step, and asks you when it needs a decision.** Task chips describe the task
in words rather than naming a document type. They only prepare the draft.

Terminal presents recorded commands as collapsed native disclosures. A summary
identifies the command number and its reported outcome; the exact command and
output remain intact inside. Record timestamps, cursors and turn metadata belong
inside the same disclosure. Plan snapshots remain readable outside it. Disclosures
reset when a different historical operation is selected. The computer's Browser
and Files views offer **View operation history** when history access and replay
capabilities are verified. The entry opens Terminal; it never admits a task.

Operation replay has compact Previous/Next/Return Live controls and a labelled
range input for the records in the loaded window. Its count is explicitly scoped
to that window, never represented as all task actions or video duration. Selecting
a record uses its exact cursor and preserves the existing read-only history rules.
Unavailable, empty and loading history has no enabled scrubber. Record controls
use existing 8px gaps, 12px padding, 8px radii, 13px/20px text and 44px mobile
targets; borders, text and the range accent use existing semantic tokens.

| 2026-11-04 | Restore Home copy, collapse raw operation details and keep recorded-history access and bounded navigation inside the computer. |

### 2026-11-04 — Keep active-turn diagnostics behind disclosure

The collapsed Thinking control shows the observed elapsed time and the count of
currently pending operations, or **View activity details** when none are reported.
Token counts, visible-token estimates, raw tool summaries and reasoning appear
only after expanding it. The Plan remains the primary progress explanation.
No current operation is attributed to a plan step without a reported association.

| 2026-11-04 | Move technical turn diagnostics into the existing Thinking disclosure so live task progress stays readable. |

### 2026-11-04 — Make running-task controls explicit

When queued follow-ups are supported, the running composer says **Add a follow-up
while the Agent works.** It does not promise delivery before the next tool step;
the chosen queue/interrupt mode continues to determine admission. The existing
Stop action gains a visible **Stop** label beside its icon, using the outline
button, 32px desktop height, 44px mobile height, 12px horizontal padding and an 8px
radius. Existing control availability and pending-request guards are unchanged.

| 2026-11-04 | Clarify supported running follow-ups and label Stop without changing queue or interruption semantics. |

### 2026-11-05 — Measured resource reporting

Measured usage surfaces share one immutable, principal-scoped report for their
selected range, filters, summary, charts and server-sorted pages. CPU cores,
memory working set and whole home-PVC usage remain distinct from allocated
capacity and resource-hours. Missing measurements and denominators stay unknown;
coverage, source time and last-observed disk values remain explicit. Money is absent.

Filters apply atomically. Last hour / 24 hours / 7 days / 30 days / custom (at
most 30 days) and supported server sorts persist in the address. Owner/Sandbox
choices search and page on the server; table pages contain 50 rows. Automatic
refresh runs at most once per minute while visible. Hide/close aborts requests;
pending reports resume by ID; denied or expired reports clear their data.
Pending, empty, partial, expired, denied, backoff and unavailable states remain
distinct. Retained results identify their previous selection.

Resource cards use layer-1 ground, border-l1, 12px radius and 16px padding. The
three-column grid stacks below 768px; filters use 12px gaps and actions 8px.
Recharts supplies nonanimated 240px line charts with brand/primary/secondary
ink, distinct dash patterns, 12px ticks, gaps without interpolation, one units
axis and a labelled current-allocation reference. A keyboard-readable HTML
table pages through 50 buckets and states time, units, coverage and gaps.
Percentages use matching summed usage/capacity inputs. Legends include Other
when present. Inline detail uses the same cards, all three charts and paged
lifecycle history.

| 2026-11-05 | Define shared measured CPU, memory and PVC reporting with explicit allocation semantics, bounded server paging, accessible charts and honest missing-data states. |

### 2026-11-05 — Operations resource dashboard

**Sandboxes & usage** replaces the separate Sandboxes and Usage tabs, guarded
by `read_all_sandboxes`. Warm pools, Platform settings and Activity retain their
existing gates and interactions. `tab=usage` is a legacy hint for the merged
view. Tab, range, owner/status/runtime/Sandbox/search filters and supported
server sort persist in URL state. The default range is the last 24 hours.

The report summary, three resource charts and 50-row inventory share a report
ID. Chart grouping offers Total / By owner / By Sandbox and whole-cohort top
series plus Other. Owner search covers the frozen cohort by subject prefix;
it never derives choices from the visible inventory page. Rows open inline
usage details, including View this owner's usage. Runtime details retain the
existing drawer; retirement retains its acknowledgement and uncertain-outcome
read recovery and still requires `control_any_sandbox`. Retained stale rows
never enable retirement. Unassigned/unattributed infrastructure readings stay
in an administrator-only diagnostic footer, excluded from owner totals.

| 2026-11-05 | Merge Operations Sandboxes and Usage into one measured dashboard, preserve runtime/control authorization, and replace browser inventory scans with report-backed server discovery, sorting and paging. |

### 2026-11-05 — Account Usage popup and canonical Appearance

Account Usage opens the shared Modal titled **Your usage** over the retained
Agent or service route. Opening never remounts the Agent editor, stream or pending
input. Charts load on the first visit. The closed dialog retains its report state,
aborts reads and pauses refresh; reopening resumes pending work within the report
lifetime. Owner endpoints are used even for administrators. Agent discovery is
paged and available only after an owner-scoped report confirms the Sandbox;
Agent selection is explicitly **Shared Sandbox usage**.

The card is at most 960px wide with 16px viewport clearance; below 768px it becomes
a full-height sheet. The close header stays fixed while content and horizontal
tables scroll inside. Details and lifecycle history expand inline with no stacked
dialog. Selecting CPU / Memory / Disk focuses that chart and its table columns.
Menu navigation focuses the retained account trigger before opening so Escape,
close and browser Back restore focus without discarding work.
On mobile the navigation sheet remains behind Usage so its account trigger can
receive focus on close. Escape inside a dialog belongs to that dialog and does
not also close the background navigation sheet.

The canonical /usage route saves its in-app background for Back/Forward. Direct
links use the Agent manager shell. /settings/usage replaces to /usage; /settings
and /settings/general replace with the one-time notice “Appearance is now in your
account menu.” The stored theme is preserved. Settings leaves the account menu.
Appearance has one System / Light / Dark choice group with menuitemradio selected
state, browser-local persistence, immediate preview, storage synchronization and
OS updates for System. Existing text-size controls remain available.

| 2026-11-05 | Replace account Settings with an owner-scoped measured Usage popup, preserve Agent work and legacy links, and consolidate Appearance into explicit account-menu choices. |
- 2026-11-05 — Name the account action Copy build version, copy the immutable bundle version and full revision, and expose accessible clipboard feedback with selectable fallback identity.

### 2026-11-05 — Allocated Usage before measured rollout

Operations **Sandboxes & usage** and the account **Your usage** popup default to
allocated resources. Show current allocated CPU cores, memory GiB and disk GiB
as three resource cards, followed by allocated core-hours/GiB-hours, the report's
effective period, and allocation-history completeness, unknown time and projected
tail. Retained disk continues accruing allocation time while paused. Summary,
50-row server pages and inline detail use the same report ID; lifecycle history
and authorized runtime controls remain available. Amounts and charges are absent.

Measured presentation follows the effective **Show measured usage** platform
setting, off by default. Administrators edit it in the existing Platform settings
form; other users consume only its public read-only exposure. An absent or
unavailable setting keeps measured presentation hidden. The setting does not
enable backend collection or billing. The default
view omits measured readings, coverage/source warnings, metric selectors,
measured sorting, grouping, charts and series requests, including inline detail.
Saved measured sorts resolve to last activity descending while hidden. Allocation
history gaps and report access/expiry/backoff/unavailable errors remain visible;
a source outage must not imply zero allocation. Enabling measured presentation
restores the existing measured UI alongside allocation values.

| 2026-11-05 | Default Usage to allocated capacity, resource-hours and allocation-history coverage; honor the effective Show measured usage platform setting without changing backend collection flags. |
### 2026-11-05 — New agent landing redesign

This contract supersedes prior task-home heading sizes (including 20/28), subtitle,
inline repository fields, Desktop options disclosure, 160px textarea, template
copy and composer geometry from 2026-11-03/04. Existing task admission, retained
intent, idempotency, draft persistence, permissions, recovery, connections,
provisioning and contextual creation behavior remain authoritative.

**Palette.** Extend static zinc with 440 #8B8B94, 460 #85858E, 720 #3A3A40,
790 #2A2A2E, 810 #26262A, 870 #1C1C1F, 880 #1A1A1D, 890 #19191C,
920 #131315, 930 #0F0F11, 940 #0B0B0C and 960 #060607; extend lime with
350 #D7FF5A and 420 #C8F542. Components read only aliases/specifics.
The following `--dsw-specific-task-*` tokens use these static steps:

| Suffix | Light | Dark |
| --- | --- | --- |
| page | zinc 0 | zinc 940 |
| shell | zinc 50 | zinc 920 |
| border | zinc 200 | zinc 810 |
| panel | zinc 0 | zinc 890 |
| panel-border | zinc 200 | zinc 790 |
| focus-border | zinc 400 | zinc 720 |
| primary | zinc 950 | zinc 100 |
| weak | zinc 600 | zinc 440 |
| placeholder | zinc 600 | zinc 460 |
| accent | lime 800 | lime 420 |
| accent-hover | lime 900 | lime 350 |
| on-accent | zinc 0 | zinc 940 |
| hover | zinc 100 | zinc 870 |
| disabled | zinc 200 | zinc 810 |
| muted-icon | zinc 500 | zinc 500 |
| menu | zinc 0 | zinc 900 |
| focus-ring | 0 0 0 3px lime 800 at 20% | 0 0 0 3px lime 420 at 20% |
| scrim | zinc 950 at 42% | zinc 960 at 72% |

Secondary text keeps `--dsw-alias-label-secondary`. `--dsw-specific-sidebar-fill`
becomes zinc 50/light, zinc 930/dark. New `--dsw-specific-sidebar-new-current`
is zinc 100/light, zinc 880/dark. Borders use the existing zinc 200/light,
zinc 850/dark alias. All fills are flat, without gradients.

**Sidebar.** Current New agent has the current fill and task accent plus glyph.
Search retains platform-aware ⌘K / Ctrl K in an 11px/18px mono kbd with 1px border,
4px radius and 4px horizontal padding. The account area has a 1px top divider.
Ready to start has a static task-accent 8px dot and its own `ready` tone; Completed
has a zinc 500 neutral dot and a separate `completed` tone. Other task states retain
existing semantics. The current flag applies only where task home is rendered.

**Landing.** Center vertically with auto block margins in the existing scrollable
center column; content width is at most 780px, with 24px outer padding (828px maximum outer
width; 16px padding at 600px and below). Grid gaps are 20px. A centered noninteractive 30px pill, 1px shell
border, 999px radius, 12px horizontal padding and 8px gap reads **New agent** with
an 8px accent dot. The heading follows the §4 display exception. Subtitle:
**Give it a task. Your Agent works through it on a cloud computer.** The bottom
mono line reads **GPT-6 · High reasoning · Autonomous**, with the same accent dot.
Long retained content wraps within the column; errors and provisioning stay reachable.

**Composer.** Outer 26px radius, 6px padding, 1px shell border; focus-within uses
focus-border. Toolbar has 6px padding, 8px gaps, wrapping 36px controls with 12px
horizontal padding and 999px radii, separated by a 20px vertical hairline. Inner
panel has 20px radius, 1px panel border and 16px padding. Textarea has minimum
128px height, no resize handle, 17px/26px Geist, transparent ground and placeholder
**What would you like your Agent to do?** Footer has 8px top padding, 12px gap,
right-aligned mono platform hint **⌘ ↵ to run** (Ctrl off Apple platforms) and a
40px circular arrow-up send button. Blank/whitespace input and existing admission
constraints disable it with disabled fill and zinc 500 icon. Enabled fill is
accent, hover accent-hover, foreground on-accent. Cmd/Ctrl+Enter uses form submit,
never duplicates admission and does not submit during IME composition. No attachment
control is offered. Named task controls may specialize shared Button/Input surfaces
with these documented dimensions, colours and radii; other consumers are unchanged.

**Run target.** Reuse Menu focus, navigation, portal and dismissal. A documented
`run-target` presentation uses 320px width bounded by viewport minus 32px, 18px
radius, 8px padding and menu fill. Trigger has 1.5px accent outline while open.
Group labels use 12.5px/20px weak text. The sole location is **Cloud computer**,
a checked menuitemradio with a 32px icon square, 8px radius, 1px panel border and
secondary **Its own Linux machine in the cloud**. A separator and **Computer options**
label precede the sole **Shared desktop** menuitemcheckbox for new drafts, with
its description/unavailable reason at 12px/18px. It includes Chromium for the
owner and Agent. Only a recorded lifecycle intent with `desktop: true` also shows
a checked, disabled **Agent browser** row explaining that its choice is saved
with the submitted task (see the 2026-11-06 refinement). Menu gains per-item roles
so independent checkboxes and a radio can coexist. Checked rows show an accent
check; row padding is 8px, gap 12px, minimum height 44px. Choices freeze after intent;
a retained unavailable choice can still be unchecked before intent. The trigger suffix is
**Shared desktop** when selected; no Browser or combined suffix is used.

**Repository.** Reuse Modal with a `repository` presentation: full-screen scrim,
8px backdrop blur, 16px viewport clearance, 480px maximum card width, 26px radius,
24px padding and 20px gaps. A centered 56px repository icon square (24px glyph, 16px radius,
1px panel border), title **Add git repo**, and secondary subtitle **Your Agent
clones it onto the cloud computer before it starts.** precede required GitHub
repository and optional Repository ref fields. Labels use 14px/22px, gaps 8px;
field groups 16px apart. Shared Input specializes to 48px height, 12px radius,
accent focus border and the 3px specific focus ring. Ref uses Geist Mono. Helper:
**The requested ref is verified during preparation.** Footer is two equal columns,
12px gap, 48px actions, 12px radius, Cancel outline / Add repo accent. Close is a
32px icon button at top/right 12px. Repository input is initial focus; existing
Modal trap, Escape and focus return apply. Invalid nonempty refs show an inline
error; Add repo/Enter require valid repository and ref. Trim surrounding whitespace and accept bare owner/repo or
optional https://github.com/, optional .git and trailing slash; store canonical
HTTPS github.com URL. Reject invalid/dot segments and keep existing ref constraints.
Unsupported/contextual creation disables adding with an accessible reason. Retained
repositories remain removable; contextual creation always inherits its repository.

The toolbar chip shows GitHub mark, editable owner/repo name, branch glyph and
mono ref (**default** if empty), and a labelled remove button. It wraps with 8px
gaps, 36px minimum height, 999px radius and shell border. Name/ref truncate within
160px each; removal preserves text. Editing pre-fills the dialog. No second dialog
is opened inside the contextual creation dialog.

**Templates and feedback.** Five wrapping 42px chips use 12px radius, 1px panel
border, 12px horizontal padding, 8px gaps, 13px/20px text and hover fill. Icons:
magnifier, table, file, globe, code brackets. Their prompts are respectively:
- Research [topic] and summarize the key findings, with links to sources.
- Analyze the spreadsheet at [link] and point out trends, outliers and what to do next.
- Write a [type of document] about [subject] for [audience].
- Build a one-page website for [project] and share a preview link.
- Write a [language] script that [does what].

Click replaces the unsent draft, focuses the textarea and selects the first bracketed
placeholder. Below the composer, pending submission or accepted queued/running
preparation shows one status pill: **Preparing a cloud computer for your Agent…**,
with 8px accent dot, 12px/20px text, 8px/12px padding, shell border and 999px radius.
Dot opacity pulses 1 to .4 over 1.8s using the shared easing; reduced-motion removes
the animation. Failed, waiting-for-connection, succeeded or uncertain recovery uses
its existing state instead. Receipt text does not duplicate this announcement;
provisioning milestones remain visible but do not make a competing live announcement.

At coarse pointers or widths ≤600px, interactive landing controls, templates, dialog
close and chip removal have minimum 44px targets. Toolbar/chips wrap, textarea is
fluid, model text can wrap, heading scales to 32px. AppFrame retains its auto-collapse
and overlay behavior; the sidebar never restacks above the content.

| 2026-11-05 | Redesign New agent landing with a layered composer, shared run-target menu and repository dialog, accessible templates, preserved lifecycle recovery and matching light/dark tokens. |


#### Review refinements — 2026-11-05

Repository capability is tri-state. While checking, the disabled Add git repo control
has the screen-reader-only description **Checking repository support…**, and
submitting a retained repository says
**Checking repository support. Wait for the check to finish before starting.** Only a
confirmed unsupported result or absent capability reader says unavailable.

Contextual creation uses a compact task-home variant inside the shared **New task**
Modal: visually hidden section heading, no New agent pill or display subtitle, no
viewport-sized type. **Creating in Workspace/Sandbox X** is secondary 14px/22px.
The contextual Modal uses 16px viewport clearance, at most 700px width, 26px radius,
shell ground and panel border; maximum height is viewport minus 32px. Its body has
16px top spacing and 24px horizontal padding; task content has zero padding, 12px
gaps and no minimum width. Composer stays ahead of templates and model metadata.
The initial composer must be entirely visible without scrolling at 1440×900 and
390×844. Long subsequent states scroll within the dialog. The contextual run-location
reason is **This Sandbox's existing Linux machine**; preparation says **Preparing
your Agent…**. New-Sandbox landing retains its original cloud-computer wording.

Shared-desktop validation points to **Shared desktop in the Cloud computer menu**.
As refined on 2026-11-06, new drafts have no Agent browser row, validation or
capability read. The trigger suffix is only **Shared desktop** when selected.

Repository errors use the same 14px/22px error ink as ref errors. A nonempty invalid
repository is explained after blur; Enter also validates empty input. The form uses
noValidate and aria-required, never native required bubbles. The error is **Enter
owner/repository or a github.com repository URL.**, connected by aria-describedby
and aria-invalid. All template icons are 16px, including a matching globe glyph.
The task textarea exposes Meta+Enter on Apple platforms and Control+Enter elsewhere
through aria-keyshortcuts. Toolbar controls and repository chip use the same 36px
outer height (44px at coarse pointers or ≤600px), with border-box sizing. Chip
borders are inset so they do not increase row height; chip name/remove controls use
that same outer height. Adding an ordinary repository does not move the composer.

**Retained task and feedback surfaces.** Submitted task and provisioning use panel
ground, panel-border, 20px radius, 16px padding and 12px gaps. Submitted heading is
16px/24px at 600; task text is Geist 14px/22px with pre-wrap and anywhere wrapping.
Repository URL, ref and checkout commit are Geist Mono 12.5px/20px; labels are
secondary 12px/18px. Metadata uses an 8px-gap grid and 4px label/value gaps. Status,
context, explanation and permission/loading feedback are centered 14px/22px with
zero paragraph margins; alerts use error ink on panel ground, 1px panel-border,
12px radius and 12px padding. Recovery/continue/start-another/retry/manual actions
are centered, at most full width, 40px high (44px for touch/≤600px), 12px radius,
16px horizontal padding, outline border and primary text. Continue/start-another
may use the task accent fill and paired foreground; hover uses task hover or
accent-hover respectively. Provisioning retains reported milestones and times,
with primary labels, secondary details, task-border rules and task-accent active/
complete rules. The shared provisioning component owns this named task variant;
other callers keep their existing presentation. No state, recovery action, error,
requested ref or checkout commit is removed.

Shared Button/Input placement remains placement-only, never colour/depth/size,
except the task-home composer and repository dialog controls documented here.
Task status dots are owned by Tree.css beside the state labels, including shared
navigation consumers. Text contrast measurements for task paints are recorded in §2.

| 2026-11-05 | Refine repository checking and errors, compact contextual creation, retained-task feedback, browser-option copy, stable toolbar geometry, accessible shortcuts and measured task contrast. |

At ≤600px the repository slot reserves a second toolbar row for both Add git repo
and its chip; the divider is hidden. This keeps ordinary add/remove operations at
the same toolbar height even when a repository name is wider than the Add label.


#### Task entry failure and toolbar refinements — 2026-11-05

Supersedes the preceding narrow-toolbar reservation and placement-exception wording.
At ≤600px the empty repository slot remains in the toolbar's normal wrapping flow,
with its divider; Cloud computer and Add git repo share a row when they fit. Only
an existing repository chip reserves its own row and hides that divider. Desktop
add/remove retains constant toolbar height. Neither state overflows at 320, 360 or
390px. The hidden contextual section heading reads **New task in Sandbox X** or
**New task in Workspace X**.

The sole Button/Input placement specialization exception is **task-home landing
controls (composer toolbar, send, repository chip, templates, recovery actions)
and repository dialog controls**. All other placement classes remain placement-only,
never colour, depth or size.

Unknown/network/timeout failures do not expose browser or transport diagnostics.
Uncertain task submission says **We couldn't confirm your task was received. Use
Recover original task to resend the same request.** A failed task status read says
**Task status couldn't be read. Use Retry task status to check again.** Manual Agent
creation uses the equivalent sentences naming **Recover creation** and **Retry
status**. An explicit 400/422 creation refusal without a readable detail says
**Your task could not be started. Review it and try again.** (or **Your Agent could
not be created. Review its settings and try again.** in manual creation).
Deliberate identity/verification and saved-request messages and readable service
details remain intact. These presentation rules do not change retries, retained
requests, or their keys. Failed preparation omits the promise that progress and
results will appear in a conversation. Error-ink contrast on task panel and shell
is measured in §2 alongside the other text paints.

| 2026-11-05 | Make task failure guidance actionable, restore compact empty mobile toolbars, clarify contextual labels and control exceptions, and verify error-ink contrast. |


#### Failure actions and explicit refusals — 2026-11-05

Failure guidance names the action rendered by the current surface. Task home uses
**Recover original task** before a receipt, **Continue original task** when a
connection-gated request must resume, and **Retry task status** after a receipt.
Manual Agent creation uses **Recover creation** / **Retry status**. The Agent
conversation's setup follow-up uses **Retry setup status** for read failures and
uncertain continuation results. When the available action only reads status, the
sentence says **check again**, never **resend the same request**. An uncertain
continuation in that surface says **We couldn't confirm your task setup. Use Retry
setup status to check again.** (or **Agent setup** for manual creation).

A detail-free, non-retryable HTTP submit refusal says **Agent Manager refused this
task (403). Your original request is saved.**, substituting the actual status and
**Agent creation** for manual creation. The existing typed 400/422 validation
refusal keeps its review-and-try-again sentence and existing retention behavior.
Only transport/unknown errors, 408/504 timeouts, and detail-free 5xx/429 responses
use uncertain-submit wording. Status-read failures describe the failed read, not a
refusal of the earlier creation request. Readable service details and deliberate
identity/storage messages remain unchanged. Copy classification never changes
request retention, key reuse, polling cadence, or recovery actions.

| 2026-11-05 | Match lifecycle failure guidance to each surface's visible action and distinguish explicit HTTP refusals from uncertain submission. |


### 2026-11-06 — Theme palette roles and environment management

**Canonical surfaces.** Appearance stays in the account menu as the existing
System / Light / Dark menuitemradio group. The existing useAppearance,
setAppearance/startAppearance, storage key, System media-query updates and storage
synchronization remain unchanged. Usage retains the **Your usage** Modal/sheet,
owner-scoped reporting, retained Agent work and focus return. Legacy /settings
links keep develop's redirects and **Appearance is now in your account menu.**
notice. No Settings page, appearance cards, sidebar Settings link, or preview
palette is introduced.

**Palette.** Light uses #FFFFFF page/card/menu, #F7F7F8 sidebar/secondary layer,
#E6E6E9 decorative border, #ECECEF selected, #111113 primary, #52525B secondary,
#62626B weak and #4D7C0F accent. Dark uses #0B0B0C page, #0F0F11 sidebar,
#1F1F23 decorative border, #141416 card, #18181B menu/secondary layer,
#1C1C1F selected, #F4F4F5 primary, #A1A1AA secondary, #8B8B94 weak and
#C8F542 accent. Static additions are zinc 75/150/190/550/915 and lime 700.
Components read alias/specific roles, never static paints. §2 records every
changed alias and measured text contrast in both themes. Decorative borders stay
subtle; icons, focus indicators and selected-state marks meet ≥3:1.

Primary buttons, including Send and Add repo, use lime 420 #C8F542 and zinc 940
#0B0B0C text in both themes; hover is lime 350 #D7FF5A and pressed fill lime 500
#AEE734. The button-primary-foreground role separates button ink from selection
ink. Send and Add repo use the shared primary Button variant. Other button fill
families, border-l3/l4, success state and unrelated bubble/highlight roles retain
their previous values.

**Approved task surfaces.** Composer and repository dialog keep shell #FAFAFA /
#131315, shell border #E4E4E7 / #26262A, panel #FFFFFF / #19191C, panel border
#E4E4E7 / #2A2A2E, focus border #A1A1AA / #3A3A40, placeholder #52525B / #85858E,
menu #FFFFFF / #18181B, disabled send #E4E4E7 / #26262A, hover #F4F4F5 / #1C1C1F
and muted icon #71717A in both. The scrim remains zinc 950 at 42% / zinc 960 at
72%, and the 3px focus wash follows accent. Only task page, primary/weak text,
accent, button fill/foreground and button hover follow shared app roles.

**Selected text.** Appearance and System text size selected choices use selected
fill, primary ink, 600 weight and aria-checked, without underline. Light accent
text on #ECECEF is below AA and is not used there. Selected command suggestions
and links within selected document cells use primary text (document links keep
their existing underline). Current sidebar links share New agent's selected
fill and accent glyph. Active fills use the shared selected-ground role. Account
menu groups, Usage text and the legacy notice are covered by contrast checks.

**Environment management.** The misleading Settings heading and its one-item
Navigation settings menu are removed. The existing Tasks sliders disclosure keeps
its Scope picker, Status picker and Clear filters, with existing labels and
persistent filter behavior. A native **Manage environments** button is separated
at the panel's end by a 1px border-l2 rule and 8px top padding, only in Tasks view.
It shares Clear filters' 32px height (44px below 30rem), 8px horizontal padding,
6px radius, 12px/16px secondary text and neutral hover. Tab reaches it; Enter/Space
activates it. Selection closes the disclosure and preserves the existing
route/scope/scroll/focus return path. **Back to tasks** draws an inset 2px accent
focus ring, with no outer outline/shadow or geometry change, fully visible in
both themes. Scope and status selections themselves keep the outer panel open.

**Changed alias audit.** Unlisted aliases retain develop's values. Sidebar and
selector specifics implement the sidebar/selected roles; task surfaces retain
the approved values above. Existing accent focus/glow effects follow the new
accent RGB values.

| Alias | Owner palette role |
| --- | --- |
| `--dsw-alias-bg-base` | page |
| `--dsw-alias-bg-layer-1` | card |
| `--dsw-alias-bg-layer-2` | sidebar/secondary surface |
| `--dsw-alias-bg-layer-3` | selected |
| `--dsw-alias-bg-multi-select` | selected |
| `--dsw-alias-border-l1` | border |
| `--dsw-alias-border-l2` | border |
| `--dsw-alias-border-l2-darkmode-thin` | border |
| `--dsw-alias-brand-primary` | accent |
| `--dsw-alias-brand-text` | accent text |
| `--dsw-alias-brand-primary-invert` | accent paired foreground |
| `--dsw-alias-button-ghost-active-fill` | selected |
| `--dsw-alias-button-primary-fill` | lime button fill |
| `--dsw-alias-button-primary-hover` | lime button hover fill |
| `--dsw-alias-button-primary-active` | lime button pressed fill |
| `--dsw-alias-button-primary-foreground` | lime button foreground |
| `--dsw-alias-interactive-bg-active` | selected |
| `--dsw-alias-interactive-bg-hover-solid` | selected/hover |
| `--dsw-alias-interactive-bg-hover-accent` | accent wash |
| `--dsw-alias-label-primary` | primary text |
| `--dsw-alias-label-primary-bluish` | primary text |
| `--dsw-alias-label-tertiary` | weak text |
| `--dsw-alias-state-business-primary` | accent |


| 2026-11-06 | Align theme palette roles and lime primary buttons, maintain accessible account choices and canonical Usage surfaces, and move environment management into the existing Tasks filter disclosure. |

### 2026-11-06 — Readable allocated usage

The personal Usage popup and Operations inventory share the native zinc/lime
palette, type scale, 12px panel radius and 8/12/16/24px spacing. Apply filters uses
the primary button; refresh, retry and paging use outline buttons. Filters
align at the control edge and wrap as a toolbar; optional Agent guidance stays
on a separate line. Existing modal bounds and focus behavior remain authoritative.

Three resource cards lead with period resource-hours in 24px tabular values and
label current allocated capacity separately beneath, followed by an expandable
history explanation. Missing history is stated
plainly, durations use days/hours/minutes, and the explanation distinguishes
missing intervals from extension of the last known allocation. Missing time is
excluded, never described as zero. Retained disk accrues while paused.

Recharts renders nonanimated 240px horizontal bars for resource-hours by Sandbox,
or by owner in Operations, using only the currently displayed server page. Chart
scope states the page's Sandbox count against the report count; the largest eight
page groups are drawn, with all page groups in a keyboard-readable data table.
CPU core-hours and memory/disk GiB-hours have separate resource selections and
labelled units. No allocation timeline or whole-cohort owner/status aggregation
is inferred from missing API fields. Existing measured visibility gates hold.

Inventory rows group name, status, optional owner and expandable runtime/identity
metadata. Resource-hours align in three sortable numeric columns, with current
capacity beneath. Sort selection also exposes server-supported owner, status and
last-activity sorts. Full identifiers/image references are available on hover,
keyboard focus and copy, with selectable fallback on clipboard failure. Below
640px of content width, rows stack identity above three labelled resource cells;
the table retains its native headers and semantics without horizontal overflow.
Loading reserves card/chart geometry with the shared skeleton. Errors retain
visible retry actions, and empty inventories never draw invented bars.

| 2026-11-06 | Redesign allocated Usage hierarchy, filters, scoped charts, compact responsive inventory and plain-language history using existing CMA primitives; reviewed in the single usage redesign milestone PR. |

#### Usage review refinements — 2026-11-06

Label every toolbar control, including owner and Sandbox discovery. The usage
scope uses equal 44px control heights, matching the shared input; its picker and
Apply placements take the same height. A seven-column Operations row becomes a
three-column grid below 1000px of content width, then two columns below 720px and
one below 320px. Personal filters use three inputs and Apply, then the same
narrow grid. Discovery values stay on one line with full values on hover.

Refresh belongs beside the update time in the allocation overview header. The
count and concise SGT period share a compact metadata row with 8px separation
from the heading. Neither header nor chart adds a sticky overlay. Sections keep
16px scroll clearance, and chart/table captures align their headings inside the
scrolling content area. History uses a labelled What's missing?/Details
disclosure. Chart names have a 136px label column, up to two lines and a full SVG
title; plots reserve at least 40px per row, with a 240px minimum. Inventory names
and status chips share a line, secondary metadata wraps together, and permission
hints stay beside the action they explain.

| 2026-11-06 | Apply lead review to Usage toolbar alignment, overview spacing, refresh placement, history disclosure, chart labels and compact inventory rows. |
| 2026-11-06 | Specify Ops usage dropdown search, bounded paging, keyboard navigation, visible discovery states and human-readable identity labels; preserve Custom date ranges and atomic Apply/Clear. |


#### Settled landing capability checks — 2026-11-06

Task-experience capability is explicitly unknown until settled. Only a settled
non-admitted result (including a failed read, mismatched account or missing reader)
may show **Task creation is unavailable. You can create an Agent and start a
conversation.** and its **New agent** button. Unknown capability never shows that
fallback. Focus, visible-document and online refreshes preserve the same scope's
last settled capabilities until the new read succeeds or fails. Client or account
changes immediately hide the previous scope's capabilities; late responses cannot
restore them. Suggestions and Agent computer controls use the same behavior.

Going offline aborts the read and invalidates capabilities to unknown, disabling
new admission without showing misleading unavailable copy. Focus while offline
does not read or restore capabilities; reconnecting starts a fresh check. All
capability consumers remain fail-closed until that check settles. Same-scope
refreshes otherwise keep the last confirmed admission; the server still enforces
current authority on every submission.

Initial and refresh checks must not insert transient visible content or shift the
landing. **Checking task creation…** is a screen-reader-only status; **Checking
repository support…** is a screen-reader-only description of the disabled repo
control. Settled unsupported/error reasons remain visible, as do meaningful
submission and preparation statuses. Repository support retains its existing
tri-state validation and contextual inheritance. Start task stays disabled until
creation support and task admission are confirmed; pending repository support
still prevents submitting a retained repository.

| 2026-11-06 | Keep landing capability checks visually stable and scope-safe: no unavailable fallback before a settled result, same-scope refreshes keep the last settled capabilities, offline invalidates admission, and checking statuses are screen-reader-only. |


#### Shared desktop as the only landing computer option — 2026-11-06

This refinement supersedes earlier independent Agent browser selection on Home,
including the 2026-11-03/04 desktop creation rules. Both the new-Sandbox landing
and contextual Workspace/Sandbox New task menu offer only **Shared desktop** under
**Computer options** for new drafts. Shared desktop includes Chromium, which the
owner and Agent can both control. Existing Sandbox and permission restrictions
remain; a retained unavailable shared-desktop choice can still be cleared.
The trigger suffix is **Shared desktop** only when selected. New submissions
never request `desktop: true`, and the landing does not read browser capability
or use the desktop-enable policy. The owner's **Enable browser** command after
creation is unchanged.

Version-1 task drafts containing a boolean `desktop` still load their text,
repository and shared-desktop choice. Ignore that retired field immediately and
omit it on the next persist. Recorded lifecycle intents are a separate boundary:
keep their original key and body byte-identical during recovery, including
`desktop: true`. Only such a frozen intent shows a checked, disabled **Agent
browser** row, with **Agent browser choice is saved with the submitted task.**
It does not add a Browser trigger suffix or require a capability read.

| 2026-11-06 | Offer only Shared desktop for new landing tasks, accept and drop the retired desktop field in old drafts, and keep recorded Agent browser intents frozen, visible and byte-identical on recovery. |


#### Next draft after confirmed creation — 2026-11-06

Keep the editable next draft through submission, receipt, uncertain recovery,
connection waits and failed preparation; **Start another task** still offers it.
When a confirmed successful operation for the retained request's Agent clears
its intent, discard the same scope's draft only if its exact text, repository URL,
repository ref and Shared desktop choice still equal the submitted body (missing
options mean empty/unchecked). An edited next draft survives unchanged. This
applies to conversation follow-up and creation-dialog success paths, never to
receipt alone or failed-intent clearing. A later landing starts empty, without a
repository chip and with Shared desktop unchecked; a mounted matching draft also
resets immediately so subsequent edits cannot re-save the submitted content.
Draft cleanup is best-effort: storage errors must not throw or block conversation
access, and unreadable or different drafts are preserved. Account isolation and
sign-out clearing keep their existing rules.

| 2026-11-06 | Discard the landing draft only when it still equals a task whose creation was confirmed successful; edited next drafts and failed preparations keep their text. |


#### Streaming Markdown safe prefix — 2026-11-06

Streaming replies render only a safe Markdown prefix. An open link or image,
an unfinished link label, and unmatched `**` or `__` in the final paragraph stay
hidden until their closing syntax arrives. Emphasis uses CommonMark flanking,
including adjacent CJK text, punctuation and intraword asterisks. A blank line
ends the paragraph's emphasis scope: unmatched delimiters in completed
paragraphs are literal and cannot pair with a later paragraph or hide it.
Literal underscores in words and spaced mathematical powers remain visible.
Inline code closes with a backtick
run of the same width, and the first remaining unclosed run stays hidden.
Code spans stay within a paragraph; a blank line ends that scope. Unmatched
backticks in completed paragraphs remain literal and cannot consume a later
paragraph's code span. A single line break can remain inside a code span.
Masking code preserves UTF-16 offsets, including emoji. Text inside an open
fenced code block remains literal. A partial `<!--cma-deliverable:v1:` prefix,
including a one-character prefix, is hidden only on the final line still being
written; earlier lines keep their literal characters. Full marker prefixes
remain hidden outside fences. Completed messages keep their existing bytes and
marker handling.

| 2026-11-06 | Render streaming replies through a safe Markdown prefix: CommonMark-flanking emphasis and code spans scoped to the paragraph still being written, UTF-16-exact masking, and partial deliverable markers hidden only on the final line. |



#### Agent conversation streaming continuity — 2026-11-06

A live stream gap preserves every visible message and reasoning row. The client
freezes that run's live tail after the gap and waits for its durable message;
it never appends a post-gap fragment to a buffer with a hole. Durable text
replaces the live row in the same timeline position and keeps its DOM identity.
Snapshot merges retain that identity on subsequent refreshes too. The durable
successor replaces a frozen live row even when its text differs; an earlier
durable message from the same run never consumes the second message's live row.
The **Refreshing live updates…** status occupies one fixed slot at the end of
the message stream and clears when the run is recorded or reaches a terminal
outcome. Active thinking, composer busy state and other turn loading indicators
also end at that terminal observation.

| 2026-11-06 | Preserve streamed text across live gaps, replace frozen live rows in place with their durable successor and a stable DOM identity, and keep the live recovery status in one slot that clears at the run's terminal outcome. |



#### Task list status for the open Agent — 2026-11-06

When the open Agent is newer than its sidebar summary, the task list keeps the
summary's last reported task status while requesting a navigation refresh.
**Status unavailable** is shown only when the Agent's summary has never
reported a task status.

| 2026-11-06 | Keep the open Agent's last reported task status in the sidebar while its stale summary refreshes; show Status unavailable only when no status was ever reported. |
