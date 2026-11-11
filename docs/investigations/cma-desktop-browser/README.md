# Shared desktop browser launch lifetime (#1391)

Both desktop `--browser` branches previously used `exec chromium`. Their caller
therefore remained running, with its output pipes open, until Chromium exited.
The response ledger requires the provider's `TurnCompleted` event; a final
assistant message does not substitute for that event. Response finality is
unchanged by this fix.

The XFCE desktop entry and Omarchy startup, key binding, and browser wrapper all
use this launcher. The separate headless browser service owns its own explicit
request lifetime and is unaffected.

## Ownership and readiness

`cma-desktop-session --browser [URLs...]` sends a bounded request to a private
Unix socket. The manager is started by the desktop session and remains in its
process ancestry for native recovery and writer fencing. Requests are serialized.
The persistent profile remains `$HOME/.cma/chrome` under the desktop's user.

The manager launches Chromium with `setsid` (Python `start_new_session=True`),
`close_fds=True`, stdin from `/dev/null`, and stdout/stderr in the existing
`/tmp/cma-chromium.log`. Tool calls create only a short-lived request client.
Readiness requires a live profile-owning process and a visible window with its
PID: X11 uses `xdotool --all`; Wayland uses the desktop's Hyprland instance.
No DevTools listener is exposed. Probes and startup have bounded deadlines.

An existing browser is focused. URL requests use Chromium's existing singleton
protocol and wait for successful forwarding plus window readiness. Concurrent
requests do not create duplicate persistent browsers. Startup failure or timeout
terminates the unsuccessful process group.

The manager has its own session so the runtime's desktop-group SIGKILL cannot
interrupt cleanup. It observes the desktop PID **and start time**, detects owner
exit (including SIGKILL), and terminates the browser groups. A Linux subreaper
also retains and cleans helpers that double-fork or start another session.
Graceful shutdown gives the browser a short opportunity to flush its profile.
The manager lock prevents competing managers from replacing the request socket.
Desktop recovery pins include the new scripts and retained XFCE/Openbox/Omarchy scripts.

Exit codes: `0` ready, `64` invalid request, `69` desktop unavailable/stopped,
`70` browser startup failure, `124` readiness/request timeout. Browser readiness
has a 20-second deadline; the client allows 23 seconds including manager startup.

## Verification

- `python3 api/crates/layer4-runtime/src/desktop-browser-test.py` on Linux runs the
  real launcher branches and IPC against a fake browser/window probe. It checks
  session isolation, stdout/stderr and extra-descriptor EOF, profile reuse,
  concurrent callers, failure, bounded probes, unrelated-window rejection, and
  owner SIGKILL cleanup including a helper in another session.
- `python3 api/crates/layer3-adapter/src/codex/recovery_desktop_test.py` verifies
  source pins and recovery ownership, including retained desktop versions.
- The existing desktop and Omarchy image evidence scripts use an offline model
  fixture with real native Codex/ACP tools. Their shell command closes/reopens
  Chromium, exercises reuse, navigates a fixture page, and inspects its screenshot.
  They require terminal tool statuses and ACP `end_turn`, then verify that the
  page remains open for shared user input. The fixture does not create a CMA
  Agent/Sandbox or call a remote model. It verifies the provider terminal boundary;
  it is not an end-to-end REST response or NyxID deployment test.

Ship rebuilt desktop runtime images (Debian `RUNTIME_DESKTOP=1` and the Omarchy
image target) and the API image containing setup guidance/recovery source pins.
The default runtime image also packages the shared scripts and recovery-related
runtime code, so its normal image contract/build remains a release gate.
