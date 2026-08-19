# Web Style

**This document is the single source of truth for all chrono-bot frontend design and style.**

No colour, spacing value, type scale step, radius, component pattern, or interaction rule may be
introduced in `web/` unless it appears here. If a design need is not covered, extend this document
first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

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

**Each body the log carried is its own region.** What the call was asked to do and what it produced
are two bodies, and putting them in one window with one scroll position means a long argument
payload pushes the result out of sight — so the reader scrolls past what they already knew to reach
the thing they opened the row for, which is the scrollback this section exists to replace. Each takes
its own scrollport under the same cap, its own gutter label, in the order the log holds them. A body
that did not arrive draws nothing, never an empty labelled region: §18.5's rule that absence is a
fact and not a zero. Truncation is reported per body, because the log bounds them separately.

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

**The commands the kernel offers are reachable from the field.** Typing `/` at the head of an empty
draft opens a filtered list built from what the kernel advertised and from nothing else — no command
this document invented, on §18.2's rule for a tool's kind. Arrow keys move, Enter or a click inserts
the command and leaves the caret after it, Escape closes and returns focus to the field: the plate
behaviour §3 already defines, reused rather than reinvented. A kernel that advertises none opens
nothing at all, on the same reasoning that keeps an ungranted affordance off the surface entirely.

They are liveness and not history. A kernel's command list arrives on the live lane and is absent
after a reload until the kernel says again, which is why it cannot be drawn in the stream: §18.1
requires a turn replayed to match a turn watched, and a stale list replayed from the log would offer
commands that no longer exist.

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

### 18.10 A stage, a mode change, and the commands a kernel offers

Announced in the changelog on 2026-08-19 and never written. The surface has been drawing all
three since, which put them outside this document's authority — the thing §7 exists to prevent.
Written now against what the kernel already sends.

**A stage is the agent's plan, drawn as structure.** A `--sunken` list of steps inside `--radius-md`,
each step the kernel's own words verbatim and its own status word beside it, sitting **above** the
in-flight content: a plan is what the agent is doing now, not a thing it said, and placing it in the
stream would date it the moment the next step began. It arrives as a full revision and replaces the
one before it, because the kernel sends the whole list each time and merging two revisions would
invent a plan neither of them described.

**A mode change is a safety fact about the session, not agent content.** One mono `--text-label` line
in `--ink-3` carrying the kernel's own word — `unrestricted`, `on-request`, whatever that kernel
calls it — never mapped onto a set this document invented, on the rule §18.2 already holds for a
tool's kind. It is drawn where state is drawn and never in the reading column, because a reader
scanning the agent's words must not meet a sentence the agent did not write.

**The commands a kernel offers are liveness and belong in the composer, not the stream.** They were
printed as a static list at the foot of the message column, below every turn, where they could be
read and not used: the reader retyped what they had just read. They move to §18.4, and the stream
draws none of them.

**Both the stage and the mode ride the durable log**, so a reload shows what a turn watched arriving
showed — §18.1's equivalence rule, which the command list cannot satisfy and is why it is liveness.

### 18.11 What a reader can take away

An answer that cannot leave the surface has not finished being useful. This plane exists to drive a
coding agent, and the two things somebody does with what an agent produced are run it and quote it —
both of which mean getting the characters out intact.

**Every fenced block carries a header.** A row on the block's own frame: the fence's info string on
the left in mono `--text-label` `--ink-3`, rendered **verbatim** and uppercased by `text-transform`
only — casing is typography, but mapping `py` to `Python` is naming, and §18.2 and §18.10 both forbid
this document inventing a set the kernel did not use. A fence carrying no info string draws no label,
never an invented one. A copy control sits at the right, following §12's existing pattern.

The header is `position: sticky` against the stream's scrollport, on the wrapper's own background, so
a long listing scrolls beneath its own label rather than out from under it. That means the header
cannot live inside the `<pre>`: the `<pre>` owns the horizontal overflow, and a sticky child of a
horizontally scrolling box slides away sideways. The block becomes a wrapper carrying the header and
an inner `<pre>` that keeps the scrolling.

