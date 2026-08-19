# Frontend refactor bundle

The five files staged in the `docs_refactor_fe` handoff folder, combined verbatim into one document.
Nothing below has been edited, reflowed or reconciled — each part is the file it names, as it was
filed. Where the parts disagree, the individual sections are the record of what was written, not a
settled decision.

| Part | Original file | Lands in the repo at | Kind |
|---|---|---|---|
| A | `README.md` | nowhere — handoff note | note |
| B | `ISSUES.md` | `docs/ISSUES.md`, and as GitHub issues in the milestone | plan |
| C | `Web_Style.md` | `docs/Web_Style.md` — replaces §2–§18, keeps the changelog | spec |
| D | `tokens.css` | `web/src/styles/tokens.css` — replaces it | code |
| E | `components.css` | `web/src/styles/components.css` — replaces it | code |

The two stylesheets are markdown code blocks here because `CLAUDE.md` §2 requires every file in
`docs/` to be markdown. Their destination is `web/src/styles/`, not `docs/`.

---

# Part A — `README.md`

# Filing the redesign

Everything an agent needs to carry out the frontend refactor. Five files staged in this `docs/` folder,
and the order they land in matters — `CLAUDE.md` §7 makes `docs/Web_Style.md` the single source of truth, so the spec is merged
before any code that implements it.

## What goes where

| File here | Lands in the repo at | Kind |
|---|---|---|
| `Web_Style.md` | `docs/Web_Style.md` — replaces §2–§18, keeps the changelog | spec |
| `ISSUES.md` | `docs/ISSUES.md`, and as GitHub issues in the milestone | plan |
| `README.md` | nowhere — it is the handoff note, and stops being true once PR 1 merges | note |
| `tokens.css` | `web/src/styles/tokens.css` — replaces it | code |
| `components.css` | `web/src/styles/components.css` — replaces it | code |

The two stylesheets are staged here beside the spec they implement, but they do **not** land in the
repository's `docs/`: `CLAUDE.md` §2 requires every file in `docs/` to be markdown. They go to
`web/src/styles/` in PR 3.

The two design documents stay outside the repository, because `CLAUDE.md` §2 allows only markdown in
`docs/` and only the fixed list at root:

- `ChronoBot Redesign.dc.html` — the eight frames every issue is reviewed against, numbered 01–08.
- `ChronoBot — Current UI (recreated).dc.html` — today's build, recreated from `web/src`, for before-and-after.

Reference them by frame number in issues and PR descriptions (“matches frame 04”). Keep them wherever
the team keeps design files, and link that location from the milestone description.

## Rules that govern the work

Taken from `CLAUDE.md`; the refactor does not get an exemption from any of them.

- The spec PR is **spec only**, first, and gets argued about on its own. No code.
- The changelog in `docs/Web_Style.md` is **append-only**. Keep every existing entry byte-for-byte and
  add the new one at the bottom. `Web_Style.md` here carries the new entry and a marker where the old
  ones go — do not retype or reflow them.
- Every PR targets `develop`, is small, and clears the frontend gates: Prettier, ESLint at
  `--max-warnings=0`, `tsc --noEmit`, Vitest, `npm run build`, `npm audit`, `npm ci`.
- No colour, radius, shadow, type step or spacing value may appear in a component that is not in the
  token layer. A literal hex or an inline `box-shadow` in a `.tsx` or a component `.css` is a defect.
- Any change to config keys, env vars or build inputs updates `infra/web/` in the same PR.

## Two decisions to make before PR 1

1. **The typefaces must be vendored.** §4 forbids fetching from a third party, and the design uses
   Figtree and JetBrains Mono. Both are SIL OFL. Add the woff2 files and their licence text to
   `web/src/assets/fonts/`, and delete the Geist pair in the same PR that removes the last reference
   to it. If vendoring is refused, the fallback is Geist for the interface and Geist Mono for data —
   the design survives it, the type scale does not change, and the spec entry needs editing before it
   is merged rather than after.
2. **Whether the light theme ships with the dark one.** The spec specifies both and records measured
   ratios for both. Shipping dark first is allowed; shipping light *unmeasured* is not.

## Reading order for whoever picks this up

1. `docs/Web_Style.md` §1 — the information architecture, which the redesign does not change.
2. This spec's §19 — what deliberately did not change, so nothing gets "fixed" that was already right.
3. `ISSUES.md` — the dependency order. Issues 1 and 2 touch every surface; land them first.

---

# Part B — `ISSUES.md`

# Frontend refactor — Quiet instrument

Milestone plan for implementing the `docs/Web_Style.md` (Quiet instrument revision) in `web/`.

Every issue names **the spec section it implements** and **the frame it must match**, so review is
"does this match §N" rather than a matter of taste. Frames are numbered 01–08 in
`ChronoBot Redesign.dc.html`; `ChronoBot — Current UI (recreated).dc.html` is today's build for
comparison.

Per `CLAUDE.md`, the spec lands first, in its own pull request, before any of this is implemented.
Every PR targets `develop`, is small, and clears the frontend gates.

---

## PR 1 — spec only (blocks everything)

Replace §2–§18 of `docs/Web_Style.md` with the Quiet instrument revision. Keep every existing changelog
entry byte-for-byte and append the new one.

No code. This is the PR that gets argued about, and keeping code out of it means the argument is about
the design rather than the diff.

**Done when:** the document is merged and the three decisions it records are settled in writing — the
warm-neutral ramp with its measured ratios, two depth levels replacing letterpress, and the details
drawer replacing the bottom sheet.

## PR 2 — fonts

Vendor Figtree and JetBrains Mono (variable woff2, SIL OFL) into `web/src/assets/fonts/` with their
licence text beside them, declare them in `styles/base.css` with `font-display: swap`, and delete the
Geist pair in the same PR as the last reference to it.

**Done when:** no font is requested from a third party; text is readable in the fallback before the
faces arrive; `grep -r Geist web/src` returns nothing.

## PR 3 — the token and component layer

Replace `web/src/styles/tokens.css` and `web/src/styles/components.css` with the sheets filed beside
this plan.

**Done when:** every existing component still renders (a visual diff is expected and intended), no
component declares a literal colour, radius or `box-shadow`, and `grep` finds no `var(--)` reference
without a definition. Check specifically for the tokens this revision removes: `--press-*`,
`--halo-live`, `--text-2xs`, `--text-xs`, `--text-sm`, `--text-base`, `--radius`.

---

## Issues, in dependency order

### 1. Page frame — app bar and footer band
**Spec** §6, §5 · **Frames** every frame · **Files** `shell/TopBar.tsx/.css`, `shell/StatusBand.tsx/.css`, `shell/PageFrame.tsx/.css`, `shell/Mark.tsx/.css`

One 60px app bar carrying mark, wordmark, view switch, command trigger, live count, account control.
One 40px footer band reading zone → what this page shows → build, undivided. Mark gains its 3px-radius
container and `--rule-3` quiet cells. View switch becomes a 10px-radius segmented control with a
`--raised` selected segment and no impression.

**Acceptance:** the bar and band are byte-identical across signed-out, sessions and operations; the
band no longer draws per-column cells; `body` still never scrolls; the mark is legible at 16px.

---

### 2. Controls
**Spec** §7, §3 · **Frames** 08, and every frame · **Files** `ui/Button.tsx`, `ui/Select.tsx/.css`, `ui/Chip.tsx/.css`, `ui/Dialog.css`

The five heights and no others. `--lift-*` replaces the letterpress state machine: hover raises the
background, active lowers it, disabled takes `--inset` with an `--ink-3` label, focus stacks the ring.
`Button` loses `reason` as a stacked-key prop and gains nothing in its place — the reason is now a
sibling `.control-reason` drawn once per row. Status chips become dot-plus-word pills.

**Acceptance:** every control measures one of the five heights; no two controls in the same row differ
in height; nothing translates on hover; a disabled control's label measures ≥4.5:1 against its own
fill; no component sets `box-shadow` from a literal.

**Note:** this issue touches every surface. Land it early and in its own PR — batching it with a layout
rewrite makes the diff unreviewable.

---

### 3. Session rail
**Spec** §12, §7 · **Frames** 01, 02, 04 · **Files** `sessions/SessionRail.tsx/.css`, `sessions/ArrangementToggle.tsx/.css`

320px. Header row with the count and the collapse handle; second band with the full-width primary key
and a row of counted filter pills plus the sort control. Rows become 12px-padded, 12px-radius targets
carrying dot, time at `--text-ui` 600, kernel in mono, and the last-activity line. Collapsed is a 56px
strip carrying the handle and the dot summary with counts. The arrangement toggle becomes a
List/Canvas segmented control in the rail header.

**Acceptance:** collapsing changes no session's state; the handle is where the panel was; `⌃← / ⌃→`
work and focus lands on the handle; an unrecognised stored value falls back to expanded; the activity
line never wraps.

---

### 4. Session header and the details drawer
**Spec** §12, §7 · **Frames** 02, 03 · **Files** `sessions/SessionHeader.tsx/.css`, `sessions/SessionActions.tsx/.css`, new `sessions/SessionDrawer.tsx/.css` (replaces `SessionDetails`), `Console.tsx/.css`

The header carries subject, status pill, kernel and plane, then one primary, one secondary and a `···`
overflow menu. The sheet becomes a 360px right drawer, closed by default, opened from the header's
`Details` key: facts as labelled rows, creator, reach, a copyable identifier, and End/Delete at its
foot.

**Acceptance:** `abilitiesFor()` remains the single source for enablement; every disabled control's
reason is drawn once per row; the terminal keeps its scrollback when the drawer opens and closes; the
drawer is an overlay below 75rem.

