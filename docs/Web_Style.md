# Web Style

**This document is the single source of truth for all chrono-bot frontend design and style.**

No colour, spacing value, type scale step, component pattern, or interaction rule may be introduced in `web/` unless it appears here. If a design need is not covered, extend this document first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

---

## 1. What the frontend is

chrono-bot's frontend is **a terminal, and the chrome needed to run several of them**.

The terminal is the work. Everything else earns its place by making a terminal easier to reach, to identify, or to stop. There is one page frame, two views inside it, and no navigation beyond them.

| View | What it is |
|---|---|
| **sessions** | The default, and where the work happens. A session list, one terminal, and what is known about it. |
| **operations** | Running the service: who is using it, what it has been asked to do, what is running, and stop that. Drawn only for a caller holding an operator capability. |

Sessions has **two arrangements** of the same list — `tabs`, where one terminal owns the column, and `canvas`, where every listed session is a framed node with its own terminal. An arrangement is not a view: it changes how the middle column is drawn and nothing else. The control for it lives with the list it rearranges.

The unauthenticated state is the same terminal, driven by an authored introduction instead of a kernel.

Design consequences, binding:

- **The terminal owns whatever width is left.** It is the last surface to give up space: the details sheet closes first, then the rail collapses, then the rail leaves the layout.
- **One surface, many drivers.** The introduction and a live session are the same engine driven by different sources. A visual difference between them that is not a difference in *content* is a defect.
- **Fidelity beats decoration inside the viewport.** Agent-emitted output is rendered faithfully. Styling applies to the frame, the chrome and the authored introduction — never to output produced by a kernel.
- **Never build an interactive feature here that belongs in the API.** If a client would want it, it belongs in `api/`.

---

## 2. Colour

Defined as CSS custom properties on `:root`. Components reference tokens only — never a literal colour.

Dark is the primary expression. Light is fully specified and measured, not an afterthought. The two share the accent *fill* and differ in the accent *ink*, because a colour bright enough to read on near-black cannot carry text on paper.

### Light (default)

| Token | Value | Use |
|---|---|---|
| `--ground` | `#F7F7F5` | page background |
| `--surface` | `#FFFFFF` | chrome: bars, bands, panels |
| `--raised` | `#F7F7F5` | selected row |
| `--inset` | `#ECEAE6` | fields, tracks |
| `--ink` | `#121214` | primary text |
| `--ink-2` | `#52525B` | secondary text |
| `--ink-3` | `#686871` | labels, metadata |
| `--rule` | `#E2E2DE` | hairline borders |
| `--rule-2` | `#CACAC4` | control borders |
| `--rule-3` | `#B5B5AE` | the lit edge of a raised control |
| `--accent` | `#46620B` | interactive text and borders |
| `--accent-fill` | `#C6F24E` | the accent key's fill |
| `--accent-edge` | `#7C9E27` | its impression |
| `--on-accent` | `#121214` | text on the accent fill |

### Dark

Redefined under both `@media (prefers-color-scheme: dark)` guarded as `:root:not([data-theme='light'])` and `:root[data-theme='dark']`, so an explicit choice wins in either direction.

| Token | Value |
|---|---|
| `--ground` | `#0A0A0B` |
| `--surface` | `#0F0F11` |
| `--raised` | `#121214` |
| `--inset` | `#141417` |
| `--ink` | `#F2F2F3` |
| `--ink-2` | `#A0A0A8` |
| `--ink-3` | `#8A8A94` |
| `--rule` | `#232327` |
| `--rule-2` | `#34343B` |
| `--rule-3` | `#45454E` |
| `--accent` | `#C6F24E` |
| `--accent-fill` | `#C6F24E` |
| `--accent-edge` | `#8FAE3B` |
| `--on-accent` | `#0A0A0B` |

**One accent.** The cyan/magenta pair is retired: two accents meant every surface had to decide which one it was, and the answer was decorative. `#C6F24E` marks what is live and what to press, and nothing else.

### Semantic

Session and system state. Separate from the accent; never used decoratively.

