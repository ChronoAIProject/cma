# Web Style

**This document is the single source of truth for all chrono-bot frontend design and style.**

No colour, spacing value, type scale step, component pattern, or interaction rule may be introduced in `web/` unless it appears here. If a design need is not covered, extend this document first, in its own pull request, then implement.

The changelog at the end is **append-only**. Entries are never edited or deleted.

---

## 1. What the frontend is

chrono-bot's frontend is **two planes onto one session, and the chrome needed to run several of them**.

A session is reached on one of two planes, drawn in the same middle column and chosen by what that session was granted:

| Plane | What it is |
|---|---|
| **terminal** | The kernel's own interface, entire and unmediated, exactly as its authors built it. The raw seat at the agent. |
| **conversation** | The structured seat: approvals as buttons rather than keystrokes, tool calls as structure rather than scrollback, usage as a number, history that survives the screen. |

Same agent, same conversation — not two products and not two histories. Conversation is the general surface and the terminal is the escape hatch, not a friendlier skin over the same thing; and conversation is the *only* plane a kernel without a terminal can offer at all.

**A plane is not a view**, exactly as an arrangement is not a view. It changes what the middle column draws and nothing else. The views stay two, there is one page frame, and there is no navigation beyond them.

| View | What it is |
|---|---|
| **sessions** | The default, and where the work happens. A session list, one surface, and what is known about it. |
| **operations** | Running the service: who is using it, what it has been asked to do, what is running, and stop that. Drawn only for a caller holding an operator capability. |

Sessions has **two arrangements** of the same list — `tabs`, where one surface owns the column, and `canvas`, where every listed session is a framed node with its own. An arrangement is not a view: it changes how the middle column is drawn and nothing else. The control for it lives with the list it rearranges.

The unauthenticated state is the same terminal, driven by an authored introduction instead of a kernel.

Design consequences, binding:

- **The surface owns whatever width is left**, terminal or conversation. It is the last to give up space: the details sheet closes first, then the rail collapses, then the rail leaves the layout.
- **One surface, many drivers.** The introduction and a live session are the same engine driven by different sources. A visual difference between them that is not a difference in *content* is a defect. This does more work than it used to: the conversation plane is driven by a recorded transcript in development and by a live event stream in production, and a component that can tell the two apart is a component that has learned something about its driver it had no business learning.
- **Fidelity beats decoration inside the viewport.** Agent-emitted output is rendered faithfully. Styling applies to the frame, the chrome and the authored introduction — never to output produced by a kernel. This extends to agent-emitted message content on the conversation plane: the code blocks, lists and inline formatting an agent wrote are rendered as it meant them, and the frame around them is what is styled.
- **The conversation surface may call no route a third-party client could not call.** It exists because everything it does is already in AG-UI. The moment it needs something AG-UI lacks, the answer is to add it to AG-UI — never to add it here.

  This replaces "never build an interactive feature here that belongs in the API", and is stricter than the sentence it replaces: that one asked whether a feature *belonged* in the API, which is a judgement, and this asks whether the API *already projects it*, which is checkable. `acp-kernel` claims a product frontend can build a chat interface on the public interfaces, and a first-party client held to the same constraint is the only thing that keeps that claim honest — a client permitted a private route would prove nothing about the one a third party has to build.

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

A session is shown in one of six states, and the state decides what the interface offers. **One rule governs all of it** — whether the surface accepts input, whether a control is enabled, and whether the session appears in the list.

The table covers **both planes**, because a state means the same thing on each and a second table would be a second place for them to disagree.

| State | Shown as | Terminal plane | Conversation plane | Available |
|---|---|---|---|---|
| provisioning | starting | progress, no input | progress, composer refuses | end |
| ready / running | ready / running | live | live | reconnect, end, delete |
| rotating | **updating** | progress, no input | **progress, composer refuses, and says why** | nothing |
| idle | idle | history, read-only | history, read-only — **unless it is waiting on an approval** | **revive**, delete |
| terminated | ended | history, read-only | history, read-only | delete |

**A session that cannot be typed into always says why**, in the owner's terms, on the surface showing it — not only in a panel that may be describing something else.