---

### 5. Failure reporting
**Spec** §12, §8 · **Frame** 04 · **Files** `Console.tsx/.css`

The 40px `--fail` band becomes a dismissible `.notice--fail` card at the head of the reading column,
carrying what failed, what was not changed, and the API code in identifier style. Cleared by every
action as it starts.

**Acceptance:** a stop returning 409 while the canvas holds twelve attached terminals detaches none of
them; the notice never replaces the surface; it carries `role="alert"`.

---

### 6. The conversation surface
**Spec** §18.1–18.5 · **Frames** 01, 02, 04 · **Files** `chat/MessageStream.tsx/.css`, `chat/ConversationSurface.css`, `chat/ToolCall.tsx/.css`, `chat/ApprovalCard.tsx/.css`, `chat/Composer.tsx/.css`, `chat/Thinking.tsx/.css`, `chat/Usage.tsx/.css`

The largest issue and the one the redesign exists for. A 760px centred reading column; the agent's
turns full width and unboxed at 16px/1.65 with an eyebrow; the owner's turns as a right-aligned block
on `--inset`; tool calls as 40px rows with glyph, kind, target, duration and status word; reasoning
collapsed to one key; the composer as a card carrying policy, interrupt and the primary send, with
usage beneath it; the approval card with the kernel's question as the largest type on the surface.

**Acceptance:** a turn drawn from history and a turn watched arriving are pixel-identical; the caret
belongs only to the message being typed; no colour is applied to agent content; the composer's refusal
states the reason and the approval's refusal says the agent stopped on purpose; nothing in the stream
is below 13px.

**Split it if it grows:** stream and turns first, then tool calls and reasoning, then composer and
usage, then the approval card. Four PRs is fine; one 2,000-line diff is not.

---

### 7. Waiting on an answer
**Spec** §11, §18.3 · **Frame** 04 · **Files** `sessions/availability.ts`, `chat/planeState.ts`, `sessions/lastActivity.ts`, `chat/Composer.tsx`

The live defect. A session parked on `requires_action` is `idle`, and today the composer says
*"This session is stopped. Revive it to carry on."* while an answerable approval sits above it.
`isWaitingForAnswer()` already exists and nothing reads it: thread it through so the rail line, the
header pill and the composer all say the session is waiting for its owner, and the composer offers the
answer rather than a revive.

**Acceptance:** a fixture parking a session on an approval renders "waiting" in all three places and
"stopped" in none; a genuinely idle session is unchanged; `abilitiesFor()` is still the only source of
enablement.

---

### 8. Canvas
**Spec** §13 · **Frame** 05 · **Files** `canvas/SessionCanvas.tsx/.css`, `canvas/CanvasNode.tsx`, `canvas/layout.ts`

Nodes stay 420×280 on a `--sunken` floor with 38px headers. Focus moves to hue plus `--focus-node`
plus the words `typing here`. **A conversation node draws a recap** — last agent message, what is
running, and `Open to reply` — instead of mounting `ConversationSurface`. Add the trailing fade and the
pan pill carrying the session count.

**Acceptance:** node geometry is unchanged from today, so no session re-wraps on merge; no node mounts
a composer; the 12-node budget and its stated truncation line still hold; arrow navigation still
clamps; nothing inside a node clips a control or a placeholder.

---

### 9. Operations
**Spec** §14 · **Frame** 06 · **Files** `operator/OperatorPage.tsx/.css`, `operator/RecordList.tsx`, `operator/views.tsx`

Page title with its one-line description, an underlined tablist at `--text-ui`, filters as
`.select` controls with the 200px minimum, and record rows inside one card on a 190px timestamp
column. The withheld running total becomes a `.notice--quiet` stating why.

**Acceptance:** each view still loads only on its own capability and only when opened; a view is never
drawn empty because of entitlement; the record list is newest-first regardless of arrival path; a
provider that could not be asked reads unknown with its reason.

---

### 10. Dialogs, menus and the command bar
**Spec** §7, §6 · **Frame** 08 · **Files** `ui/Dialog.tsx/.css`, `sessions/NewSessionDialog.tsx`, `profile/ProfileMenu.tsx/.css`, new `shell/CommandBar.tsx/.css`

One plate shell in three widths (520 / 700 / 1000) at `--radius-lg` with `--lift-plate`. The
new-session dialog's plane choice becomes two labelled cards rather than a select. The command bar
arrives here, reaching sessions, arrangements, new session and appearance — and nothing the spec does
not cover.

**Acceptance:** the focus trap, Escape, scrim click and focus return are written once; no dialog opens
over a dialog; destructive confirmation is inline; `⌘K` opens and closes and returns focus to the
trigger; the command bar lists nothing it cannot do.

---

### 11. Signed-out surface
**Spec** §15 · **Frame** 07 · **Files** `landing/IntroTerminal.tsx/.css`

Same app bar and footer band. Availability tag, 64px headline, lede, one `--control-h-xl` primary key,
beside a terminal panel with a 40px chrome strip. Timings and the reduced-motion equivalence are
unchanged.

**Acceptance:** the prompt is focused on first paint; a visitor typing at 300ms keeps every character;
the accessible transcript is present before any animation starts; the sign-in key appears only after a
character is typed.

---

### 12. Responsive
**Spec** §16 · **Files** `Console.css`, `styles/base.css`, `chat/*.css`

Four widths. Drawer as overlay below 75rem, rail collapsed below 75rem, rail as drawer below 60rem, bar
at 52px and targets at 44px below 30rem, reading column full width there. Canvas offered at 60rem and
above only.

**Acceptance:** at 390px every target measures ≥44px and no horizontal scroll appears outside a
terminal, a code block or the canvas; the surface is the last thing to lose width.

---

## Sequencing

```
PR1 spec ─→ PR2 fonts ─→ PR3 tokens ─┬─→ 1 page frame ─→ 3 rail ─→ 4 header + drawer ─→ 5 failures
                                     ├─→ 2 controls (parallel, touches everything)
                                     ├─→ 6 conversation ─→ 7 waiting on an answer
                                     ├─→ 8 canvas   (after 4)
                                     ├─→ 9 operations (after 1, 2)
                                     ├─→ 10 dialogs + ⌘K (after 2)
                                     ├─→ 11 landing (after 1, 2)
                                     └─→ 12 responsive (after 1, 3, 4)
```

Issues 2 and PR 2 touch every surface. Land them early and alone.

## What is not in this milestone

- Rewriting `api/`. Nothing here needs a new endpoint; where the interface withholds a total or
  refuses an action, it is reporting what the API already says.
- A design-token build step. The sheet is plain custom properties on purpose.
- Motion beyond the 140ms shadow and 200ms colour transitions, the waiting pulse, and the reported-step
  progress bar.
- Anything the command bar could grow into. It ships with the surfaces §6 names and no more.

---

# Part C — `Web_Style.md`

# Web Style

**This document is the single source of truth for all chrono-bot frontend design and style.**

No colour, spacing value, type scale step, radius, component pattern, or interaction rule may be
introduced in `web/` unless it appears here. If a design need is not covered, extend this document
first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

> **Filing note, delete on merge.** This revision replaces §2–§18 of the previous document. §1's
> information architecture is unchanged and is restated below for continuity. The existing changelog
> table is preserved verbatim — do not retype, reflow or edit any prior entry — and the entry for this
> revision is appended at the bottom, marked `2026-08-18 · Quiet instrument`.

---

## 1. What the frontend is

Unchanged. **Two planes onto one session, and the chrome needed to run several of them.**

| Plane | What it is |
|---|---|
| **terminal** | The kernel's own interface, entire and unmediated. The raw seat at the agent. |
| **conversation** | The structured seat: approvals as buttons, tool calls as structure, usage as a number, history that survives the screen. |

| View | What it is |
|---|---|
| **sessions** | The default, and where the work happens. A list, one surface, and what is known about it. |
| **operations** | Running the service. Drawn only for a caller holding an operator capability. |

Sessions has two **arrangements** of the same list — `list` and `canvas`. A plane is not a view and an
arrangement is not a view: both change what the middle column draws and nothing else. There are two
views, one page frame, and no navigation beyond them.

The four binding consequences survive this redesign untouched:

- **The surface owns whatever width is left.** It is the last to give up space: the details drawer
  closes first, then the rail collapses, then the rail leaves the layout.
- **One surface, many drivers.** A visual difference between the introduction and a live session that
  is not a difference in *content* is a defect.
- **Fidelity beats decoration inside the viewport.** Agent-emitted output is rendered faithfully;
  styling applies to the frame.
- **The conversation surface may call no route a third-party client could not call.**

---

## 2. Colour

Defined as custom properties on `:root`. Components reference tokens only — never a literal colour.

The ramp is **warm neutral**: every grey is tinted a few degrees toward the accent's hue family, so a
lime key reads as part of the page rather than as something dropped onto it. Dark and light are equal
expressions, and both are measured.

### Dark

| Token | Value | Use |
|---|---|---|
| `--ground` | `#101210` | page background, app bar, footer |
| `--surface` | `#171916` | the surface a session is drawn on |
| `--raised` | `#1B1E1A` | cards, composer, tool rows, plates |
| `--inset` | `#22261E` | fields, the user's own turn, tracks |
| `--sunken` | `#131512` | a terminal viewport, the canvas floor |
| `--ink` | `#F1F2EE` | primary text |
| `--ink-2` | `#A9AEA3` | secondary text |
| `--ink-3` | `#8A9082` | metadata, labels, hints |
| `--rule` | `#23261F` | hairlines |
| `--rule-2` | `#2A2E26` | control borders |
| `--rule-3` | `#3A3F36` | the quiet cells of the mark |
| `--accent` | `#C6F24E` | interactive text, the primary fill |
| `--on-accent` | `#111310` | text on the accent fill |