| Token | Light | Dark | Meaning |
|---|---|---|---|
| `--live` | `#46620B` | `#C6F24E` | ready, running, healthy |
| `--wait` | `#8A5A00` | `#F2B23E` | starting, idle, rotating |
| `--fail` | `#A12626` | `#FF6B6B` | failed, refused, invalid |
| `--ended` | `#686871` | `#8A8A94` | ended, unknown, retired |

### Measured contrast

Worst case per theme, computed per WCAG 2.1, against the surface the token actually sits on. Body text requires 4.5:1; no pair falls below it in either theme.

| Theme | Worst text pair | Ratio |
|---|---|---|
| Dark | `--ink-3` on `--inset` | 5.38:1 |
| Dark | `--ink-3` on `--surface` | 5.60:1 |
| Light | `--ink-3` on `--inset` | 4.59:1 |
| Light | `--wait` on `--surface` | 5.93:1 |
| Light | `--accent` on `--ground` | 6.49:1 |

`--ink-3` is the binding constraint in both themes: it carries 11px uppercase metadata, which is not large text, so the full 4.5:1 applies. Do not darken it in dark or lighten it in light. A new colour requires a changelog entry and a recomputed row.

### Rules

- Every colour is defined on bare `:root` first. A colour whose only definition lives inside a media query or `[data-theme]` block is a bug.
- `body` always sets an explicit `background` from a token.
- Semantic colour never carries meaning alone — always paired with the state word.

---

## 3. Depth

Depth is **letterpress**: a hard offset shadow with zero blur, as if the control had been stamped into the page. There are no soft drop shadows, no gradients beyond a fill's own hue, no glass and no coloured glow.

### Tokens

| Token | Dark | Light |
|---|---|---|
| `--press-accent-rest` | `4px 4px 0 0 var(--accent-edge)` | same |
| `--press-accent-hover` | `2px 2px 0 0 var(--accent-edge)` | same |
| `--press-flush` | `0 0 0 0 transparent` | same |
| `--press-quiet-rest` | `3px 3px 0 0 rgb(242 242 243 / 0.24)` | `3px 3px 0 0 rgb(18 18 20 / 0.26)` |
| `--press-quiet-hover` | `1.5px 1.5px 0 0 var(--accent)` | same |
| `--press-card-rest` | `5px 5px 0 0 var(--press-ink), inset 0 0 0 1px var(--rule)` | same |
| `--press-card-hover` | `2px 2px 0 0 var(--press-ink), inset 0 0 0 1px var(--rule-2)` | same |
| `--press-ink` | `rgb(242 242 243 / 0.10)` | `rgb(46 62 11 / 0.15)` |
| `--press-none` | `inset 0 0 0 1px var(--rule)` | same |
| `--focus-ring` | `0 0 0 2px var(--ground), 0 0 0 4px var(--accent)` | same |

**These are the only shadow values permitted.** An inline `box-shadow` string in a component is a bug.

Dark uses a bone ink for the card impression rather than black: on `#0A0A0B` a black offset is invisible, and an impression nobody can see is a shadow that costs paint for nothing.

### The state contract

| State | Shadow | Transform |
|---|---|---|
| rest | the element's `*-rest` | none |
| hover | its `*-hover` — the shadow **shrinks** | translate 1.5–2px down and right |
| active | `--press-flush` | translate 3–4px |
| disabled | `--press-none` | none |
| focus | the state's shadow **plus** `--focus-ring` | unchanged |

Hover presses **into** the impression. Nothing in this interface lifts toward the cursor.

**A disabled control carries no impression.** The impression is what says a thing can be pressed; a greyed key that still looks pressable reads as broken rather than unavailable.

### What gets depth

- **Raised** — anything you press: keys, filter chips when on, the selected segment.
- **Flat** — the page, its bars and bands, and anything you type into or read: fields, selects, tracks, status chips, record rows.
- **Card** — a thing among peers, 5px. Interactive cards press in on hover; static panels do not.
- **Plate** — anything floating over the page: dialogs, menus, the command bar. Same 5px impression, and it never moves.

The one glow the product has is a 6px halo on the dot of a **live** session. Nothing else glows, and no glow is ever applied to a terminal viewport or to a character a kernel emitted.

---

## 4. Typography

