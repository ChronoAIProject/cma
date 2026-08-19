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