**Every settled turn carries a copy control** — the agent's and the owner's alike. It writes the
turn's **markdown source**, not the rendered tree, and that is the whole point rather than an
implementation note: selection-copy already works and already yields degraded text, because the
renderer strips heading hashes, list markers, fences and backticks before drawing. Somebody pasting
an agent's numbered steps into an issue, or its backticked command into a shell, gets something the
agent did not write.

Persistently drawn and labelled, never revealed on hover: §3 permits no hover-gated affordance and
says nothing in this interface moves on hover. While the turn is still in flight the control is
**disabled with its reason stated**, not removed — §7 and §19 — because copying half an answer is the
thing to prevent and removing the control is the wrong instrument for it.

**Copy is a claim about the clipboard and is made only after the write resolves.** The guarded
`navigator.clipboard?.writeText`, the word changing on the control itself, and a refusal swallowed
in silence because the text remains selectable — the pattern §12 already sets.

### 18.12 What an agent wrote, and what the surface can draw of it

§18.1 fixes the treatment of code, lists, headings and emphasis. It says nothing about the rest of
what an agent writes, and the renderer draws none of it: a link arrives as `[label](url)`, a table as
a wall of pipes, every nesting level flattens to one, and a wrapped list of steps runs together into
a paragraph.

**Links are drawn, and carry no colour.** Underline and cursor distinguish them and nothing else —
§18.1 forbids colour on agent content without exception, and a blue link is that exception arriving
by habit. The href is rendered as the agent wrote it; §9's rule for the terminal's OSC 8 sequences is
the precedent, and the same reasoning applies here: this surface does not decide where an agent's
link points.

**Tables take the structure they were written in**, on the same hairline-and-ground treatment §16
uses for a record list, scrolling horizontally inside their own frame rather than widening the
reading column. **Nested lists keep their nesting**, because a plan with sub-steps flattened to one
level is a different plan. **Blockquotes** take a `--rule` left border and `--ink-2`. **A soft line
break is a line break**: an agent that wrote its steps on separate lines meant them to be on separate
lines, and joining them into a paragraph rewrites the shape of the answer.

None of this takes colour, and none of it takes a syntax highlighter. The rule is unchanged: the
frame is styled and the content is not.

### 18.13 When the surface cannot hear the session

**The failure mode is asserting liveness that is false, not failing to narrate the transport.**

While the stream is down mid-turn the surface goes on drawing the §11 wave and the word beside it,
and goes on blinking the caret on a message that has stopped growing — because nothing but the run
ending clears the in-flight state. The one affordance built to distinguish a working agent from a
dead stream is therefore the one that lies, which is worse than drawing nothing.

**A transient reconnect says nothing.** The terminal plane already holds this line and it holds here:
a blip announced as a fault teaches a reader to distrust the next notice, and §11 forbids drawing a
recovering session as a failed one.

**The moment the link is not live, the liveness claim is withdrawn** — the pulse stops and the caret
stops — because withdrawing a false assertion is quieter and cheaper than adding a true one, and it
is correct at every point in between.

**Only a link that has stayed down past a bounded threshold is named**, once, in words, where the
turn is: this surface is not hearing the session, and what is on screen is what arrived before it
stopped. Not a banner across the head — the reader is looking at the turn, and §12 already puts a
failed action at the head of the column for actions, which this is not.

Nothing here is a failed run. §18.9 draws a run the agent could not finish; this draws a surface that
cannot hear one, and conflating them tells an owner their work broke when their network did.

---

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