| Role | Stack |
|---|---|
| Platform — headings, labels, prose, controls | `'Geist', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif` |
| Data — terminal, times, identifiers, metadata | `'Geist Mono', ui-monospace, 'SF Mono', Menlo, Consolas, monospace` |

Both faces are **self-hosted** under the OFL in `web/src/assets/fonts/`, with `font-display: swap` and a system fallback. Nothing is fetched from a third party: a font request to someone else's host can be logged, blocked or slow. Geist Pixel is retired — the product mark is drawn geometry now and needs no glyph (§17).

### Scale

| Step | Size | Use |
|---|---|---|
| `2xs` | 10px | status chip words, band micro-labels |
| `xs` | 11px | every band label and value, all metadata |
| `sm` | 12px | secondary prose, hints |
| `base` | 13px | controls, terminal |
| `md` | 14px | body, menu items, record primaries |
| `lg` | 15px | a band's subject — the session time |
| `xl` | 17px | page and dialog titles |
| `2xl` | 20px | empty-state leads |
| `3xl` | 28px | section statements |
| `display` | 62px | the landing headline only |

Uppercase labels take `letter-spacing: 0.14em`. Display type takes `-0.03em`. Headings take `text-wrap: balance`, prose takes `text-wrap: pretty`, and any column of digits takes `font-variant-numeric: tabular-nums`.

**Terminal type** keeps its own metrics: 13px, line height 1.6, letter spacing 0. Never track a terminal.

---

## 5. Layout

4px base unit. Permitted spacing: `4, 8, 12, 16, 20, 24, 32, 40, 56`.

**The 10% page inset is removed.** It cost the terminal roughly 290px on a 1440px window and made the bar look detached from a page it was already aligned to. Page content runs to a 20px gutter, and alignment is achieved by the band grid instead.

**The page does not scroll.** The app fills the viewport; the terminal owns its scrollback, and a column or a canvas owns its own overflow.

### The band grid

Every column's header is one **head band** tall and every column's footer one **foot band**, so when two panels sit side by side their hairlines land on the same line and the page reads as one surface.

| Token | Value |
|---|---|
| `--bar-height` | 56px (48px below 30rem) |
| `--band-head` | 52px |
| `--band-foot` | 40px |
| `--page-gutter` | 20px |
| `--rail-width` | 312px |
| `--rail-collapsed` | 48px |
| `--sheet-height` | 412px |

Binding rules:

- A band's height is **set on the band**, never accumulated from padding. Padding is what lets two aligned columns drift apart when one gains a longer label.
- A column needing more chrome adds a **second band beneath the first**. It never grows its header.
- **One type size lives in a band** — mono `xs`, tracked and uppercase for a label, plain for a value. The single exception is a band's subject, the session time, at `lg`.
- One documented exception to the row shape: the rail's second band **stacks** its new-session key over its filters. It sits below the header band, so the header rule still runs straight across every column.

### Radius

`2px` everywhere. A hard offset shadow wants a sharp corner; anything rounder reads as a different material.

---

## 6. Page frame

**One app bar and one status band, on every page**, so the window's top and bottom edges never move as somebody switches view.

The **app bar** (56px) carries the mark, the view switch, the command field, and the account control. It is not navigation and must never become it.

The **status band** (40px) always reads in one order: the display zone, then what this page is showing, then the build. It is the page's footer; in a multi-column view its 40px is shared by the columns' own footers, and the rule still runs across.

---

## 7. Controls

Height is declared on the control, and every control is `box-sizing: border-box` — a quiet button carries a 1px border and an accent button does not, so under content-box the two render 2px apart and sit a pixel out of line in the same row.

| Token | Value | Use |
|---|---|---|
| `--control-h` | 32px | anything in a band or a dialog footer |
| `--control-h-sm` | 28px | controls inside a record row or a card footer |
| `--control-h-block` | 36px | a panel's one full-width primary action |
| `--control-h-lg` | 44px | the landing's opening actions |
| `--chip-h` | 22px | status chips |
| `--segment-h` | 26px | segmented options |
| `--hit` | 32px | minimum target (44px below 30rem) |

Two button sizes and no others. **One accent button per view**, spent on the action the view exists for; everything else is quiet. Destructive actions take `--fail` and confirm inline, in place of the row's own controls — never a second dialog over a dialog.