**A session with no sandbox still shows its history**, read-only. Stopping releases what it costs; it does not hide what it did. Keystrokes into a read-only terminal are dropped rather than buffered, and a composer on a read-only session refuses rather than accepting text it will not send.

### `rotating` is the one the interface must get right

`API_Overview.md` §2 does not describe this state, it instructs: *"Clients must refuse input while it lasts — a keystroke sent now reaches a sandbox being torn down, so it would be accepted and then lost."* `CLAUDE.md` §5 puts it more bluntly: accepting input that would be lost with the outgoing pod is worse than briefly refusing it.

Refusing is the requirement. **Saying why is the design**, and it is on the surface rather than in a panel, because a composer that has quietly stopped working is the thing people report as broken.

### `idle` and `recovering` are not failures

`idle` with `stop_reason: requires_action` is a session waiting for its owner, and is the most alive a session gets. `recovering` with reason `runtime_absent` is normal during an idle-to-revive cycle. `unknown` with `runtime_observation_unavailable` means the control plane cannot see the runtime *right now* — not that the session is dead.

None of the three takes `--fail`. Drawing them as errors trains owners to ignore real ones.

**Ended sessions leave the list, except when they failed.** A session that died on its own is the one its owner is looking for.

**Revive is the primary action wherever it is offered.**

### Waiting

Provisioning takes seconds on a warm node and a minute on a cold one. The wait is animated as a left-to-right character wave — 1.4s loop, 45ms stagger, 0.28em lift — because a spinner is forbidden and a wave makes no claim about progress. Beside it, a bar reflecting **steps the control plane actually reported**, with the step named in words. It never advances on a timer: a bar that moves without anything happening costs the user their trust in every other bar. Under reduced motion the animation is removed and the message stays.

**A session that ends badly says why.** Terminating normally and failing to start are different events and must not read the same.

---

## 12. Sessions — the tabs arrangement

Three columns: the session list, **the session's surface**, and what is known about the session. Which surface is drawn is the session's to decide, not the arrangement's — §18.6.

**The rail lists sessions by when they started, in Singapore time**, newest first and reversible, with the date shown only once a session is no longer from today. A fixed zone means the same session reads the same to everyone who sees it, and the zone is stated in the status band rather than assumed.

A rail row carries a status dot, the time, **one line saying what the session last did**, and its kernel and profile. The last-activity line is what makes a list of times into a list of work; it never wraps.

**The details panel is a sheet under the surface, not a column beside it.** It is reference material: read when somebody wants to know which agent is answering or why a session stopped, and the rest of the time it is width the surface should have. Open, it is 412px and its facts sit wide rather than tall; closed, it is a 40px band carrying a one-line summary. Its columns scroll inside it.

**A session's lifecycle controls live in the session header**, at the head of the surface they act on — visible without opening anything. §11 decides which are enabled; each disabled one states its reason.

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

## 18. The conversation surface

The other plane a session can be drawn on (§1). Everything here is built from the tokens and patterns §2–§8 already define; a component that needs a value this document does not carry extends this document first.

The whole reason this surface exists is that the platform understands what is happening on it. A terminal shows bytes; a conversation shows a message, a tool call, an approval and a number. **Anything drawn here that could equally have been drawn as scrollback has thrown away the thing this plane was built to obtain.**

---

### 18.1 The message stream

The turn-by-turn record of the conversation: the owner's turns and the agent's, in the order they happened.

**Turns, not bubbles.** A turn is a band of content on the surface, separated from the one before it by a 1px `--rule` and `--space-4` of air. No speech bubbles, no alternating alignment, no avatars. Two things distinguish a turn: a mono `xs` uppercase `--ink-3` label naming who is speaking, and the ink the content takes — the owner's turns in `--ink-2`, the agent's in `--ink`. The agent's words are the ones being read.

Content is `md` on the platform stack at `--leading-base`, with `text-wrap: pretty`.

