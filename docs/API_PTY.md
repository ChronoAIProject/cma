# PTY Interface

**Priority 3 — for CLI-like streaming clients.**

The PTY interface relays a real pseudo-terminal from the agent runtime to a client, byte for byte. A browser attaching to it sees exactly what a developer running `codex` in a local terminal would see, including the full-screen TUI.

See [API_Overview.md](./API_Overview.md) for the core operation set and session model.

---

## 1. Why this interface exists separately

The agent kernels present **full-screen TUIs** — Codex's is built on ratatui. That means:

- Output is not a scrolling log. The application repaints regions and uses the alternate screen buffer.
- There is no "line N was emitted" event to project into a semantic stream.
- Fidelity requires a genuine PTY: termios, raw-mode input, and `TIOCSWINSZ` resize. A pipe is not a substitute — a kernel given a pipe instead of a TTY will either refuse to render or produce unusable output.

No semantic interface can carry this honestly, so it gets its own. Clients wanting a conversation use [AG-UI](./API_AGUI.md).

---

## 2. Endpoint

```http
GET /api/v1/sessions/{id}/terminal
Upgrade: websocket
```

Authenticated as the session owner. A second concurrent attach is rejected — see §6. Two ways to authenticate; see §2.1.

### 2.1 Authenticating the upgrade

The browser WebSocket API cannot set an `Authorization` header on the upgrade, and cross-origin cookies are not a workable contract for chrono-bot's actual consumers — other products' frontends, on their own origins. So there are two ways in, and a client uses whichever fits it.

**Same-origin, cookie-authenticated** (our own dashboard): nothing to do. The browser sends the session cookie itself and the upgrade is authenticated like any other route.

**Cross-origin, bearer-authenticated** (a product frontend): exchange the credential for a ticket over a route that *can* carry a header, then present the ticket on the upgrade.

```http
POST /api/v1/sessions/{id}/terminal-ticket
Authorization: Bearer …
Content-Type: application/json

{ "takeover": false }
```

```json
{ "ticket": "…", "expires_at": "2026-08-17T14:37:14Z" }
```

```
wss://…/api/v1/sessions/{id}/terminal?ticket=…
```

The body is optional; omitting it means `takeover: false`.

**What makes a ticket in a URL acceptable.** It is single-use — the second redemption is refused, and so is the second attempt after a refused first. It expires in about thirty seconds. It is bound to one session and one subject, and it grants nothing but the attachment it was issued for. A ticket presented against a different session is refused *and spent*. The long-lived credential never appears in a URL, a proxy log, or a browser history entry, which is the whole point of the exchange.

**A ticket is never recorded.** The activity log keeps only what its classifier names, and `ticket` is not a name it knows. Nothing logs a ticket's value. Asking for a ticket is recorded as the attach it is for, with no query parameters and no body content.

**Alternative considered.** The token could instead ride `Sec-WebSocket-Protocol`, which browsers do let a page set. The ticket is preferred because it keeps long-lived credentials out of the WebSocket layer entirely: a subprotocol header carries the real credential into every intermediary on the path and into whatever any of them logs, for the life of the connection, whereas a ticket's exposure is bounded to seconds and to one use.

**Refusals happen before the upgrade.** A missing, spent, expired, or wrong-session ticket produces an HTTP `401` with a problem document, not a socket that opens and immediately closes — so a client can tell "you are not allowed" from "the terminal went away".

### Framing

Binary frames are terminal bytes. Text frames are JSON control messages. There is no in-band escape convention.

| Direction | Frame | Meaning |
|---|---|---|
| C → S | binary | raw keystroke bytes written to the PTY master |
| C → S | `{"t":"resize","cols":120,"rows":40}` | propagate to `TIOCSWINSZ` |
| C → S | `{"t":"signal","sig":"INT"}` | out-of-band signal; `INT`, `TERM`, `QUIT`, `HUP` |
| C → S | `{"t":"ping"}` | keepalive |
| S → C | binary | raw PTY output |
| S → C | `{"t":"ready","cols":120,"rows":40,"snapshot":true}` | attached; a screen snapshot follows |
| S → C | `{"t":"exit","code":0,"reason":"end_turn"}` | the kernel process ended |
| S → C | `{"t":"error","code":"…","detail":"…"}` | typed failure |
| S → C | `{"t":"pong"}` | keepalive response |

---

## 3. Attach sequence — get this right or the first paint is corrupt

```
1. Client opens the WebSocket
2. Client sends {"t":"resize", …}   ← BEFORE any binary frame
3. Server replies {"t":"ready", …}
4. Server sends a screen snapshot as binary
5. Bidirectional streaming begins
```

**Send `resize` before the first data frame.** The runtime's PTY is allocated at a default geometry; a full-screen TUI that paints at the wrong size produces corruption that does not self-heal until the next full repaint.

Resize frames with `cols` or `rows` of `0` are rejected with `{"t":"error"}` and ignored — a zero dimension is a common bug in fit calculations during layout transitions.

---