### Light

| Token | Value |
|---|---|
| `--ground` | `#F6F6F2` |
| `--surface` | `#FFFFFF` |
| `--raised` | `#F1F2EB` |
| `--inset` | `#F1F2EB` |
| `--sunken` | `#EDEEE6` |
| `--ink` | `#141613` |
| `--ink-2` | `#55584F` |
| `--ink-3` | `#64685E` |
| `--rule` | `#E3E4DB` |
| `--rule-2` | `#D8DACF` |
| `--rule-3` | `#C7CABD` |
| `--accent` | `#46620B` |
| `--accent-fill` | `#C6F24E` |
| `--on-accent` | `#141613` |

**One accent, and the two themes share its fill and differ in its ink** — a colour bright enough to
read on near-black cannot carry text on paper. `#C6F24E` marks what is live and what to press, and
nothing else.

### Semantic

| Token | Light | Dark | Meaning |
|---|---|---|---|
| `--live` | `#46620B` | `#C6F24E` | ready, running, healthy |
| `--wait` | `#8A5A00` | `#E9B44C` | starting, stopped, rotating, waiting on you |
| `--fail` | `#A63232` | `#F58585` | failed, refused, invalid |
| `--ended` | `#64685E` | `#8A9082` | ended, unknown, retired |

Each has a `*-wash` companion at 7–10% alpha, which is the only permitted background for a status
pill. A semantic colour never carries meaning alone: the state word is always drawn beside it.

### Measured contrast

Worst case per theme, computed per WCAG 2.1 against the surface the token actually sits on. Body text
requires 4.5:1; no pair falls below it in either theme.

| Theme | Pair | Ratio |
|---|---|---|
| Dark | `--ink-3` on `--inset` `#22261E` | 4.69:1 |
| Dark | `--ink-3` on `--surface` | 5.39:1 |
| Dark | `--ink-2` on `--surface` | 7.81:1 |
| Dark | `--wait` on `--surface` | 9.35:1 |
| Dark | `--fail` on `--surface` | 7.22:1 |
| Light | `--ink-3` on `--inset` | 5.06:1 |
| Light | `--ink-3` on `--ground` | 5.26:1 |
| Light | `--accent` on `--surface` | 6.97:1 |
| Light | `--wait` on `--surface` | 5.93:1 |
| Light | `--fail` on `--surface` | 6.73:1 |

`--ink-3` is the binding constraint in both themes: it carries 11–12px metadata, which is not large
text, so the full 4.5:1 applies. **Do not lighten it in dark or darken it in light.** A new colour
requires a changelog entry and a recomputed row.

**A disabled control's label is measured too.** It sits on `--inset` and takes `--ink-3`, which is
4.69:1 in dark and 5.06:1 in light. A disabled primary action nobody can read is the one control where
"it looks disabled" costs the most.

### Rules

- Every colour is defined on bare `:root` first. A colour whose only definition lives inside a media
  query or `[data-theme]` block is a bug.
- `body` always sets an explicit `background` from a token.
- No colour is ever applied to agent-emitted content (§18.1).

---

## 3. Depth

**Two levels and no others.** A thing resting on the page, and a thing floating over it.

| Token | Dark | Light | Use |
|---|---|---|---|
| `--lift-rest` | `0 1px 2px rgb(0 0 0 / 0.35)` | `0 1px 2px rgb(20 22 19 / 0.06)` | the composer, a selected row, a card that matters |
| `--lift-plate` | `0 12px 32px rgb(0 0 0 / 0.5)` | `0 12px 32px rgb(20 22 19 / 0.12)` | dialogs, menus, the command bar |
| `--focus-ring` | `0 0 0 2px var(--ground), 0 0 0 4px var(--accent)` | same | every focusable thing |
| `--focus-node` | `0 0 0 3px rgb(198 242 78 / 0.12)` | same | the canvas node holding the keys |
| `--edge-selected` | `inset 2px 0 0 var(--accent)` | same | the selected row's leading edge |

**These are the only shadow values permitted.** An inline `box-shadow` string in a component is a bug.

**The letterpress contract is retired.** Hard-offset impressions, hover translating into the
impression, active sinking flush: all removed. They gave every control a 4px visual weight that a
dense interface could not carry, and they demanded 2px corners.

### The state contract

| State | Treatment |
|---|---|
| rest | its own background and hairline |
| hover | background raises one step in the ramp; the border firms to `--rule-2` |
| active | background lowers one step |
| disabled | `--inset` fill, `--rule` border, `--ink-3` label, and a reason on the surface |
| focus | `--focus-ring`, stacked over whatever the state holds |

**Nothing moves on hover.** No translate, no scale, no lift toward the cursor. The only glow the
product has is `--focus-node`, and no glow is ever applied to a terminal viewport or to a character a
kernel emitted.

### Radius

Five steps, and a thing keeps the same one everywhere it appears.

| Token | Value | Use |
|---|---|---|
| `--radius-sm` | 8px | small controls, menu items |
| `--radius` | 10px | buttons, fields, tool rows |
| `--radius-md` | 12px | cards, rows, canvas nodes, code blocks |
| `--radius-lg` | 14px | panels, plates, the composer, a page frame |
| `--radius-pill` | 999px | status pills, filters, counts, the account control |

---

## 4. Typography

| Role | Stack |
|---|---|
| Interface — headings, labels, prose, controls | `'Figtree', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif` |
| Data — terminal, times, identifiers, metadata | `'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, Consolas, monospace` |

Both faces are **self-hosted** under the SIL OFL in `web/src/assets/fonts/`, with `font-display: swap`
and a system fallback. Nothing is fetched from a third party: a font request to someone else's host
can be logged, blocked or slow. Geist and Geist Mono are retired with the last reference to them.

### Scale

| Token | Size | Use |
|---|---|---|
| `--text-label` | 11px | mono, uppercase, tracked `0.08em` — every eyebrow and band label |
| `--text-meta` | 12px | metadata, timestamps, identifiers |
| `--text-note` | 13px | secondary prose, hints, reasons, quiet controls |
| `--text-ui` | 14px | controls, menu items, rail subjects |
| `--text-body` | 15px | interface body, the owner's own turn |
| `--text-read` | 16px | **the agent's words, and nothing else** |
| `--text-title` | 17px | dialog and plate titles |
| `--text-subject` | 20px | the session a surface is showing |
| `--text-page` | 24px | a page title |
| `--text-display` | 64px | the landing headline only |

Line height: `1.15` display, `1.55` interface, `1.65` the conversation and the terminal. Headings take
`text-wrap: balance`, prose `text-wrap: pretty`, any column of digits `font-variant-numeric:
tabular-nums`. Weights are 400, 500 and 600 only.

**Nothing an owner reads at length is below 15px, and the agent's own words are 16px.** The previous
revision set the conversation at 14px and its metadata at 10–11px, which is what "squeezed" meant.

**Terminal type keeps its own metrics:** 13.5px, line height 1.65, letter spacing 0. Never track a
terminal.

---

## 5. Layout

4px rhythm. Permitted spacing: `4, 8, 12, 16, 20, 24, 32, 48, 64`, with 2px steps allowed **only**
inside a control's own padding, where optical alignment beats the grid.

**The page does not scroll.** The app fills the viewport; the terminal owns its scrollback, and a
column, a drawer or a canvas owns its own overflow.

| Token | Value |
|---|---|
| `--bar-height` | 60px (52px below 30rem) |
| `--band-head` | 56px |
| `--band-foot` | 40px |
| `--page-gutter` | 20px |
| `--rail-width` | 320px |
| `--rail-collapsed` | 56px |
| `--drawer-width` | 360px |
| `--read-column` | 760px |

Binding rules:

- A band's height is **set on the band**, never accumulated from padding.
- **One hairline per boundary.** Two adjacent elements may not both draw a border on the shared edge;
  the container draws it, or the child does, never both.
- **The reading column is centred and capped at 760px**, whatever the surface's width. A conversation
  set to the full width of a 1440px window is a conversation nobody can track a line in.
- A column needing more chrome adds a second band beneath the first. It never grows its header.

---

## 6. Page frame

One app bar and one footer band, on every page, so the window's top and bottom edges never move as
somebody switches view.

The **app bar** (60px) carries, in order: the mark and wordmark, the view switch, the command trigger,
the live count, and the account control. It is not navigation and must never become it.

The **command trigger** is a real control: a 36px field-shaped button reading *"Find a session, or
type a command"* with a `⌘K` hint, opening the command bar plate (§7). It is no longer drawn disabled —
a permanently dead field in the middle of the bar is the single loudest "unfinished" signal the
product had. **The plate ships with the surfaces it can reach and no more:** sessions by time and
last-activity line, the two arrangements, new session, and the appearance controls. Anything else it
would offer belongs to a section of this document that does not exist yet.

The **footer band** (40px) reads: the display zone, then what this page is showing, then the build.
It is one band across the whole window; it no longer divides into per-column cells, because the
columns no longer draw their own footers.

---

## 7. Controls

| Token | Value | Use |
|---|---|---|
| `--control-h-sm` | 30px | inside a row, a node header, a filter row |
| `--control-h` | 32px | anything in a band or a dialog footer |
| `--control-h-md` | 36px | fields, the command trigger, a drawer's actions |
| `--control-h-lg` | 40px | a panel's one full-width primary action |
| `--control-h-xl` | 46px | the landing's opening action |
| `--chip-h` | 26px | status pills, filters, tags |
| `--hit` | 32px | minimum target (44px below 30rem) |