**Controls are disabled, never removed**, and every disabled control states why. A two-line stacked key carries its reason as part of the button, because a tooltip is unreachable by touch and invisible to anyone scanning. Stacked keys hold a 52px minimum so a column of them stays even when one reason wraps.

### Dropdowns

A dropdown is an **instrument closed and a plate open**: hairline and fill at rest with no impression, since it is not a key; the open menu floats and takes `--press-card-rest`. 32px tall, caret at `2xs` in `--ink-3`, and the caret **flips** rather than rotating — at that size a rotation reads as a wobble. A filter dropdown holds a 160px minimum so its row does not resize as the value changes. The selected option is marked on its leading edge **and** by a tick.

Choosing a **value** is a select; choosing an **action** is a menu. They look alike and behave differently, so a select never runs anything when it opens.

### Cards side by side

A row of peer cards is a **grid**, never a flex row of hand-set widths: columns are equal by construction and every card stretches to the tallest, so content length can never decide how big a card is. A card whose text overruns truncates or scrolls inside itself rather than growing and leaving its neighbours short.

### Collapsing a panel

A collapsed panel becomes a **handle, never nothing**: 48px of strip that keeps its header and footer bands, so the way back is exactly where the panel was and the page's rules still line up. The strip carries a **summary** of what it hides — the session dots in state order and a count. Collapsing gives up detail, not awareness.

The handle glyph points where the panel will go — `‹ ›` for a column, `⌃ ⌄` for a sheet — and reverses when collapsed, so the control describes its outcome rather than its state. The choice persists per browser and is validated on read. Keyboard: `⌃← / ⌃→` for a column, `⌃↑ / ⌃↓` for the sheet, and focus moves to the handle so nobody is stranded on a control that just left.

---

## 8. Interaction

- Every interactive element has a visible focus state: `--focus-ring`, stacked over whatever impression the state carries. Never removed.

  **One documented variant: the terminal prompt row.** A 2px ring around a full-width borderless field boxes in the whole prompt line and breaks the illusion the surface depends on, so for that input the indicator is a 2px `--accent` bar on the row's leading edge (`box-shadow: inset 2px 0 0 var(--accent)`), reinforced by the accent caret. A relocation, not a removal, scoped to that one input.

- Transitions are 140ms on `transform` and `box-shadow`, 200ms on colour, and every one is wrapped in `@media (prefers-reduced-motion: no-preference)`. Under reduced motion the translate is suppressed and the shadow swap alone communicates the press — nothing is lost.
- Loading is a **skeleton of the shape that is coming**, or an inline indicator. Never a full-page spinner.
- Errors state what went wrong and what to do about it. No apologies, no raw codes without a human sentence.

---

## 9. Terminal

The byte stream originates from a coding agent executing the user's own untrusted code. Escape sequences are an attack surface, and the unauthenticated introduction shares the engine, so the **engine** carries the hardening.

- **OSC 52 (clipboard write) is disabled.**
- Escape sequences must not set the tab title or trigger any host-level action.
- Scrollback is capped. Unbounded output must not exhaust the tab.
- OSC 8 hyperlinks are validated before being made clickable.

The 16-slot ANSI palette is derived from the tokens and is authoritative; agent output is rendered through it and not otherwise restyled. The selectable terminal themes remain as specified: vendored as data, filtered by the current appearance, scoped to the terminal canvas and never to the surrounding interface.

---

## 10. Accessibility

- Text contrast meets WCAG AA in **both** themes. Measured ratios are recorded in §2.
- Semantic HTML before ARIA. Landmarks on every page.
- Every view is fully keyboard operable, including the terminal, which must be escapable by keyboard alone.
- Never convey state through colour alone.
- **The animated introduction is never the only route to its information.** The viewport is exposed to assistive technology as a static, complete transcript — a screen reader user never waits for an animation and never receives a character-by-character stream.
- Focus on load rests on the prompt.
- Minimum target 32px, 44px below 30rem.

---

## 11. Session states and what they allow

A session is shown in one of six states, and the state decides what the interface offers. **One rule governs all of it** — whether the terminal accepts typing, whether a control is enabled, and whether the session appears in the list.