| Date | Change |
|---|---|
| 2026-08-12 | Initial version. Established colour tokens for light and dark, semantic state colours, type stacks and scale, 4px spacing system, component rules for status chip, data table, terminal and buttons, interaction and accessibility baselines. |
| 2026-08-13 | Terminal-first cyberpunk redesign. Reframed the frontend as a single full-bleed terminal page with no navigation, in two states (scripted introduction and live session). Replaced the colour system with a cyan/magenta-on-near-black palette, recording measured WCAG ratios and raising `--ink-3` to `#7C8A9E` in dark to clear AA for 11px metadata. Added the `--emphasis` token, a bounded three-step glow system with an ambient overlay cap, and an authoritative 16-slot ANSI palette. Promoted the monospace stack to primary and retuned the type scale. Made the page non-scrolling and the terminal full-bleed, and set radii to 0/2px. Specified the intro sequence timing, skip behaviour and reduced-motion equivalence, the session tab strip, and the profile menu. Added §8 Terminal safety covering OSC 52, tab-title suppression, scrollback caps and OSC 8 validation. Removed the data table component and the "information density over whitespace" premise, both of which described a dashboard this frontend no longer is. |
| 2026-08-13 | Documented one focus-state variant in §7: the terminal prompt row carries its 2px accent indicator on the row's leading edge rather than as a ring around the input, because a ring around a full-width borderless field boxes in the whole prompt line and breaks the terminal illusion. A relocation, not a removal, scoped to that one input. |
| 2026-08-13 | Added §10 Selectable terminal themes: six vendored palettes (Dracula, Nord, Solarized Dark, Solarized Light, GitHub Light) alongside the product's own, selectable per person and scoped to the terminal canvas only. Vendored as data rather than taken as a dependency. Themes are filtered by the current appearance so a dark palette cannot be placed on a light page. Also specified the appearance control in the profile menu, cycling system → light → dark, where "system" is the absence of a choice rather than a third palette. |
| 2026-08-14 | Added §11 Waiting for a session: the provisioning message animates as a left-to-right character wave (1.4s loop, 45ms stagger, 0.28em lift), chosen over a spinner because §5 forbids one and because a wave claims no knowledge of progress. Motion carries no information — the sentence is exposed once to assistive technology and the animation is removed under reduced-motion. Also specified that a session which fails to start must show the recorded reason rather than the same wording as one that ended normally. |
| 2026-08-14 | Added §12 Page frame and layout and §13 The platform typeface. Established a persistent top bar in both signed-out and signed-in states, a 10% inset on page content with terminal output exempt, a vertical session column on the left labelled by creation time in Singapore time (newest first, reversible, date shown only once not from today), and a collapsible session details panel on the right that collapses to a handle. Columns stack below 60rem. Adopted Geist Pixel as the platform typeface, self-hosted under the SIL Open Font License with a system fallback and `font-display: swap`; terminal content keeps its monospace stack because cell metrics govern its rendering. Replaced the horizontal session tab strip specified in the 2026-08-13 redesign, which truncated labels once sessions accumulated. |
| 2026-08-14 | Corrected §12: the console obeys the page inset like every other surface, so the session column aligns with the product mark and the details panel with the account control. The earlier entry exempted terminal output from the inset on the grounds that cropping it wasted width; alignment between the bar and the page beneath it matters more, and a full-bleed console under an inset bar reads as a detached bar. Also moved the session lifecycle controls out of the top bar and into the details panel, beside the session they act on. |
| 2026-08-14 | Added §14 Session states and what they allow: a single table binding each of the six session states to what the terminal does and which controls are available, with one rule shared by the terminal, the buttons and the list so they cannot disagree. Established that controls are disabled rather than removed and that every disabled control and every non-interactive terminal states its reason in the owner's terms; that a session with no sandbox still shows its history read-only, with keystrokes dropped rather than buffered; that ended sessions leave the list unless they ended by failing; and that revive is the primary action wherever it is offered. Adds the `rotating` state, during which the session is neither usable nor stopped and no control acts on it. |
| 2026-08-14 | Added §15 Canvas — every session at once: a second arrangement of the same sessions, in which each listed session is a 420×280 framed node on a pannable surface hosting its own terminal, chosen by a two-button toggle at the trailing edge of the top bar whose stored value is validated on read. The canvas replaces the console's middle column only, so the rail, the details panel and the bar keep their places and one selection serves both arrangements. Established that exactly one node is focused and only it receives keystrokes, with hover distinguished from focus by border weight rather than hue; that focus is never a second opinion on §14, so a provisioning, rotating, idle or ended session stays read-only on the canvas, says so on the node, and shows its stored transcript rather than an empty frame; that the canvas is one tab stop with clamped arrow navigation and a polite live region announcing moves; that unfocused terminals are never dimmed, tinted or covered; that the only motion is the 150ms focus transition, wrapped in `prefers-reduced-motion: no-preference`; and that at most 12 nodes hold a terminal at once, evicted by least recent focus, with the count of unattached sessions stated below the viewport because silent truncation reads as breakage. Adds the `--canvas-node-w` and `--canvas-node-h` tokens, and labels nodes by start time as §12 labels the rail rather than by identifier, so one session is not called two things in two views. |
| 2026-08-14 | Specified in §12 where a failed action is reported: one `sm` `--danger` line with a `--rule` underline at the head of the middle column, above whatever the column is showing and never in place of it, recording 6.16:1 light and 5.47:1 dark against `--surface`. The console previously rendered the failure instead of the surface, which was survivable when the surface was one terminal and is not now that §15 puts up to twelve attached terminals there — a stop returning 409 detached eleven sessions that had not failed. Also established that the line reports the most recent attempt, cleared by every action as it starts, so a stale failure never sits above a console that has recovered. |
| 2026-08-14 | Recorded in §15 that a node's size is the geometry its runtime is told to draw at, so choosing the canvas resizes every attached session to roughly 47 columns and a dozen rows and each agent redraws at that width. Kept the behaviour — a terminal in a small frame is a small terminal, and declaring anything else wraps output against a width the frame does not have — but stated its two consequences: output already wrapped narrow stays wrapped, and a session detached mid-arrangement keeps its last geometry until something attaches to it and re-declares one. The mode toggle was previously described in the code as reaching nothing beyond the arrangement; it reaches the runtime of every session on the canvas, and may not be described as a browser-only control. |
| 2026-08-14 | Added §16 The operator panel: a dialog opened from the profile menu as an account action, keeping the product one page, whose entry point is drawn only when the control plane reports that the caller holds an operator capability — presentation, never protection, since the API refuses on its own. Established that each of its four views is offered on its own capability, so a view is never drawn empty because of entitlement; a view switch marked up as a tablist (monospace `xs` uppercase labels; selected by `--accent` text plus a 2px `--accent` bottom border and `aria-selected`, never colour alone); and a record list — rows rather than the data table removed on 2026-08-13 — with a leading tabular-nums timestamp, `base` primary label, `xs` `--ink-3` monospace metadata, hairline separators, no zebra striping and no row-level click target, ordered newest first always and filterable by person and by action. Established that a person is named by display name then email and never by their subject identifier, which is shown in identifier style when nothing else is known, and that an unattributed action reads "Not signed in"; that a provider which could not be asked renders as unknown carrying its reason and never as one running nothing, with the running total withheld while any provider is unobservable; and that stop and terminate on another owner's session keep the §6 destructive pattern — terminate confirms inline, both are disabled with a stated reason rather than removed, and deleting somebody else's session is not offered at all. |
| 2026-08-17 | Ground control: full redesign of the frontend's visual language and layout, replacing the cyberpunk terminal-first system established 2026-08-13. Retired the cyan/magenta pair for one lime accent with a measured light counterpart, and reworked the surface ramp into ground/surface/raised/inset with worst-case contrast recorded per theme (dark `--ink-3` 5.38:1 on inset; light `--ink-3` 4.59:1 on inset). Replaced the bounded glow system with letterpress depth — hard-offset impression shadows with zero blur, hover translating into the impression, active flush, disabled carrying none, focus stacking a page-halo and accent ring over whichever impression the state holds — leaving one 6px halo on a live session's dot as the only glow in the product. Adopted Geist and Geist Mono, self-hosted, and retired Geist Pixel in favour of a mark drawn from four squares. Removed the 10% page inset, which cost the terminal ~290px at 1440px, and replaced it with a band grid: a 52px head band and a 40px foot band whose heights are set rather than accumulated from padding, one type size per band, and a second band for any column needing more chrome. Established one app bar and one status band on every page; two control heights and no others, every control `box-sizing: border-box` so a bordered and an unbordered control measure alike; peer cards as grids so content length cannot decide card size; one dropdown pattern, instrument closed and plate open; and a collapse pattern in which a panel becomes a 48px handle carrying a summary rather than disappearing. Reframed the frontend as one page frame with two views — sessions and operations — where the canvas is an arrangement of sessions rather than a view of its own, and moved operations out of a 560px dialog onto a page for the one surface that shows many rows at once. Kept the canvas node fixed at 420×280 and restated why: a node's size is the geometry its runtime draws at, so a fluid grid would re-wrap everything an agent has printed. Promoted the session lifecycle controls into the session header, demoted the details panel to a 412px sheet beneath the terminal, and added a last-activity line to each rail row. Added §16 Responsive with four widths, and the rule that the terminal is the last surface to give up width. Set light `--ink-3` and `--ended` to `#686871`: at the `#6B6B74` the handoff carried, both measured 4.39:1 on `--inset` and failed the 4.5:1 floor this section sets for the 11px uppercase metadata `--ink-3` exists to carry. | |
| 2026-08-18 | §1 admits the conversation plane. The frontend was "a terminal, and the chrome needed to run several of them", which was true while a terminal was the only thing it could offer; `acp-kernel` made a second plane possible and nobody had built one. It becomes two planes onto one session — terminal and conversation — drawn in the same middle column and chosen by what the session was granted. A plane is not a view, exactly as an arrangement is not: the views stay two, the frame stays one, and there is still no navigation. Replaced the binding rule "never build an interactive feature here that belongs in the API" with a stricter one: the conversation surface may call no route a third-party client could not call, and where it needs something AG-UI lacks the answer is to add it to AG-UI rather than here. The old rule asked whether a feature *belonged* in the API, which is a judgement; the new one asks whether the API *already projects it*, which is checkable — and a first-party client held to the same constraint is the only thing that keeps `acp-kernel`'s claim honest. Re-examined §1's three surviving design consequences: "the terminal owns whatever width is left" becomes the *surface* owning it, terminal or conversation; "one surface, many drivers" survives unchanged and now covers a conversation driven by a recorded transcript in development and a live stream in production; "fidelity beats decoration inside the viewport" survives and extends to agent-emitted message content, so the code blocks, lists and inline formatting an agent wrote are rendered as it meant them and only the frame around them is styled. |
| 2026-08-18 | Added §18 The conversation surface, the plane §1 admitted on the same date, written before it is built so its review is "does this match §18" rather than a matter of taste. §18.1 the message stream: turns as bands separated by a rule rather than speech bubbles, the owner's turns in `--ink-2` and the agent's in `--ink`, nothing restyling itself on commit because a message that does tells a reader the earlier state was provisional, the surface pinned to the newest turn only while nobody has scrolled away from it, and the equivalence rule that governs the whole plane — a turn drawn from history and a turn watched arriving must produce the same result, since the one a person reloads into is the one they will believe. Extended §1's fidelity rule concretely: code blocks, inline code, lists and headings take the scale §4 already defines, and no colour is ever applied to agent-emitted content, because the semantic tokens describe session and system state and describe nothing an agent said. §18.2 tool calls as structure: collapsed by default at one control height, the kernel's own words for kind and target rendered verbatim rather than mapped onto a set this document invented, a running call distinguished by the wait animation and a failed one by `--fail` with a status word beside it, and enormous output bounded by disclosure rather than truncated — bounding is a frame decision and dropping the tail is not. §18.3 the approval card: the card impression and an accent border, drawn in the message stream where the turn stopped, carrying the kernel's own wording verbatim and the answers the kernel offered rather than a hardcoded allow/deny pair, the affirmative answer taking the surface's one accent key, surviving a reconnect because approvals ride the durable stream, and explicitly **not a modal** — a session can wait on one while its owner reads something else, and a modal makes every other session in the rail unreachable. §18.4 the composer: a block-height field growing to six lines, and behaviour mid-turn read from the session's own `concurrent_input` policy rather than invented or guessed at, with the sentence on the surface beside the key. Steering shows no affordance at all where it was not granted, because a control that can never work is not one that is currently unavailable, and interrupt lives beside the composer for the same reason somebody reaches for it there. §18.5 usage, and the honest absence of it: the count and cost in the surface's foot band, never estimated, and a session not granted `usage` saying "not reported" rather than showing a zero — the same rule §14 holds for a running total while a provider is unobservable, for the same reason, since a session with no usage grant has often spent a great deal. §18.6 plane and changing it: the affordance absent rather than disabled where the kernel does not advertise the other plane, an inline confirmation stating the two costs that are not obvious, `rotating` governing the window, and the boundary afterwards drawn as a rule across the message stream at `rotated_at` rather than as a status chip — a gap somebody can see is one they can recognise. §18.7 what the surface does not do: it renders no terminal, offers no operation AG-UI does not project, names the capability behind every refusal on screen rather than only in a console, and never renders a credential. Amended §11 so its one table covers both planes rather than growing a second that could disagree with it, and added what the three routinely-misread states mean — `rotating` where clients must refuse input rather than accept and lose it, and `idle` and `recovering`, neither of which is a failure and neither of which takes `--fail`. Amended §12 where it assumed the middle column was a terminal: three columns are the list, the session's *surface*, and what is known about it. |
| 2026-08-18 | Extended §18.7 with what the terminal can do that the conversation surface cannot, and where each of those lives instead. The section already said the surface renders no terminal and offers no operation AG-UI does not project; what it did not say is what somebody should do about it. Three of the six rows have one answer — the plane changes and the session does not, keeping its identifier, its history and its place in the rail — and the other three are capabilities the kernel behind the session either offers or is named for lacking. Stated rather than left to be discovered, because a person hunting for a control that was never going to exist blames the interface and then themselves. |
| 2026-08-18 | Added §18.9 A run that failed. §11 already said a session that ends badly says why and §18.7 already said every refusal names its capability on screen rather than only in a console; neither was drawn, because the conversation surface folded a failed run into state nothing read, so a run that failed looked exactly like a run still going — nothing at all. Established that a failure is drawn in the stream at the point it stopped rather than in a band above it, on the same reasoning §12 uses for a failed action and one step further: a failure has a position, and what the agent had already done stays readable beside it. Established the three things it says and their order — what failed in the kernel's own words, the ability it lacked **named** and only where the failure was a refusal, and the stop reason as the API spells it — and that most failures name nothing, since a name invented for those would be read as one and acted on. Recorded the trap that made this worth writing down: a session being carried onto another runtime ends the run and arrives on the *same frame* a genuine failure does, because the protocol has one event for a run ending badly, so a client that cannot tell them apart draws every rotation as a fault — which §11 forbids outright. What distinguishes them is stated by the API rather than inferred from wording, and where it is absent the frame is drawn as a failure, that being the safer of the two to be wrong about. |
| 2026-08-18 | Amended §18.1 with what a turn looks like while it is still running, which the section had described only for its two settled ends. Between the turn starting and anything arriving the surface draws the §11 wave with a word: a turn that has started and produced nothing is otherwise indistinguishable from a dead stream, and a turn takes as long as the agent takes. Established that the message being typed is **not a stream entry** — it is drawn after the last entry, in the same band and ink as any other, and is *replaced* by the committed message rather than appended to. That is what makes the section's own equivalence rule hold by construction rather than by discipline: history contains no in-flight message, so a reload folds none and has none to draw, and the two agree without anybody remembering to make them. Modelling it as an entry and removing it on commit would be the same rule enforced by hand, and the hand is what fails. The existing caret rule now follows from that rather than standing as a second one: the caret belongs to the message being typed and to nothing else. |
| 2026-08-18 | Added §18.8 What the agent is working through. A turn spent thinking was indistinguishable from a turn spent stuck, and thinking is most of what a long turn is. Established that reasoning is **liveness and never in the record** — removed when the turn ends, absent after a reload — and that the surface says so in words rather than leaving it to this document, because a reader who saw reasoning and reloaded to find it gone reports it as lost work. Recorded why it may not be durable: §18.1 requires a turn drawn from history and a turn watched arriving to produce the same result, so a durable thought is one that gets replayed, and somebody reloading to check what the agent decided would watch it think a second time; the turn's own boundaries already say when the thinking happened. One band above the message being typed, height-capped with its own scroll pinned to the newest, since reasoning is unbounded and arrives fast and would otherwise push the conversation off the screen while saying less than any line of it. **No colour, ever** — §2's semantic inks describe session and system state and describe nothing the agent thought — with the label carrying the §11 wave while reasoning arrives. A kernel that does not reason draws nothing at all rather than an empty band, on §18.4's existing rule. And it is not announced: the band changes many times a second, and a live region there would make the surface unusable with a screen reader, while the message that follows is announced and is what is worth hearing. |
| 2026-08-18 | Quiet instrument: full redesign of the frontend's visual language, replacing the Ground control letterpress system of 2026-08-17. Kept the accent and every rule in §19, changed everything that made the interface hard to read. Replaced the neutral ramp with warm near-neutrals in five steps per theme (ground/surface/raised/inset/sunken), recording worst-case measured contrast per theme — dark `--ink-3` 4.69:1 on `--inset`, light `--ink-3` 5.06:1 on `--inset` — and set `--ink-3` to `#8A9082` dark and `#64685E` light so 11–12px metadata clears AA on every surface it sits on, including a disabled control's own fill, which the previous revision left at 2.5:1. Retired letterpress depth for two levels and no others — one for a thing resting on the page, one for a thing floating over it — with hover raising the background a step rather than translating into an impression, and nothing in the interface moving on hover. Replaced the single 2px radius with five steps from 8px to a pill, since a hard offset shadow was the only reason a sharp corner was required. Adopted Figtree and JetBrains Mono, self-hosted under the OFL, and retired Geist and Geist Pixel; raised the reading sizes so the agent's own words are 16px at 1.65 and nothing read at length is below 15px, which is what "squeezed" meant. Capped the conversation at a 760px centred reading column, gave the owner's turns a right-aligned block on `--inset` and the agent's turns the full column unboxed, so the two roles are told apart by shape as well as ink. Moved what is known about a session from a 412px bottom sheet to a 360px right drawer, because the sheet took its height from the surface it was describing. Reduced the session header to one primary, one secondary and an overflow menu, and moved End and Delete to the foot of the drawer. Made the command trigger a working `⌘K` plate over the surfaces it can already reach, rather than a disabled field in the middle of the bar. Collapsed tool calls to 40px rows carrying a status glyph, the kernel's own kind and target, a duration and a status word, and collapsed reasoning to one key stating that it is not kept. Replaced the conversation surface drawn inside a 420×280 canvas node — where the composer clipped mid-placeholder — with a recap of the last thing the agent said and a key that opens the surface which can hold a reading column, and gave the canvas an explicit trailing fade and pan affordance so a viewport narrower than the grid reads as pannable rather than clipped. Fixed one live defect in §11's wording rather than its rules: a session parked on `requires_action` is `idle`, and told its owner it was stopped while an approval it could answer sat above the composer; it now says it is waiting for them, in the rail, the header and the composer. Fixed the double hairline where a panel and its first band both drew a border on the shared edge, and added §5's rule that one boundary draws one hairline. |
| 2026-08-19 | §18.2 records tool bodies, and says when it does not have them. A tool call's row now discloses what it was asked to do and what it produced when the session was granted the recording, drawn from the durable log — never estimated, never reconstructed from the terminal. A body cut to fit the log's bound is marked `(truncated)` rather than silently shortened, the same rule the section already holds for output: disclosure is a frame decision, dropping the tail without saying so is not. Recording tool bodies makes §18.7's "it never renders a credential" non-trivial for the first time, so the core redacts the values it handed the session before a body is written, and the surface renders nothing it was not given. |
| 2026-08-19 | Added §18.10 A stage and a mode change. The conversation surface now says what the agent is at and what it is waiting for, because the kernel already said so. A stage is the agent's plan drawn as structure rather than prose — a `--sunken` list of steps, each the kernel's own words verbatim, a full revision that replaces the one before it — and it sits above the in-flight content, since a plan is what the agent is doing now rather than a thing it said. A mode change is a safety fact about the session, drawn as a mono `--text-label` `--ink-3` line carrying the kernel's own word, never mapped onto a set this document invented. Both ride the durable log through `STATE_DELTA`, so a reload shows the same stage and the same mode as a turn watched arriving, on §18.1's equivalence rule. |
| 2026-08-19 | Amended §18.6 for a plane change that keeps its sandbox. A plane change that replaces the pod still draws the boundary, because something really did not travel; a plane change that restarts the program inside the same pod draws no boundary, because nothing moved. A boundary drawn where nothing was lost teaches an owner to distrust boundaries that mean something, so the surface draws one only where the record carries a snapshot timestamp, and none where it does not. |
| 2026-08-19 | Wrote §18.10, which the changelog announced on this date and nobody wrote. The surface had been drawing a stage, a mode change and a kernel's command list ever since, which put all three outside this document's authority — the thing §7 exists to prevent, arriving through a changelog entry rather than through code. Written against what the kernel already sends: a stage is the agent's plan as a `--sunken` list of steps in the kernel's own words, above the in-flight content because a plan is what the agent is doing now rather than a thing it said, replaced whole on each revision since merging two would invent a plan neither described; a mode change is one mono `--text-label` `--ink-3` line carrying the kernel's own word, drawn where state is drawn and never in the reading column, because a reader scanning the agent's words must not meet a sentence the agent did not write. Moved the command list out of the stream, where it was printed below every turn and could be read but not used — the reader retyped what they had just read. Added §18.11 What a reader can take away: every fenced block carries a header with the fence's info string rendered verbatim and uppercased by `text-transform` only, since casing is typography and mapping `py` to `Python` is naming, which §18.2 and §18.10 both forbid; a fence with no info string draws no label rather than an invented one; the header is sticky against the stream's scrollport on a wrapper outside the `<pre>`, because the `<pre>` owns the horizontal overflow and a sticky child of a horizontally scrolling box slides away sideways. Every settled turn carries a copy control that writes the turn's **markdown source** rather than the rendered tree, which is the point and not an implementation note: selection-copy already works and already yields degraded text, because the renderer strips heading hashes, list markers, fences and backticks before drawing, so somebody pasting an agent's steps into an issue or its command into a shell gets something the agent did not write. Persistently drawn and labelled rather than revealed on hover, since §3 permits no hover-gated affordance and nothing in this interface moves on hover; disabled with its reason stated while the turn is in flight rather than removed, because copying half an answer is the thing to prevent and removal is the wrong instrument for it. Added §18.12 What an agent wrote: links drawn but carrying **no colour** — underline and cursor and nothing else, since §18.1 forbids colour on agent content without exception and a blue link is that exception arriving by habit — with §9's OSC 8 rule as the precedent for not deciding where an agent's link points; tables in the structure they were written in, scrolling inside their own frame rather than widening the reading column; nested lists keeping their nesting, because a plan with sub-steps flattened to one level is a different plan; blockquotes on a `--rule` left border in `--ink-2`; and a soft line break drawn as a line break, because an agent that wrote its steps on separate lines meant them on separate lines and joining them rewrites the shape of the answer. Added §18.13 When the surface cannot hear the session, whose finding is that the failure mode is **asserting liveness that is false** rather than failing to narrate the transport: while the stream is down mid-turn nothing but the run ending clears the in-flight state, so the surface goes on drawing the §11 wave and blinking the caret on a message that has stopped growing, and the one affordance built to tell a working agent from a dead stream is the one that lies. A transient reconnect says nothing, on the line the terminal plane already holds; the moment the link is not live the liveness claim is withdrawn, because withdrawing a false assertion is quieter than adding a true one and is correct at every point in between; and only a link down past a bounded threshold is named, once, where the turn is rather than in a band across the head. None of it is a failed run — §18.9 draws a run the agent could not finish, this draws a surface that cannot hear one, and conflating them tells an owner their work broke when their network did. Amended §18.2 so each body the log carried is its own region with its own scrollport under the existing cap: what a call was asked to do and what it produced are two bodies, and one window with one scroll position means a long argument payload pushes the result out of sight, so the reader scrolls past what they already knew to reach the thing they opened the row for — which is the scrollback the section exists to replace. Amended §18.4 so the kernel's commands are reachable from the field: `/` at the head of an empty draft opens a filtered list built from what the kernel advertised and nothing else, reusing §3's existing plate behaviour, and a kernel advertising none opens nothing. They stay liveness rather than history, which is why they may not be drawn in the stream: a stale list replayed from the log would offer commands that no longer exist, against §18.1's equivalence rule. |