**One primary action per surface**, drawn as the accent fill, spent on the action that surface exists
for. Second-rank actions are bordered, third-rank are borderless, and **everything beyond the third
goes into the overflow menu** behind a `···` key. A header carrying four equally weighted keys makes
the reader choose before they can read, which is what the session header did.

**Controls are disabled, never removed**, and the reason is stated **once per row**, beside the
controls it explains, at `--text-note` in `--ink-3` — never in a tooltip, and never repeated on each
key. The stacked two-line key is retired: it made every disabled control taller than its enabled
neighbour.

Destructive actions take `--fail` and confirm **inline**, in place of the controls that raised them.
Never a second dialog over a dialog.

### Dropdowns

An instrument closed and a plate open. 32px tall, the caret **flips** rather than rotating, a filter
holds a 200px minimum so its row does not resize as the value changes, and the selected option is
marked by weight **and** a tick. Choosing a **value** is a select; choosing an **action** is a menu;
a select never runs anything when it opens.

### Cards side by side

A row of peer cards is a **grid**, never a flex row of hand-set widths. A card whose text overruns
truncates or scrolls inside itself rather than growing and leaving its neighbours short.

### Collapsing a panel

A collapsed panel becomes a **handle, never nothing**: 56px of strip carrying the handle and the
status-dot summary with counts. The glyph points where the panel will go and reverses when collapsed.
`⌃← / ⌃→` for a column, and focus moves to the handle so nobody is stranded on a control that just
left. The choice persists per browser and is validated on read — web storage is untrusted input.

---

## 8. Interaction

- Every interactive element has a visible focus state: `--focus-ring`, stacked over whatever the state
  carries. Never removed. **One documented variant:** the terminal prompt row takes
  `--focus-prompt` on its leading edge, because a ring around a full-width borderless field boxes in
  the whole prompt line and breaks the surface's illusion.
- Transitions are 140ms on `box-shadow`, 200ms on colour and background, each wrapped in
  `@media (prefers-reduced-motion: no-preference)`. **Nothing transitions position or size.**
- Loading is a skeleton of the shape that is coming, or an inline indicator. Never a full-page spinner.
- Waiting is a pulse on a dot, or a bar reflecting steps the control plane actually reported. A bar
  never advances on a timer.
- Errors state what went wrong and what to do about it. No apologies, no raw codes without a human
  sentence beside them.

---

## 9. Terminal

Unchanged, and the engine carries the hardening: **OSC 52 disabled**, no escape sequence setting the
tab title or triggering a host action, capped scrollback, OSC 8 hyperlinks validated before being made
clickable. The 16-slot ANSI palette is derived from the tokens and is authoritative; agent output is
rendered through it and not otherwise restyled. Selectable terminal themes remain vendored as data,
filtered by the current appearance, and scoped to the terminal canvas.

The viewport sits on `--sunken` inside a `--radius-md` frame with a `--rule` hairline, at
`--term-size`. Inside a canvas node it drops its own border: the node frame is that border.

---

## 10. Accessibility

- Text contrast meets WCAG AA in **both** themes. Measured ratios are recorded in §2.
- Semantic HTML before ARIA. Landmarks on every page.
- Every view is fully keyboard operable, including the terminal, which must be escapable by keyboard
  alone.
- Never convey state through colour alone.
- **The animated introduction is never the only route to its information.** The viewport is exposed to
  assistive technology as a static, complete transcript.
- Focus on load rests on the prompt.
- Minimum target 32px, 44px below 30rem.

---

## 11. Session states and what they allow

Unchanged, and one rule still governs all of it — whether the surface accepts input, whether a control
is enabled, and whether the session appears in the list. `abilitiesFor()` remains the single source.

| State | Shown as | Terminal plane | Conversation plane | Available |
|---|---|---|---|---|
| provisioning | starting | progress, no input | progress, composer refuses | end |
| ready / running | ready / running | live | live | reconnect, end, delete |
| rotating | **updating** | progress, no input | progress, composer refuses, and says why | nothing |
| idle | stopped | history, read-only | history, read-only — **unless it is waiting on an approval** | **revive**, delete |
| terminated | ended | history, read-only | history, read-only | delete |

**A session that cannot be typed into always says why**, in the owner's terms, on the surface showing
it. `idle`, `recovering` and `unknown` are **not failures** and none of them takes `--fail`.

**The wording of a refusal belongs to the reason it exists, not to the state's name.** This is the one
substantive change to this section, and it fixes a live defect: a session parked on `requires_action`
is `idle`, so the composer told its owner *"This session is stopped. Revive it to carry on."* while an
approval it could answer sat directly above. A session waiting on an answer says it is **waiting for
you**, in the rail, in the header and at the composer, and offers the answer rather than a revive.

Ended sessions leave the list, except when they failed. **Revive is the primary action wherever it is
offered.**

---

## 12. Sessions — the list arrangement

Two columns and a drawer: the session list, the session's surface, and — when asked for — what is
known about it.

**The rail is 320px** and lists sessions by when they started, in Singapore time, newest first and
reversible, with the date shown only once a session is no longer from today. A row carries a status
dot, the start time at `--text-ui` weight 600, the kernel in mono `--text-label`, and **one line
saying what the session last did**, which never wraps. The selected row takes `--raised`,
`--edge-selected` and `--lift-rest`.

The rail's second band carries the one full-width primary key — **New session** — and a row of filter
pills carrying counts (`All 5`, `Live 2`, `Stopped 3`) and the sort control.

**What is known about the session is a right drawer, 360px, closed by default.** It is reference
material, and the previous revision's 412px bottom sheet took that height from the surface it was
describing — on a 900px window, a third of the terminal. As a drawer it takes width from a surface
that has 1000px of it and none of the height. It carries the session's facts in labelled rows, who
started it, what it reaches, its identifier as a copyable control, and — at its foot, away from
everything reversible — **End session** and **Delete**.

**A session's lifecycle controls live in the session header**: one primary, one secondary, and the
overflow menu (§7). The header also carries the subject, the status pill, and the kernel and plane in
mono.

**A failed action is reported inside the column, above the surface and never in place of it.** It is a
`--fail` notice card at the head of the reading column, carrying what failed, what was not changed,
and the API's own code in identifier style, with a dismiss. Placement is the substantive part: a
failure belongs to one request about one session, and the surface beneath it may hold a dozen live
terminals. **Nothing that reports a failure may destroy work that did not fail.** Every action clears
it as it starts.

---

## 13. Sessions — the canvas arrangement

Every listed session is a framed node on one pannable surface. **The canvas replaces the middle column
and nothing else.** One selection, two arrangements: the session the rail marks active and the node the
canvas focuses are the same fact.

### Node framing

| Property | Value |
|---|---|
| Size | `--canvas-node-w` 420px × `--canvas-node-h` 280px |
| Gap and surface padding | `--space-5` |
| Background | `--surface` on a `--sunken` floor |
| Border | 1px `--rule`, `--radius-md` |
| Header | 38px, 1px `--rule` underline: dot, start time, kernel and state in mono |

**Fixed size, deliberately.** A node's size *is* the geometry its runtime is told to draw at: a
420×280 node asks for roughly 47 columns, and a node that changed width as siblings came and went
would re-wrap everything the agent has drawn. Choosing the canvas therefore resizes every attached
session once, and that does not undo.

### What a node draws

| Plane and state | The node draws |
|---|---|
| terminal, interactive | its terminal, live |
| terminal, read-only | its terminal, live, and the word `read-only` in its header |
| **conversation** | **a recap: the last thing the agent said, what is running, and one key reading `Open to reply`** |
| rotating | the sentence, and a bar reflecting reported steps |
| failed | what failed, in the kernel's words, and `Start again` |
| past the budget | why it is unattached, and `Attach` |

**The conversation recap is the substantive change here.** The previous revision drew the whole
conversation surface inside the node — stream, composer, usage band, all of it — inside 420×280,
where the composer's placeholder clipped mid-word and the send key had 40px of room. A conversation
needs a reading column (§5) and a canvas node cannot give it one, so the node shows what a glance
wants and hands the reply to the surface that can hold it.

### Focus, selection, and the cost

| State | Treatment |
|---|---|
| Resting | 1px `--rule`, `--ink-2` header |
| Selection — pointer over the node | 1px `--rule-2`. Border weight only. |
| Focus — this node takes the keys | 1px `--accent`, `--focus-node`, `--ink` header, `aria-pressed` on its handle, and the words `typing here` |

**Focus is never a second opinion on §11.** The canvas is one tab stop; arrows move by one and by a
row, clamped and never wrapping, and are not intercepted inside a terminal. **Unfocused terminals are
never dimmed, tinted, greyed or covered.**

**At most 12 nodes are attached at once**, and truncation is never silent: the footer band states how
many are unattached and why. **The surface overflows on purpose and says so** — a fade at the trailing
edge and a pill reading *"drag or → to pan"* with the session count. A grid that runs past the
viewport with nothing marking the cut reads as a clipped layout rather than a canvas.

---

## 14. Operations

**A page, not a dialog**, and the only surface that shows many rows at once. The view is absent unless
the control plane says the caller holds a capability it can act on — presentation, never protection.
Each of its four views is offered on its own capability, and a view is never drawn empty because of
entitlement.

The page carries a `--text-page` title with a one-line description, then the four views as an
underlined tablist at `--text-ui` — selected by weight, `--accent` ink, a 2px `--accent` bottom border
**and** `aria-selected`.

### Record list

Rows inside one `--radius-md` card, not a table.

- A leading tabular-nums timestamp at 190px, a `--text-body` primary line, `--text-meta` mono
  metadata beneath.