**Where the eye rests when output is long.** The surface is pinned to the newest turn while the newest turn is the one growing, and stops being pinned the moment somebody scrolls up. It does not scroll back down on its own: a person reading something the agent said four turns ago has said what they want. When new content arrives while they are reading, a quiet control at the foot of the surface says how many turns they are behind and returns them — a `--control-h-sm` quiet key, not a jump nobody asked for.

The surface owns its overflow. `body` never scrolls (§5).

**Streaming versus committed.** Text that is arriving is drawn in the same ink and the same size as text that has settled. Nothing about a message changes when it commits, because a message that restyles itself on commit tells a reader the earlier state was provisional when it was not. A turn still in flight carries the caret — the same 1ch `--accent` block the introduction uses (§15) — and loses it when the turn ends. That is the whole difference.

**The equivalence rule, which is what this section is really about.** A turn drawn from history and a turn watched arriving must produce the same result. Streaming is liveness and history is the record; if the two disagree, the one a person reloads into is the one they will believe, and the difference is a defect rather than a style. Nothing may be rendered from a streaming frame that is not also derivable from the durable log.

**Fidelity.** The code blocks, lists, headings and inline formatting the agent emitted are rendered as the agent meant them (§1). The frame is styled and the content is not:

- Code blocks take `--font-mono` at `--text-base` on `--inset`, with `--space-3` of padding and `--radius`. They scroll horizontally inside themselves; the surface never does.
- Inline code takes `--font-mono` at `--text-base` on `--inset`.
- Lists, headings and emphasis take the type scale §4 already defines and nothing else.
- **No colour is applied to agent content**, ever. Not syntax highlighting, not a tint per role, not a semantic colour on a word the agent happened to write. `--live`, `--wait`, `--fail` and `--ended` describe session and system state (§2) and describe nothing an agent said.

---

### 18.2 Tool calls

A tool call is **structure, not prose**. This is the section that pays for the plane.

**Collapsed by default**, as a `--control-h` row: a disclosure caret at `2xs` in `--ink-3` that flips rather than rotating (§7), the tool's kind, and its target. Both come from the kernel verbatim and are drawn in `--font-mono` at `--text-base` — a kernel's vocabulary for its own tools grows without warning, so an unrecognised kind is rendered as it arrived rather than mapped onto a set this document invented.

**The collapsed row carries enough to identify the call without opening it.** A row reading only "tool call" is a row somebody has to expand to learn anything from, which is scrollback with extra steps.

**A running call is distinguishable from a finished one without reading it**, and a failed one from a successful one:

| State | Treatment |
|---|---|
| running | the caret's place is taken by the wave §11 uses for waiting, and the row takes `--press-none` |
| succeeded | `--live` status word, `--ink` target |
| failed | `--fail` status word, `--ink` target |

The status is a **word** as well as a colour, because §2 forbids colour carrying meaning alone.

**Disclosure, not truncation.** A tool call whose output is enormous is agent-emitted content and §1's fidelity rule applies, so it is not silently shortened. Bounding it behind the disclosure is a frame decision and is permitted; dropping the tail of it is not. Expanded output takes the code-block treatment from §18.1 and scrolls inside itself.

---

### 18.3 Approvals

The card that appears when a session parks on `requires_action`. It is the highest-stakes control in the product and reads as one.

It takes the **card impression** (§3, 5px), a 1px `--accent` border, and sits in the message stream where the turn stopped — not above it, not pinned, not floating. It belongs at the point in the conversation the agent reached.

It carries three things, in this order:

1. **What is being asked**, as the kernel worded it, at `md` in `--ink`. Verbatim: a kernel that says `Run \`rm -rf build/\`?` is asking about that command, and rewording it into a category loses the one detail the decision turns on.
2. **Why**, when the kernel said — its own name for the kind of thing it wants to do, at `xs` mono uppercase in `--ink-3`.
3. **The answers the kernel offered**, as `--control-h` keys in a row. **Rendered from what arrived, never from a hardcoded pair.** A kernel offering three answers gets three keys; the interface has no opinion about how many there are or what they are called.

The affirmative answer is the surface's one accent key (§7). Everything else is quiet. An answer that abandons the turn is not the same as one that refuses this action, and the two are never collapsed into one control.

