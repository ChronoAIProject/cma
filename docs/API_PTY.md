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

Authenticated as the session owner. A second concurrent attach is rejected — see §6.

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

One read-write attachment per session. A second attach receives `{"t":"error","code":"already_attached"}` and the connection closes.

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