- Hairline separators, no zebra striping, no hover highlight, no row-level click target. Nothing on
  this page navigates.
- **Newest first, always.** Filters narrow without ever changing the order.
- An empty list says what would have been there, in a sentence.

**A person is their name, then their email — never their subject identifier.** Where the directory
knows nothing, the identifier is shown in identifier style. An action with no established identity
reads "Not signed in".

**Unknown is not empty.** A provider that could not be asked renders as unknown carrying its reason,
and the running total is withheld entirely while any provider is unobservable — drawn as a quiet
notice stating why, not as an absence.

---

## 15. The signed-out surface

A statement, and a terminal that already works. Timings are unchanged:

| Property | Value |
|---|---|
| Machine lines | printed whole, 75ms stagger |
| Prose | typed, 20ms per character |
| Pauses | 180ms after `.` `?` `!`, 90ms after `,` and `—`, 140ms at a line break |
| Caret blink | 530ms |
| **Time to interactive** | **0ms; introduction complete under 3.2s, hard cap 4s** |

The prompt is a real `<input>`, mounted and focused from the first frame. The animation is skippable
by any keypress or click. The sign-in affordance appears once the visitor has typed a character. Their
text survives the round trip and becomes the first turn of their first session. Under
`prefers-reduced-motion: reduce` all text is present instantly, with no typing, no blink and no hint.

The layout is a statement column beside a terminal panel: an availability tag, the headline at
`--text-display`, the lede at `--text-body` + 4, one `--control-h-xl` primary key, and beside it the
panel with a 40px chrome strip and the introduction inside. The page carries the same app bar and
footer band as every other page.

---

## 16. Responsive

The number of columns changes; the bands never do.

| Width | Layout |
|---|---|
| ≥ 75rem | Rail, surface, and the drawer on request. Nothing collapses on its own. |
| 60–75rem | The rail arrives collapsed to its 56px strip; the drawer becomes an overlay. |
| < 60rem | The rail leaves the layout and becomes a drawer over it. The surface keeps the full width. |
| < 30rem | The app bar drops to 52px and gives the switch and the command trigger to the drawer. Every target grows to 44px. The reading column is the full width. The footer keeps the zone and the count and drops the build. |

**The canvas arrangement is offered at 60rem and above only**, disabled with its reason below that
rather than removed.

---

## 17. The mark

Two-by-two, one cell lit, inside a 3px-radius square. Built from four squares, so it needs no font, no
SVG and no asset, survives a 16px favicon, and prints in one colour. The gap scales with the mark:
8px at 96, 4px at 40, 2px at 18, 1px at 16. The quiet cells take `--rule-3` and the lit cell takes
`--accent`.

The wordmark is Figtree 600 at `--track-tight`, lowercase. `chrono-bot` in mono stays for anything
naming the repository or the API.

---

## 18. The conversation surface

The plane a session can be drawn on beside the terminal. **Anything drawn here that could equally have
been drawn as scrollback has thrown away the thing this plane was built to obtain.**

### 18.1 The message stream

**A reading column, 760px, centred**, with `--space-8` between turns. The two roles are told apart by
shape as well as ink, which is the change from the previous revision's undifferentiated bands:

- **The agent's turn is full width and unboxed.** An eyebrow — a `--live` dot, then `CHRONOBOT ·
  <kernel>` in mono `--text-label` uppercase — then its words at `--text-read` in `--ink` at
  `--leading-read`. The agent's words are the ones being read, and they get the whole column.
- **The owner's turn is a block, right-aligned**, at `--text-body` in `--ink` on `--inset`, max 78% of
  the column, `--radius-lg` with the trailing corner at 4px, with `You · HH:MM` above it. It is
  short, it is already known, and giving it a shape means the eye finds where a request starts without
  reading a label.

No avatars. No alternating alignment beyond that one asymmetry. No bubble on the agent's side.

**Streaming versus committed.** Text arriving is drawn in the same ink and size as text that has
settled; the only difference is the caret — a 9×17px `--accent` block, `--radius` 1px — which belongs
to the message being typed and to nothing else. **The message being typed is not a stream entry**: it
is drawn after the last entry and *replaced* by the committed message, which is what makes the
equivalence rule hold by construction.

**The equivalence rule.** A turn drawn from history and a turn watched arriving must produce the same
result. Nothing may be rendered from a streaming frame that is not also derivable from the durable log.

**Before the first thing arrives**, the surface draws the §8 pulse with a word, removed the moment
anything arrives, claiming no progress.

**Where the eye rests when output is long.** Pinned to the newest turn while the newest turn is the one
growing; unpinned the moment somebody scrolls up, and never scrolling back on its own. A quiet
`--control-h-sm` key at the foot of the column says how many turns they are behind.

**Fidelity.** Code blocks take `--font-mono` at `--term-size` − 0.5 on `--sunken` inside `--radius-md`
with a `--rule` hairline, scrolling horizontally inside themselves. Inline code takes `--font-mono` on
`--inset`. Lists, headings and emphasis take the scale §4 defines. **No colour is applied to agent
content, ever** — not syntax highlighting, not a tint per role, not a semantic colour on a word the
agent happened to write.

### 18.2 Tool calls

**Structure, not prose**, and the section that pays for the plane. A 40px row on `--raised` inside
`--radius`, in a stack directly under the turn that made the calls:

| Part | Treatment |
|---|---|
| status glyph | `✓` in `--live` finished · `×` in `--fail` failed · a pulsing `--wait` dot running |
| kind | the kernel's own word, verbatim, mono `--text-note` in `--ink` |
| target | the kernel's own word, verbatim, mono `--text-meta` in `--ink-2`, truncating — it is the part that runs long |
| duration | mono `--text-label` in `--ink-3`, once the call has finished |
| status word | for a failed or running call, beside the caret, because §2 forbids colour alone |

A failed row takes a `--fail`-tinted border. **Disclosure, not truncation:** expanded output takes the
code-block treatment and scrolls inside itself, capped at 120px in the stack; nothing is silently
shortened.

**The collapsed row carries enough to identify the call without opening it.** An unrecognised kind is
rendered as it arrived rather than mapped onto a set this document invented.

### 18.3 Approvals

The highest-stakes control in the product, and it reads as one. It sits in the stream where the turn
stopped — not above it, not pinned, not floating — on `--raised` inside `--radius-lg` with a 1px
`--accent` border and `--lift-rest`.

It carries, in this order:

1. An eyebrow: `approval · <the kernel's own reason>` in mono `--text-label` on `--accent-wash`, and
   the time it was asked.
2. **What is being asked**, as the kernel worded it, verbatim, at `--text-title` + 1 in `--ink` — the
   largest type on the surface, because it is the sentence the decision turns on. A command inside it
   takes inline code.
3. **The answers the kernel offered**, as `--control-h-md` + 2 keys in a row, **rendered from what
   arrived, never from a hardcoded pair.** The affirmative answer is the surface's one primary key;
   an answer that abandons the turn is never collapsed into one that refuses this action.
4. Beside them, in `--ink-3`: *"Your other sessions keep working."*

**It is not a modal, and this is binding.** A session can sit on an approval while its owner reads
something else. **It survives a reconnect**, because approvals ride the durable event stream, and is
drawn from history like everything else.

**A session waiting on one says so everywhere it appears**: `--wait` dot and *waiting for your answer*
in the rail, *Waiting for you* as the header's status pill, and at the composer the sentence §11
requires — *the agent stopped here on purpose* — rather than a claim that the session is stopped.

### 18.4 The composer

A card at the foot of the reading column, on `--raised` inside `--radius-lg` with `--lift-rest`: the
field, then a row carrying what the session's own policy means, `Interrupt` while a turn is in flight,
and the primary `Send` with its `↵` hint. The field grows to six lines and then scrolls inside itself.
Enter sends; Shift+Enter opens a line, stated once beneath the card.

**What it does while a turn is in flight is not a style choice.** It reads the session's
`concurrent_input` policy and behaves accordingly:

| Policy | The composer |
|---|---|
| `reject` | accepts text, disables the send key, and says a turn is in progress |
| `enqueue` | accepts and sends; says the turn will follow the one running |
| `interrupt` | accepts and sends; says sending will stop the turn in progress |
| `rollback` | accepts and sends; says sending will discard the turn in progress |

The sentence is on the surface beside the key, never in a tooltip. **Steering shows no affordance at
all where it was not granted** — a control that can never work is not one that is currently
unavailable — and where it was, it is stated as a tag on the composer's own row.

**A composer on a session that cannot take input refuses, and says why**, drawn as a dashed-border
card carrying the reason and what to do instead. It is never merely inert.

### 18.5 Usage

Beneath the composer, outside its card, in mono `--text-label`: the token count with tabular figures
and the cost. It updates as the session reports it and is **never estimated**.

**Absence is a fact, not a zero.** A session not granted `usage` reads *"Usage not reported for this
session"*. A session moved to the terminal plane drops the figure rather than leaving the last one it
saw on screen.

### 18.6 Plane, and changing it

A session's plane is decided when it is created and is a property of the session. The affordance is a
`--control-h-sm` key in the overflow menu naming the plane it moves to, **absent — not disabled — when
the kernel does not advertise the other plane**. It confirms inline, stating the two costs that are
not obvious: the turn in flight is lost, and the session is briefly not interactive.

**While it runs, this is `rotating`** and §11 applies without exception.

**Afterwards, the boundary is drawn**: a labelled rule across the reading column at `rotated_at` — the
label in `--wait` mono `--text-label`, a hairline fading out beside it, and the sentence *"Anything
after that moment did not travel."* A gap somebody can see is a gap they can recognise.

### 18.7 What the conversation surface does not do