**It is not a modal, and this is binding.** A session can sit on an approval while its owner is reading something else, and a modal makes every other session in the rail unreachable until they answer. The rest of the interface stays usable while one is pending: the rail can be scrolled, another session opened, the sheet toggled.

**It survives a reconnect**, because approvals ride the durable event stream: a client that closes the tab and returns finds the request waiting and answerable. The surface therefore draws it from history like everything else, and never from having held a connection.

**A session waiting on one says so in the rail.** Its status dot takes `--wait` and its last-activity line says it is waiting for an answer — an approval nobody notices is an agent stopped for no reason anybody can see.

---

### 18.4 The composer

Where a prompt is written and sent, in a band at the foot of the surface.

A `--control-h-block` field growing to a maximum of six lines and then scrolling inside itself, and one accent key. Enter sends; Shift+Enter opens a line. The key is disabled while the field is empty, and says so.

**What it does while a turn is in flight is not a style choice.** A second turn arriving mid-turn resolves under the session's `concurrent_input` policy — `reject`, `enqueue`, `interrupt` or `rollback` — chosen at creation and reported in the session's capability document. The composer **reads that policy and behaves accordingly**; it does not invent one and it does not disable itself on a guess:

| Policy | The composer |
|---|---|
| `reject` | accepts text, disables the send key, and says a turn is in progress |
| `enqueue` | accepts and sends; says the turn will follow the one running |
| `interrupt` | accepts and sends; says sending will stop the turn in progress |
| `rollback` | accepts and sends; says sending will discard the turn in progress |

The sentence is on the surface beside the key, not in a tooltip (§7).

**Steering is a separate, granted thing.** A session not granted `steer` shows no steering affordance at all — not a disabled one — because there is nothing about the session's state to explain and nothing that would ever enable it. A control that can never work is not a control that is currently unavailable.

**Interrupt lives here**, beside the composer, because it is what somebody reaches for in the same moment and for the same reason. A quiet `--control-h` key, present only while a turn is in flight, and only on a session granted `interrupt` — a kernel that cannot withdraw a prompt is refused by name rather than left waiting.

**A composer on a session that cannot take input refuses, and says why** (§11). It is never merely inert.

---

### 18.5 Usage

Where what the session has cost is shown, and what is drawn when it cannot be known.

In the surface's foot band beside the composer: a mono `xs` label, the token count with `tabular-nums`, and the cost. It updates as the session reports it and is never estimated — a number this interface computed would be a number nobody can reconcile with a bill.

**Absence is a fact, not a zero.** `usage` is a granted capability, and a session that was not granted it says *"not reported"* rather than showing `0`, an empty space, or nothing at all.

This is the same rule §14 already holds for the operations page, which withholds the running total while any provider is unobservable rather than showing a total that is wrong, and it is the same reason: **a reading of the interface that suggests "nothing has been spent" when the truth is "we cannot tell" is a lie the interface told.** A session with no usage grant has often spent a great deal.

A session moved to the terminal plane, which is never granted `usage`, drops the figure rather than leaving the last one it saw on screen.

---

### 18.6 Plane, and changing it

A session's plane is decided when it is created and is a property of the session, not of the view (§1). Moving between them is a deliberate act with a cost, and the interface is honest about the cost.

**The affordance** is a quiet `--control-h-sm` key in the session header, beside the lifecycle controls, naming the plane it moves to. It is **absent — not disabled — when the session's kernel does not advertise the other plane**, for §18.4's reason: a control that can never work is not one that is currently unavailable.

**It confirms, inline** (§7), in place of the header's own controls. The confirmation states the two things that are true and are not obvious:

- the turn in flight is lost
- the session is briefly not interactive

**While it runs, this is `rotating`** and §11 applies without exception: input refused, the reason on the surface, and nothing accepted that would be dropped.

**Afterwards, the boundary is drawn.** A rotation carries a session's conversation forward from a snapshot, and anything after that moment did not travel. §11 requires the moment to be shown; on this plane it is not a status chip but **a rule across the message stream at `rotated_at`, labelled with the time in the display zone**, at the point history stops being continuous. That is the difference between an owner who trusts the transcript and one who does not: a gap they can see is a gap they can recognise, and a gap they cannot is one they will read as the agent having forgotten.