| State | Shown as | Terminal | Available |
|---|---|---|---|
| provisioning | starting | progress, no input | end |
| ready / running | ready / running | live | reconnect, end, delete |
| rotating | updating | progress, no input | nothing |
| idle | idle | history, read-only | **revive**, delete |
| terminated | ended | history, read-only | delete |

**A session that cannot be typed into always says why**, in the owner's terms, on the surface showing it — not only in a panel that may be describing something else.

**A session with no sandbox still shows its history**, read-only. Stopping releases what it costs; it does not hide what it did. Keystrokes into a read-only terminal are dropped rather than buffered.

**Ended sessions leave the list, except when they failed.** A session that died on its own is the one its owner is looking for.

**Revive is the primary action wherever it is offered.**

### Waiting

Provisioning takes seconds on a warm node and a minute on a cold one. The wait is animated as a left-to-right character wave — 1.4s loop, 45ms stagger, 0.28em lift — because a spinner is forbidden and a wave makes no claim about progress. Beside it, a bar reflecting **steps the control plane actually reported**, with the step named in words. It never advances on a timer: a bar that moves without anything happening costs the user their trust in every other bar. Under reduced motion the animation is removed and the message stays.

**A session that ends badly says why.** Terminating normally and failing to start are different events and must not read the same.

---

## 12. Sessions — the tabs arrangement

Three columns: the session list, the terminal, and what is known about the session.

**The rail lists sessions by when they started, in Singapore time**, newest first and reversible, with the date shown only once a session is no longer from today. A fixed zone means the same session reads the same to everyone who sees it, and the zone is stated in the status band rather than assumed.

A rail row carries a status dot, the time, **one line saying what the session last did**, and its kernel and profile. The last-activity line is what makes a list of times into a list of work; it never wraps.

**The details panel is a sheet under the terminal, not a column beside it.** It is reference material: read when somebody wants to know which agent is answering or why a session stopped, and the rest of the time it is width the terminal should have. Open, it is 412px and its facts sit wide rather than tall; closed, it is a 40px band carrying a one-line summary. Its columns scroll inside it.

**A session's lifecycle controls live in the session header**, at the head of the terminal they act on — visible without opening anything. §11 decides which are enabled; each disabled one states its reason.

**A failed action is reported above the surface, never in place of it.** One `--fail` band with `role="alert"` at the head of the column, outside whatever the column is showing. Placement is the substantive part of this rule: a failure belongs to one request about one session, and the surface beneath it may hold a dozen live terminals — replacing it would cost eleven sessions their connection to say something about the twelfth. **Nothing that reports a failure may destroy work that did not fail.** The band reports the most recent attempt and every action clears it as it starts.

---

## 13. Sessions — the canvas arrangement

Every listed session is a framed node with its own terminal on one pannable surface. **The canvas replaces the middle column and nothing else**: the bar, the rail and the sheet keep their places, because they are the same facts in either arrangement.

**One selection, two arrangements.** The session the rail marks active and the node the canvas focuses are the same fact. Changing arrangement never changes which session is selected.

### Node framing

| Property | Value |
|---|---|
| Size | `--canvas-node-w` 420px × `--canvas-node-h` 280px |
| Gap and surface padding | `--space-4` |
| Background | `--surface`, body on `--ground` |
| Border | 1px `--rule` |
| Shadow | `--press-card-rest` |

**Fixed size, deliberately.** A node's size *is* the geometry its runtime is told to draw at: a 420×280 node asks for roughly 47 columns, and a node that changed width as siblings came and went would re-wrap everything the agent has drawn. Output already wrapped at 47 columns stays wrapped at 47 columns, on the canvas and in the transcript afterwards. Choosing the canvas therefore resizes every attached session once, and that does not undo — which is a real cost of this arrangement and must not be described as a display preference that stops at the browser.

The grid fills whole columns and overflows in both directions, so panning means something in both.

Each node carries a header above its terminal on a 1px `--rule` underline: the start time in `sm`, **the same label the rail uses**, then the kernel and the lifecycle word in `2xs` uppercase. Inside a node the terminal drops its own border — the node frame is that border.

### Focus, and how it differs from selection

Exactly one node is focused. **The focused node is the only one that receives keyboard input** — a correctness property, not a styling one.