Unchanged. It renders no terminal — not a small one, not an embedded one, not a fallback one. It offers
no operation AG-UI does not project. Where it withholds something it is reporting what the API said,
and every refusal names the capability it lacks on screen. It never renders a credential.

| Wanted | On this plane | Where it lives |
|---|---|---|
| Resize the agent's terminal | absent — a geometry is a terminal fact | switch the session's plane (§18.6) |
| Read the rendered screen | absent — a screen is a terminal fact | switch the session's plane (§18.6) |
| Type into a full-screen TUI | absent — this surface renders no terminal | switch the session's plane (§18.6) |
| Interrupt a turn | where the kernel offers it; named when it does not | — |
| Steer mid-turn | under the session's own policy; no affordance where it was not granted | — |
| Read what it has spent | where the kernel reports it; "not reported" otherwise (§18.5) | — |

### 18.8 What the agent is working through

**Liveness, and never in the record.** Removed when the turn ends; a reload shows none of it.

**Collapsed by default** to one `--radius-pill` key reading *"Thought for 4s · not kept after this
turn"*, opening to a height-capped band on `--raised` pinned to the newest. The previous revision drew
it as an always-open band, which put the least durable content on the surface at the same weight as
the conversation. The words that make it safe to lose stay on the control whether it is open or not.

**No colour, ever**, and it is not announced: the band changes many times a second, and a live region
there would make the surface unusable with a screen reader.

**A kernel that does not reason draws nothing at all** — not an empty band, and not an empty key.

### 18.9 A run that failed

Drawn in the stream at the point it stopped, as a `--fail` notice carrying: what failed in the
kernel's own words, **the ability it lacked, named, and only where the failure was a refusal**, and
the stop reason as the API spells it in identifier style. The word carries the meaning and the ink
reinforces it.

**A session being carried onto another runtime is not a failure**, and the two arrive on the same
frame. What distinguishes them is stated by the API rather than guessed from wording; where the
distinction is absent the frame is drawn as a failure, that being the safer of the two to be wrong
about. A new run clears it.

---

## 19. What this revision did not change

Stated so nothing here is "fixed" by somebody who assumed a redesign touches everything.

- **§1's information architecture.** Two views, two planes, two arrangements, one page frame, no
  navigation, no router, no URL.
- **Every capability rule.** A control that can never work is absent; one that is currently
  unavailable is disabled and says why. Absence is a fact, not a zero. A refusal names what is missing.
- **The equivalence rule** (§18.1) and the fidelity rule (§1).
- **Terminal hardening** (§9) and the canvas node's fixed geometry (§13).
- **The landing's timings** (§15) and the reduced-motion equivalences throughout.
- **`abilitiesFor()` as the single source of enablement**, and the session-state table's meanings.
- **Singapore time**, stated rather than assumed, and newest-first ordering everywhere.

---

## Changelog

Append-only. Never edit or delete an entry.

<!-- FILING NOTE: the existing changelog table, from `2026-08-12 | Initial version.` through the last
     `2026-08-18` entry, is preserved here verbatim. Do not retype it, do not reflow it, and do not
     edit any row. Append the row below as the newest entry. -->

| Date | Change |
|---|---|
| 2026-08-18 | Quiet instrument: full redesign of the frontend's visual language, replacing the Ground control letterpress system of 2026-08-17. Kept the accent and every rule in §19, changed everything that made the interface hard to read. Replaced the neutral ramp with warm near-neutrals in five steps per theme (ground/surface/raised/inset/sunken), recording worst-case measured contrast per theme — dark `--ink-3` 4.69:1 on `--inset`, light `--ink-3` 5.06:1 on `--inset` — and set `--ink-3` to `#8A9082` dark and `#64685E` light so 11–12px metadata clears AA on every surface it sits on, including a disabled control's own fill, which the previous revision left at 2.5:1. Retired letterpress depth for two levels and no others — one for a thing resting on the page, one for a thing floating over it — with hover raising the background a step rather than translating into an impression, and nothing in the interface moving on hover. Replaced the single 2px radius with five steps from 8px to a pill, since a hard offset shadow was the only reason a sharp corner was required. Adopted Figtree and JetBrains Mono, self-hosted under the OFL, and retired Geist and Geist Pixel; raised the reading sizes so the agent's own words are 16px at 1.65 and nothing read at length is below 15px, which is what "squeezed" meant. Capped the conversation at a 760px centred reading column, gave the owner's turns a right-aligned block on `--inset` and the agent's turns the full column unboxed, so the two roles are told apart by shape as well as ink. Moved what is known about a session from a 412px bottom sheet to a 360px right drawer, because the sheet took its height from the surface it was describing. Reduced the session header to one primary, one secondary and an overflow menu, and moved End and Delete to the foot of the drawer. Made the command trigger a working `⌘K` plate over the surfaces it can already reach, rather than a disabled field in the middle of the bar. Collapsed tool calls to 40px rows carrying a status glyph, the kernel's own kind and target, a duration and a status word, and collapsed reasoning to one key stating that it is not kept. Replaced the conversation surface drawn inside a 420×280 canvas node — where the composer clipped mid-placeholder — with a recap of the last thing the agent said and a key that opens the surface which can hold a reading column, and gave the canvas an explicit trailing fade and pan affordance so a viewport narrower than the grid reads as pannable rather than clipped. Fixed one live defect in §11's wording rather than its rules: a session parked on `requires_action` is `idle`, and told its owner it was stopped while an approval it could answer sat above the composer; it now says it is waiting for them, in the rail, the header and the composer. Fixed the double hairline where a panel and its first band both drew a border on the shared edge, and added §5's rule that one boundary draws one hairline. |

---

# Part D — `tokens.css`

Destination: `web/src/styles/tokens.css`.