**Both refusals are drawn, never hidden.** A kernel that does not advertise the other plane, and a conversation that cannot be resumed on the other side, both come back as typed errors naming what is missing. The interface reports what the API said — §14's existing rule, applied here.

---

### 18.7 What the conversation surface does not do

**It renders no terminal.** Not a small one, not an embedded one, not a fallback one. A session that needs a terminal has its plane changed (§18.6).

**It offers no operation AG-UI does not project.** `API_Overview.md` §1's matrix gives AG-UI no `screen_snapshot` and no `resize_terminal`: a screen and a geometry are terminal facts. A person looking for either is told the terminal plane is where they live, rather than left hunting for a control that was never going to exist.

**Where it withholds something, it is reporting what the API says**, exactly as §14 already says of Operations. Every refusal names the capability it lacks, on screen and not only in a console — an API that names what is missing (`API_Overview.md` rule 4) is worth nothing if the interface renders it as "something went wrong".

**It never renders a credential.** What a session may reach is a fact about its configuration and is shown (§11); what it holds is not, and appears nowhere.

**What the terminal can do that this cannot, and what to do instead.** Stated here rather than discovered, because a person hunting for a control that was never going to exist blames the interface and then themselves.

| Wanted | On this plane | Where it lives |
|---|---|---|
| Resize the agent's terminal | absent — a geometry is a terminal fact | switch the session's plane (§18.6) |
| Read the rendered screen | absent — a screen is a terminal fact | switch the session's plane (§18.6) |
| Type into a full-screen TUI | absent — this surface renders no terminal | switch the session's plane (§18.6) |
| Interrupt a turn | where the kernel offers it; named when it does not | — |
| Steer mid-turn | under the session's own policy; no affordance where it was not granted | — |
| Read what it has spent | where the kernel reports it; "not reported" otherwise (§18.5) | — |

The first three are one answer: **the plane changes, the session does not**. It keeps its identifier, its history and its place in the rail, and §18.6 is the whole of what that costs.

### 18.9 A run that failed

**Drawn in the message stream, at the point it stopped.** Not as a band above the surface and never in place of it — §12 already settles that for a failed *action*, and the same reasoning is sharper here: a failure has a position. It happened after the last tool call and before nothing, and what the agent had already done stays readable beside it. A failure hoisted out of the stream loses the one thing that says what the session was doing when it stopped.

**The word and the ink, never the ink alone.** `--fail` on the label, and the label says the run failed. §2's rule is not relaxed because the meaning feels obvious.

**Three things, in this order.**

| | |
|---|---|
| What failed | the kernel's own words, verbatim, as §18.2 already requires of a tool call's status |
| The ability it lacked | **named**, and only when the failure was a refusal |
| The stop reason | as the API spells it, in identifier style |

The middle row is the whole point of the section. `API_Overview.md` rule 4 asks errors to name what is missing, and §18.7 already says every refusal names its capability on screen rather than only in a console. A refusal is arrived at typed — the control plane knows *which* ability was absent — and an interface that renders it as "something went wrong" has thrown away the only part its reader could act on. Most failures name nothing, and a name invented for those is worse than the absence: it would be read as one, and acted on.

**A session being carried onto another runtime is not a failure.** This is the trap, and it is not obvious: a rotation ends the run — the runtime behind it is being torn down — and reaches a client on the *same frame* a genuine failure does, because the protocol has one event for a run ending badly. §11 forbids `rotating` taking `--fail` and §18.6 already specifies how the boundary itself is drawn, so the two must be told apart before either is drawn. What distinguishes them is stated by the API rather than guessed from wording: a handover says the session is coming back, and a client that cannot read that draws every rotation as a fault.

Where the distinction is absent — an older control plane that does not send it — the frame is drawn as a failure. That is the safer of the two to be wrong about: a handover drawn as a failure is confusing, and a failure drawn as a handover tells somebody their session is returning when it is not.

**A new run clears it**, entry and all. It belonged to the run that ended.

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