## 4. Reconnect and screen state

**A dropped connection does not end the session.** It detaches. The session remains alive and reattachable, and the kernel keeps working.

**The reconnect contract**, in order, every time:

```
1. (bearer clients) POST …/terminal-ticket   ← a ticket is single-use; reconnecting means a new one
2. Open the WebSocket
3. Send {"t":"resize", …}                    ← still mandatory, still before any binary frame
4. Receive {"t":"ready", …}                  ← limits, conformance classes, wire version
5. Receive the screen snapshot and scrollback as binary
6. Live bytes
```

Step 1 is not optional on reconnect. A ticket is spent the moment it is redeemed, so a client that kept one for later will find it refused.

chrono-bot maintains a **server-side terminal emulator** per live session, so on reattach the client receives a full screen snapshot and then live output. This is necessary rather than convenient: replaying raw historical bytes is wrong after any resize, and a full-screen application's byte stream cannot be seeked.

Consequences for clients:

- Do not buffer raw bytes hoping to replay them. Reattach and take the snapshot.
- Scrollback is bounded. The snapshot is the current screen plus a capped scrollback, not the session's entire history.
- If the session was `idle` when you attach, attaching **revives it**. Expect a cold start, and expect the screen to be freshly rendered rather than restored mid-turn.

---

## 5. What survives an idle-to-revive cycle

| Survives | Does not survive |
|---|---|
| Session identity and id | The kernel process |
| Workspace and `CODEX_HOME` on durable storage | In-flight turn |
| Event history | Terminal scrollback beyond the snapshot cap |

A revive is a **cold start**. Clients should surface that rather than presenting it as a seamless resume.

---

## 6. Concurrency

One read-write attachment per session. A second attach receives `{"t":"error","code":"already_attached"}` and the connection closes. `TerminalAttached` and `TerminalDetached` appear in the session's durable event log, so anything watching that stream can see who holds it.

**Takeover.** The owner may claim a terminal somebody already holds, by asking for the ticket with `{"takeover": true}`. This exists because the alternative is a lock-out: a client whose connection died in a way the server has not yet noticed still holds the attachment, and its owner cannot get back into their own session.

The superseded client is **told**, not merely starved of output — it receives a close frame with code `4000` and the reason `the owner attached this terminal from somewhere else`. A terminal that simply stops updating is indistinguishable from a broken one, and whoever is looking at it will reload and take the attachment straight back, which is a fight nobody wins.

Takeover is not a way around ownership: a session the caller does not own is refused exactly as an ordinary attach would be.

**Observers do not attach.** A client that only needs to watch uses `GET /screen` and the events stream. A second attachment is the wrong tool for it, and taking one would displace whoever is actually working.

Multi-viewer fan-out is not offered in this version. The server-side emulator makes it possible later; the seam exists, the feature does not.

---

## 7. Operations projected

| Operation | How |
|---|---|
| `send_input` | binary frames — keystrokes |
| `stream_events` | binary frames — PTY output |
| `interrupt` | `{"t":"signal","sig":"INT"}`, or Ctrl-C in the byte stream |
| `resize_terminal` | `{"t":"resize", …}` |
| `screen_snapshot` | delivered on attach; also available over [REST](./API_REST.md) |
| `resolve_approval` | **at the TUI.** The user answers the kernel's own prompt by typing |

`create_session`, `steer`, and `get_usage` are not projected here. Create the session over REST first, then attach.

### Approvals behave differently here, and it matters

On every other interface, chrono-bot intercepts a permission request and applies policy before the action runs. On this interface **the user answers the kernel's prompt directly in the terminal**, so the control plane's approval policy is not consulted.

That is inherent to relaying a real terminal, not an oversight. Two consequences:

- A session whose policy requires server-side approval should not be attachable over PTY. Enforce it at attach time, not in documentation.
- A client holding a PTY attachment can type anything the kernel accepts, including shell escapes. The sandbox boundary is the only boundary. Treat PTY attachment as equivalent to shell access to that session's sandbox.

---

## 8. Capability declaration

The `ready` frame carries the interface's capability set:

```json
{ "t": "ready", "cols": 120, "rows": 40, "snapshot": true,
  "conformsTo": [
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/pty-core",
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/terminal"
  ],
  "limits": { "scrollbackLines": 2000, "maxFrameBytes": 262144 } }
```

---

## 9. Client implementation notes

- **Terminal emulation belongs in the client.** Use a real emulator; do not attempt to interpret ANSI yourself.
- **Harden the emulator.** The byte stream originates from a process running the user's own untrusted code. Disable OSC handlers you do not need — in particular **OSC 52 (clipboard write)**, which lets session output silently overwrite the host clipboard. Do not allow escape sequences to set the browser tab title or trigger any host action.
- **`TERM` is set by the runtime**, not by the client. Do not attempt to negotiate it.
- **Send keepalives.** Ingress and intermediate proxies impose idle timeouts well below a terminal's natural idle period.
- **Cap what you render.** Do not let unbounded output exhaust the client.