```css
/*
 * Design tokens — docs/Web_Style.md is the single source of truth.
 * Every value here is transcribed from that document. Never introduce a colour,
 * shadow, spacing value, radius or type step that does not appear there.
 *
 * This file is custom properties and nothing else. Component rules live in
 * components.css or in the component's own stylesheet, so checking §2 against
 * the token layer stays a single-file review.
 *
 * Light is defined on bare :root first, per §2: a colour whose only definition
 * lives inside a media query or [data-theme] block is a bug, because a token
 * that fails to resolve under an unexpected combination of theme settings
 * silently renders as `unset`.
 */

:root {
  /* Surfaces — §2. Warm neutrals: the ramp is tinted toward the accent's hue
     family so a lime key reads as part of the page rather than dropped on it. */
  --ground: #f6f6f2;
  --surface: #ffffff;
  --raised: #f1f2eb;
  --inset: #f1f2eb;
  --sunken: #edeee6;

  /* Text — §2. --ink-3 is the binding constraint: it carries 11–12px metadata,
     which is not large text, so the full 4.5:1 applies. Do not lighten it. */
  --ink: #141613;
  --ink-2: #55584f;
  --ink-3: #64685e;

  /* Borders — §2 */
  --rule: #e3e4db;
  --rule-2: #d8dacf;
  --rule-3: #c7cabd;

  /* One accent. Light and dark share the fill and differ in the ink, because a
     colour bright enough to read on near-black cannot carry text on paper. */
  --accent: #46620b;
  --accent-fill: #c6f24e;
  --accent-wash: rgb(70 98 11 / 0.08);
  --on-accent: #141613;

  /* Semantic state — §2. Never used decoratively, never colour alone. */
  --live: #46620b;
  --wait: #8a5a00;
  --fail: #a63232;
  --ended: #64685e;
  --live-wash: rgb(70 98 11 / 0.08);
  --wait-wash: rgb(138 90 0 / 0.09);
  --fail-wash: rgb(166 50 50 / 0.08);

  /* The scrim behind a plate. Not a new colour: --ink at 40%. */
  --scrim: rgb(20 22 19 / 0.40);

  /*
   * Depth — §3. Two levels and no others: one for a thing resting on the page,
   * one for a thing floating over it. The letterpress contract is retired; an
   * inline box-shadow string in a component is still a bug.
   */
  --lift-rest: 0 1px 2px rgb(20 22 19 / 0.06);
  --lift-plate: 0 12px 32px rgb(20 22 19 / 0.12);
  --lift-none: none;
  --focus-ring: 0 0 0 2px var(--ground), 0 0 0 4px var(--accent);
  /* §8's one documented variant, for the terminal prompt row. */
  --focus-prompt: inset 2px 0 0 var(--accent);
  /* A focused canvas node, §13: hue change plus a soft halo, never a glow. */
  --focus-node: 0 0 0 3px rgb(198 242 78 / 0.12);
  /* The selected row's leading edge, §12. */
  --edge-selected: inset 2px 0 0 var(--accent);

  /* Type — §4. Both faces are self-hosted; nothing is fetched from a third party. */
  --font-ui: 'Figtree', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  --font-mono: 'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, Consolas, monospace;

  --text-label: 11px; /* mono, uppercase, tracked */
  --text-meta: 12px;
  --text-note: 13px;
  --text-ui: 14px;
  --text-body: 15px;
  --text-read: 16px; /* the conversation, and only the conversation */
  --text-title: 17px; /* dialog and plate titles */
  --text-subject: 20px; /* the session a surface is showing */
  --text-page: 24px;
  --text-display: 64px; /* the landing headline only */

  --track-label: 0.08em;
  --track-tight: -0.01em;
  --track-display: -0.035em;

  --leading-tight: 1.15;
  --leading-ui: 1.55;
  --leading-read: 1.65;

  /* Terminal type keeps its own metrics — §4. Never track a terminal. */
  --term-size: 13.5px;
  --term-leading: 1.65;
  --term-tracking: 0;

  /* Spacing — §5. 4px rhythm; 2px steps only inside a control's own padding. */
  --space-1: 4px;
  --space-2: 8px;
  --space-3: 12px;
  --space-4: 16px;
  --space-5: 20px;
  --space-6: 24px;
  --space-8: 32px;
  --space-12: 48px;
  --space-16: 64px;

  /* Frame — §5, §6. Heights are set on the element, never accumulated. */
  --bar-height: 60px;
  --band-head: 56px;
  --band-foot: 40px;
  --page-gutter: 20px;
  --rail-width: 320px;
  --rail-collapsed: 56px;
  --drawer-width: 360px;
  --read-column: 760px;

  /* Radius — §3. Five steps: control, row, card, panel, pill. */
  --radius-sm: 8px;
  --radius: 10px;
  --radius-md: 12px;
  --radius-lg: 14px;
  --radius-pill: 999px;

  /* Controls — §7 */
  --control-h-sm: 30px;
  --control-h: 32px;
  --control-h-md: 36px;
  --control-h-lg: 40px;
  --control-h-xl: 46px;
  --chip-h: 26px;
  --hit: 32px;

  /* Canvas node size, §13. Fixed: a node's size is the geometry its runtime is
     told to draw at, so a fluid grid would re-wrap everything an agent printed. */
  --canvas-node-w: 420px;
  --canvas-node-h: 280px;

  /* Motion — §8 */
  --motion-press: 140ms;
  --motion-colour: 200ms;
}

/*
 * Dark. Redefined under BOTH the media query (guarded so an explicit light
 * choice wins) and the explicit attribute, so a deliberate choice beats the
 * system preference in either direction. Only the tokens that change are restated.
 */

@media (prefers-color-scheme: dark) {
  :root:not([data-theme='light']) {
    --ground: #101210;
    --surface: #171916;
    --raised: #1b1e1a;
    --inset: #22261e;
    --sunken: #131512;

    --ink: #f1f2ee;
    --ink-2: #a9aea3;
    --ink-3: #8a9082;

    --rule: #23261f;
    --rule-2: #2a2e26;
    --rule-3: #3a3f36;

    --accent: #c6f24e;
    --accent-fill: #c6f24e;
    --accent-wash: rgb(198 242 78 / 0.10);
    --on-accent: #111310;

    --live: #c6f24e;
    --wait: #e9b44c;
    --fail: #f58585;
    --ended: #8a9082;
    --live-wash: rgb(198 242 78 / 0.10);
    --wait-wash: rgb(233 180 76 / 0.10);
    --fail-wash: rgb(245 133 133 / 0.07);

    --scrim: rgb(10 10 11 / 0.64);

    --lift-rest: 0 1px 2px rgb(0 0 0 / 0.35);
    --lift-plate: 0 12px 32px rgb(0 0 0 / 0.5);
  }
}

:root[data-theme='dark'] {
  --ground: #101210;
  --surface: #171916;
  --raised: #1b1e1a;
  --inset: #22261e;
  --sunken: #131512;

  --ink: #f1f2ee;
  --ink-2: #a9aea3;
  --ink-3: #8a9082;

  --rule: #23261f;
  --rule-2: #2a2e26;
  --rule-3: #3a3f36;

  --accent: #c6f24e;
  --accent-fill: #c6f24e;
  --accent-wash: rgb(198 242 78 / 0.10);
  --on-accent: #111310;

  --live: #c6f24e;
  --wait: #e9b44c;
  --fail: #f58585;
  --ended: #8a9082;
  --live-wash: rgb(198 242 78 / 0.10);
  --wait-wash: rgb(233 180 76 / 0.10);
  --fail-wash: rgb(245 133 133 / 0.07);

  --scrim: rgb(10 10 11 / 0.64);

  --lift-rest: 0 1px 2px rgb(0 0 0 / 0.35);
  --lift-plate: 0 12px 32px rgb(0 0 0 / 0.5);
}

/*
 * §16: below 30rem the app bar drops to 52px and every target grows to 44px.
 * The control heights grow with the minimum, because a control is a target.
 * Band heights do not change — the page reads as the same shape on a phone.
 */
@media (max-width: 29.999rem) {
  :root {
    --bar-height: 52px;
    --hit: 44px;
    --control-h-sm: 44px;
    --control-h: 44px;
    --control-h-md: 44px;
    --control-h-lg: 44px;
    --read-column: 100%;
  }
}
```

---

# Part E — `components.css`

Destination: `web/src/styles/components.css`.