| State | Treatment |
|---|---|
| Resting | 1px `--rule`, `--ink-2` header |
| Selection — pointer over the node | 1px `--rule-2`. Border weight only. |
| Focus — this node takes the keys | 1px `--accent`, the impression in accent, `--ink` header, `aria-pressed` on its handle |

Hover changes weight within the neutral ramp; focus changes hue. **Focus is never a second opinion on §11**: a provisioning, rotating, idle or ended session stays read-only however plainly it is being looked at, and its node says `read-only` in words in its header.

**Unfocused terminals are never dimmed, tinted, greyed or covered.** They are attached and rendering live output.

### Keyboard, pointer, panning, cost

- The canvas is one tab stop: the focused node's header handle. Arrows move by one and by a row, Home and End jump to the ends, movement clamps and never wraps. Arrows are **not** intercepted inside a terminal — there they are shell history. Tab from the handle enters the terminal. A move is announced through a polite live region naming the session and its position.
- A gesture on a node's chrome focuses it and parks the caret on its handle; a gesture on its terminal focuses it and leaves the caret in the terminal, so one click is enough to type. Focus placement is always explicit.
- The viewport scrolls and drags within itself; the page still does not scroll. A drag inside a node belongs to that node's terminal.
- **At most 12 nodes are attached at once.** Sessions past the budget appear as framed, labelled nodes with no terminal, saying so and inviting selection; focusing one attaches it and detaches whichever attached node has gone longest without focus. **Truncation is never silent** — the status band states how many are unattached and why.
- The only motion is the 140ms border and shadow transition when focus moves.

---

## 14. Operations

**A page, not a dialog.** It is the one surface that shows many rows at once, and a 560px dialog was why its rows truncated. As a page its four views also have somewhere to live without stacking chrome inside a modal.

**The view is absent unless the control plane says otherwise.** The identity surface reports the caller's capabilities and the switch offers `operations` only to a caller holding one this page can act on. This is presentation, never protection — the API refuses on its own and would refuse a hand-written request just the same. Both halves are required.

**Each view is offered on its own capability**, matching the API. A caller holding two sees two. A view is never drawn empty because of entitlement: an empty list must always mean "there is nothing", or the page teaches people to misread it.

The four views are switched by a tablist in the page's second band: mono `xs` uppercase labels, selected by `--accent` text **and** a 2px `--accent` bottom border **and** `aria-selected`.

### Record list

Rows, not a table: rows are read one at a time and a table's column rules earn nothing here.

- A leading tabular-nums timestamp, a `md` primary label, `xs` `--ink-3` mono metadata beneath.
- Hairline separators, no zebra striping, no hover highlight, no row-level click target. Nothing on this page navigates.
- **Newest first, always.** Filters narrow without ever changing the order.
- An empty list says what would have been there, in a sentence.

### Naming people

**A person is their name, then their email — never their subject identifier.** Where the directory knows nothing about a subject, the identifier is shown in identifier style so a reader can tell they are looking at an identifier. An action with no established identity reads "Not signed in", which is a fact, rather than an empty cell, which is an omission.

### Unknown is not empty

A provider that could not be asked renders as **unknown**, carrying the reason, and never as a provider running nothing. The running total is withheld entirely while any provider is unobservable: a partial total presented as a total is the same lie as an empty list. This mirrors the API contract and must not be simplified for tidiness.

### Acting on somebody else's session

| Control | Confirms | Available when |
|---|---|---|
| Stop | no | the session has a runtime to release |
| Terminate | yes, inline | the session has not already ended |

Both are disabled with a stated reason rather than removed. Deleting somebody else's session is not offered at all: the API does not expose it, and stopping is reversible where destroying a history is not.

---

## 15. The signed-out surface

A statement, and a terminal that already works.

The prompt is a real `<input>`, mounted and focused from the first frame, so time-to-interactive is zero and the introduction is decoration over an already-usable field. A visitor who types at 300ms keeps every character. The visible text **is** `input.value` — no mirror element, so the whole class of mirror-desync defects cannot occur.

| Property | Value |
|---|---|
| Machine lines | printed whole, 75ms stagger |
| Prose | typed, 20ms per character |
| Pauses | 180ms after `.` `?` `!`, 90ms after `,` and `—`, 140ms at a line break |
| Caret blink | 530ms |
| **Time to interactive** | **0ms; introduction complete under 3.2s, hard cap 4s** |

- The animation is **skippable**: any keypress or click completes it, and a dim hint says so after the boot block.
- The sign-in affordance appears only once the visitor has typed a character. Do not nag an empty prompt.
- Their text survives the round trip and becomes the first turn of their first session.
- `prefers-reduced-motion: reduce` — all text present instantly, no typing, no blink, no skip hint. Fully usable, not a degraded fallback.

The page carries the same app bar and status band as every other page: a visitor and an owner are looking at one product.

---

## 16. Responsive

The number of columns changes; the bands never do. 52px header and 40px footer hold at every width, so the page reads as the same shape on a phone as on a desk.

| Width | Layout |
|---|---|
| ≥ 75rem | Three columns. Nothing collapses on its own. |
| 60–75rem | The rail arrives collapsed to its 48px strip — still a handle, dots still reporting. |
| < 60rem | The rail leaves the layout and becomes a drawer over it. The terminal keeps the full width. |
| < 30rem | The app bar drops to 48px and gives the switch and the command field to the drawer. Every target grows to 44px. The status band keeps the zone and the count and drops the build. |

**The canvas arrangement is offered at 60rem and above only.** A grid of terminals in a narrow window is several terminals nobody can read; below that the toggle is disabled with its reason, per §11, rather than removed.

---

## 17. The mark

Two-by-two, one cell lit. It is a clock quarter and a fleet with one session live at the same time, and it inherits the checkered logic of the glyph it replaces.

Built from four squares, so it needs no font, no SVG and no asset, survives a 16px favicon, and prints in one colour. The gap scales with the mark so the cells never fuse: 8px at 96, 4px at 40, 2px at 18, 1px at 16. Knockout, on-accent and light variants are the same geometry in two colours.

The wordmark is Geist 500 at `-0.03em`, lowercase. `chrono-bot` in mono stays for anything naming the repository or the API.

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
| 2026-08-17 | Ground control: full redesign of the frontend's visual language and layout, replacing the cyberpunk terminal-first system established 2026-08-13. Retired the cyan/magenta pair for one lime accent with a measured light counterpart, and reworked the surface ramp into ground/surface/raised/inset with worst-case contrast recorded per theme (dark `--ink-3` 5.38:1 on inset; light `--ink-3` 4.59:1 on inset). Replaced the bounded glow system with letterpress depth — hard-offset impression shadows with zero blur, hover translating into the impression, active flush, disabled carrying none, focus stacking a page-halo and accent ring over whichever impression the state holds — leaving one 6px halo on a live session's dot as the only glow in the product. Adopted Geist and Geist Mono, self-hosted, and retired Geist Pixel in favour of a mark drawn from four squares. Removed the 10% page inset, which cost the terminal ~290px at 1440px, and replaced it with a band grid: a 52px head band and a 40px foot band whose heights are set rather than accumulated from padding, one type size per band, and a second band for any column needing more chrome. Established one app bar and one status band on every page; two control heights and no others, every control `box-sizing: border-box` so a bordered and an unbordered control measure alike; peer cards as grids so content length cannot decide card size; one dropdown pattern, instrument closed and plate open; and a collapse pattern in which a panel becomes a 48px handle carrying a summary rather than disappearing. Reframed the frontend as one page frame with two views — sessions and operations — where the canvas is an arrangement of sessions rather than a view of its own, and moved operations out of a 560px dialog onto a page for the one surface that shows many rows at once. Kept the canvas node fixed at 420×280 and restated why: a node's size is the geometry its runtime draws at, so a fluid grid would re-wrap everything an agent has printed. Promoted the session lifecycle controls into the session header, demoted the details panel to a 412px sheet beneath the terminal, and added a last-activity line to each rail row. Added §16 Responsive with four widths, and the rule that the terminal is the last surface to give up width. Set light `--ink-3` and `--ended` to `#686871`: at the `#6B6B74` the handoff carried, both measured 4.39:1 on `--inset` and failed the 4.5:1 floor this section sets for the 11px uppercase metadata `--ink-3` exists to carry. |