```css
/*
 * The shared component layer — docs/Web_Style.md §3, §5, §7, §12 and §14.
 *
 * base.css is element selectors and global resets; component styling does not
 * live there. This file holds the class vocabulary shared across components and
 * belonging to none of them: the two depth levels, the control set, fields,
 * dropdowns, status chips, rows, cards, plates and the tone helpers.
 *
 * Rules belonging to exactly one component stay in that component's own .css.
 */

/* ── Depth, §3 ────────────────────────────────────────────────────────────
 *
 * Two levels. A thing resting on the page carries --lift-rest; a thing floating
 * over it carries --lift-plate and never moves. Nothing translates on hover:
 * hover raises the background one step in the ramp, active lowers it one step,
 * and that is the whole vocabulary. The letterpress contract is retired.
 */

.lift {
  box-shadow: var(--lift-rest);
}

.plate {
  background: var(--raised);
  border: 1px solid var(--rule-2);
  border-radius: var(--radius-lg);
  box-shadow: var(--lift-plate);
}

@media (prefers-reduced-motion: no-preference) {
  .button,
  .chip,
  .row-item,
  .select,
  .field__control {
    transition:
      background var(--motion-colour) ease,
      border-color var(--motion-colour) ease,
      color var(--motion-colour) ease,
      box-shadow var(--motion-press) ease;
  }
}

/* ── Buttons, §7 ──────────────────────────────────────────────────────────
 *
 * One primary per surface, spent on the action that surface exists for.
 * Everything else is secondary or quiet, and the rest goes in the overflow
 * menu — a header carrying four equally-weighted keys makes the reader choose
 * before they can read. Every control is border-box, so a bordered and an
 * unbordered control in the same row measure alike.
 */

.button {
  display: inline-flex;
  gap: var(--space-2);
  align-items: center;
  justify-content: center;
  height: var(--control-h);
  min-width: var(--hit);
  padding-inline: var(--space-3);
  color: var(--ink-2);
  font-family: var(--font-ui);
  font-size: var(--text-note);
  white-space: nowrap;
  background: none;
  border: 1px solid var(--rule-2);
  border-radius: var(--radius);
}

.button:hover:not(:disabled, [aria-disabled='true']) {
  color: var(--ink);
  background: var(--raised);
}

.button:active:not(:disabled, [aria-disabled='true']) {
  background: var(--sunken);
}

.button:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.button:disabled,
.button[aria-disabled='true'] {
  color: var(--ink-3);
  background: var(--inset);
  border-color: var(--rule);
  cursor: not-allowed;
}

/* The primary. No border: the fill is the edge. */
.button--primary {
  color: var(--on-accent);
  font-weight: 600;
  background: var(--accent-fill);
  border-color: transparent;
}

.button--primary:hover:not(:disabled) {
  color: var(--on-accent);
  filter: brightness(1.06);
}

/* Quiet: no border at all. For a third-rank action beside two ranked ones. */
.button--quiet {
  color: var(--ink-2);
  border-color: transparent;
}

/* Destructive. Confirms inline, in place of the controls that raised it. */
.button--danger {
  color: var(--fail);
  border-color: var(--fail-wash);
}

.button--sm {
  height: var(--control-h-sm);
  padding-inline: var(--space-3);
  font-size: var(--text-meta);
  border-radius: var(--radius-sm);
}

.button--md {
  height: var(--control-h-md);
  font-size: var(--text-ui);
}

/* A panel's one full-width primary action — the rail's new-session key. */
.button--block {
  width: 100%;
  height: var(--control-h-lg);
  font-size: var(--text-ui);
  font-weight: 600;
}

/* The landing's opening action. */
.button--xl {
  height: var(--control-h-xl);
  padding-inline: var(--space-5);
  font-size: var(--text-body);
  font-weight: 600;
  border-radius: var(--radius-md);
}

/* An icon-only control: the overflow key, a collapse handle, a dismiss. */
.button--icon {
  width: var(--control-h);
  min-width: 0;
  padding: 0;
}

/*
 * §7: controls are disabled, never removed, and the reason is stated on the
 * surface — once per row, beside the controls it explains, never in a tooltip
 * and never repeated on each key.
 */
.control-reason {
  overflow: hidden;
  color: var(--ink-3);
  font-size: var(--text-note);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ── Fields, §7 ───────────────────────────────────────────────────────── */

.field {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.field__label {
  color: var(--ink-2);
  font-family: var(--font-ui);
  font-size: var(--text-note);
}

.field__control {
  height: var(--control-h-md);
  padding-inline: var(--space-3);
  color: var(--ink);
  font-family: var(--font-ui);
  font-size: var(--text-ui);
  background: var(--inset);
  border: 1px solid var(--rule-2);
  border-radius: var(--radius);
}

.field__control::placeholder {
  color: var(--ink-3);
}

.field__control:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.field__hint {
  color: var(--ink-3);
  font-size: var(--text-meta);
  line-height: var(--leading-ui);
}

.field__error {
  color: var(--fail);
  font-size: var(--text-note);
}

/* ── Dropdowns, §7 ───────────────────────────────────────────────────────
 *
 * An instrument closed and a plate open. Choosing a VALUE is a select;
 * choosing an ACTION is a menu. A select never runs anything when it opens.
 */

.select {
  display: inline-flex;
  gap: var(--space-2);
  align-items: center;
  justify-content: space-between;
  height: var(--control-h);
  min-width: 0;
  padding-inline: var(--space-3);
  color: var(--ink);
  font-family: var(--font-ui);
  font-size: var(--text-ui);
  background: var(--surface);
  border: 1px solid var(--rule-2);
  border-radius: var(--radius);
}

.select__value {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* The caret flips rather than rotating: at this size a rotation reads as a wobble. */
.select__caret::before {
  content: '▾';
  color: var(--ink-3);
  font-size: 9px;
  line-height: 1;
}

.select[aria-expanded='true'] .select__caret::before {
  content: '▴';
}

.menu {
  position: absolute;
  z-index: 20;
  min-width: 200px;
  margin: 0;
  padding: var(--space-2);
  list-style: none;
  background: var(--raised);
  border: 1px solid var(--rule-2);
  border-radius: var(--radius-lg);
  box-shadow: var(--lift-plate);
}

.menu__item {
  display: flex;
  gap: var(--space-3);
  align-items: center;
  width: 100%;
  height: var(--control-h-lg);
  padding-inline: var(--space-3);
  color: var(--ink-2);
  font-family: var(--font-ui);
  font-size: var(--text-ui);
  text-align: left;
  cursor: pointer;
  background: none;
  border: 0;
  border-radius: var(--radius-sm);
}

.menu__item:hover,
.menu__item--active {
  color: var(--ink);
  background: var(--inset);
}

.menu__item[aria-disabled='true'] {
  color: var(--ink-3);
  cursor: not-allowed;
}

/* Selected is marked by a tick as well as by weight — never colour alone. */
.menu__item[aria-selected='true'] {
  color: var(--ink);
  font-weight: 600;
}

.menu__tick::before {
  content: '✓';
  color: var(--accent);
}

.menu__kind {
  margin-left: auto;
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-label);
}

/* ── Status, §2 and §7 ────────────────────────────────────────────────────
 *
 * A pill carrying a dot and a word. The word is mandatory: colour never carries
 * meaning alone, and `stopped` is never drawn as a failure.
 */

.status {
  display: inline-flex;
  gap: 7px;
  align-items: center;
  height: var(--chip-h);
  padding-inline: 10px;
  font-family: var(--font-ui);
  font-size: var(--text-meta);
  font-weight: 500;
  white-space: nowrap;
  border-radius: var(--radius-pill);
}

.status__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentcolor;
}

.status--live {
  color: var(--live);
  background: var(--live-wash);
}
.status--wait {
  color: var(--wait);
  background: var(--wait-wash);
}
.status--fail {
  color: var(--fail);
  background: var(--fail-wash);
}
.status--ended {
  color: var(--ended);
  background: var(--raised);
}

/* A neutral tag: a count, a filter that is off, a fact about a service. */
.tag {
  display: inline-flex;
  gap: 6px;
  align-items: center;
  height: var(--chip-h);
  padding-inline: 10px;
  color: var(--ink-2);
  font-family: var(--font-mono);
  font-size: var(--text-label);
  letter-spacing: 0.06em;
  background: var(--surface);
  border: 1px solid var(--rule);
  border-radius: var(--radius-pill);
}

.tag--on {
  color: var(--on-accent);
  font-family: var(--font-ui);
  font-weight: 500;
  background: var(--accent-fill);
  border-color: transparent;
}

/* ── Rows, §12 and §14 ────────────────────────────────────────────────────
 *
 * A selectable row (the rail) and a record row (operations) are different
 * things: one is a target, the other is read. Both are rows, neither is a table.
 */

.row-item {
  display: grid;
  grid-template-columns: auto 1fr auto;
  gap: 2px var(--space-3);
  width: 100%;
  padding: var(--space-3);
  font-family: var(--font-ui);
  text-align: left;
  cursor: pointer;
  background: none;
  border: 0;
  border-radius: var(--radius-md);
}

.row-item:hover {
  background: var(--raised);
}

/* The selected session is marked on its leading edge and by its background. */
.row-item[aria-selected='true'] {
  background: var(--raised);
  box-shadow: var(--edge-selected), var(--lift-rest);
}

.row-item:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.row-item__dot {
  grid-row: 1 / span 2;
  align-self: start;
  margin-top: 5px;
  width: 7px;
  height: 7px;
  border-radius: 50%;
}

.row-item__subject {
  color: var(--ink);
  font-size: var(--text-ui);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.row-item__meta {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-label);
}

/*
 * What the session last did — §12. It never wraps: a row that grows with its
 * longest sentence makes the list a different height every time it reloads.
 */
.row-item__activity {
  grid-column: 2 / span 2;
  overflow: hidden;
  color: var(--ink-2);
  font-size: var(--text-note);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.record {
  display: flex;
  flex-direction: column;
  margin: 0;
  padding: 0;
  list-style: none;
  overflow: hidden;
  background: var(--surface);
  border: 1px solid var(--rule);
  border-radius: var(--radius-md);
}

/* Rows are read one at a time: no zebra striping, no hover, nothing navigates. */
.record__row {
  display: grid;
  grid-template-columns: 190px 1fr auto;
  gap: var(--space-4);
  align-items: center;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--rule);
}

.record__row:last-child {
  border-bottom: 0;
}

.record__at {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-meta);
  font-variant-numeric: tabular-nums;
}

.record__primary {
  color: var(--ink);
  font-size: var(--text-body);
}

.record__meta {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-meta);
}

/* An identifier is drawn as an identifier, so nobody reads it as a name. */
.record__identifier {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-meta);
}

.record__empty {
  color: var(--ink-3);
  font-size: var(--text-note);
}

/* ── Cards and notices, §3, §12 ──────────────────────────────────────────
 *
 * A row of peer cards is a grid, never a flex row of hand-set widths.
 */

.card {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  min-width: 0;
  padding: var(--space-4);
  background: var(--surface);
  border: 1px solid var(--rule);
  border-radius: var(--radius-md);
}

.card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: var(--space-3);
  align-items: stretch;
}

/*
 * A failed action — §12. Inside the surface's own column, above what it shows
 * and never in place of it, and dismissible: it reports one attempt about one
 * session, and the surface beneath it may hold a dozen live terminals.
 */
.notice {
  display: flex;
  gap: var(--space-3);
  align-items: flex-start;
  padding: var(--space-3) var(--space-4);
  border-radius: var(--radius-md);
}

.notice--fail {
  background: var(--fail-wash);
  border: 1px solid var(--fail);
}

.notice--quiet {
  background: var(--raised);
  border: 1px solid var(--rule);
}

.notice__title {
  color: var(--ink);
  font-size: var(--text-ui);
}

.notice__detail {
  color: var(--ink-2);
  font-size: var(--text-note);
}

/* ── Bands, §5 and §6 ─────────────────────────────────────────────────────
 *
 * A band's height is set on the band, never accumulated from padding: padding
 * is what lets two aligned columns drift apart when one gains a longer label.
 */

.band {
  display: flex;
  flex: none;
  gap: var(--space-3);
  align-items: center;
  padding-inline: var(--space-5);
}

.band--bar {
  height: var(--bar-height);
  gap: var(--space-5);
  background: var(--ground);
  border-bottom: 1px solid var(--rule);
}

.band--head {
  height: var(--band-head);
  padding-inline: var(--space-6);
  background: var(--surface);
  border-bottom: 1px solid var(--rule);
}

.band--foot {
  height: var(--band-foot);
  gap: var(--space-4);
  background: var(--ground);
  border-top: 1px solid var(--rule);
}

.band__subject {
  color: var(--ink);
  font-size: var(--text-subject);
  font-weight: 600;
  letter-spacing: var(--track-tight);
  font-variant-numeric: tabular-nums;
}

.band__label {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-label);
  letter-spacing: var(--track-label);
  text-transform: uppercase;
  white-space: nowrap;
}

.band__value {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-label);
  white-space: nowrap;
}

.band__spacer {
  flex: 1 1 auto;
  min-width: 0;
}

/* ── Section labels ─────────────────────────────────────────────────────── */

.eyebrow {
  color: var(--ink-3);
  font-family: var(--font-mono);
  font-size: var(--text-label);
  letter-spacing: var(--track-label);
  text-transform: uppercase;
}

/* ── Semantic tone helpers, §2 ───────────────────────────────────────────
 *
 * Session and system state. Never decorative, never the only carrier.
 */

.tone-ink-live {
  color: var(--live);
}
.tone-ink-wait {
  color: var(--wait);
}
.tone-ink-fail {
  color: var(--fail);
}
.tone-ink-ended {
  color: var(--ended);
}

.tone-bg-live {
  background: var(--live);
}
.tone-bg-wait {
  background: var(--wait);
}
.tone-bg-fail {
  background: var(--fail);
}
.tone-bg-ended {
  background: var(--ended);
}

/* ── Waiting, §8 and §11 ─────────────────────────────────────────────────
 *
 * A pulse on a dot, or a bar reflecting steps the control plane reported.
 * Never a spinner, and never a bar that advances on a timer.
 */

.pulse {
  animation: pulse-dot 1.4s ease-in-out infinite;
}

@keyframes pulse-dot {
  0%,
  100% {
    opacity: 0.35;
  }
  50% {
    opacity: 1;
  }
}

.progress {
  height: 3px;
  overflow: hidden;
  background: var(--inset);
  border-radius: var(--radius-pill);
}

.progress__fill {
  height: 100%;
  background: var(--wait);
  border-radius: var(--radius-pill);
}

@media (prefers-reduced-motion: reduce) {
  .pulse {
    animation: none;
    opacity: 1;
  }
}
```
