# Executable feature cases

Coverage lead/integration owner: Developer15. Issue #2149 remains open. This branch
starts at origin/develop 92b66eb57276a2f467322dc1d78da2ae7221af6e and normally merges
the M6 harness. Authors branch from the published `tl/cma-13b/coverage` scaffold,
using `tl/cma-13b/coverage-<roster-name>`. Do not merge into develop or launch a
second shared application environment. Use the required chronoai-shining commit
identity. Developer15 merges author branches and owns the shared runner/catalog.

`catalog.json` is the frozen, sanitized checklist: 120 user, 76 admin, 79 EG rows,
plus 87 separately counted API operations. The source hashes bind the original
checklist and audit. `assignments/<roster-name>.json` owns exact case IDs and one
exclusive `cases/<roster-name>/` directory. Export `cases` from that directory's
`index.mjs`; additional modules, tests and fixture specifications stay within it.
No author edits another directory or the shared registry. All contributors receive this same contract. Revision lead-1701-v6 assigns
35 user rows to Developer17, 34 to Developer06, 8 usage rows to Developer16, 26 Web rows to Developer02, and the
6 file rows to Developer15, 3 Agent settings rows to Developer14, and 8 document rows to Developer08. Developer01 owns 39 admin rows and Developer09 owns 37 admin
rows; ProdVerifier03 owns 79 EG rows and ProdVerifier04 owns the 87 API supplement operations. Existing handlers
are references only until every checklist step is implemented and verified.

Case IDs preserve the checklist exactly: `C275:<original-feature-id>` and
`API:API-SUPPLEMENT-001` through `API:API-SUPPLEMENT-087`. Do not rename IDs or
combine rows into an opaque smoke case. One implementation may share helpers,
but each row must assert every original step and expected-result clause.

Each case exports `{id, scope, steps, fixtures, run}`. `scope` is `agent`,
`global`, `browser`, or `eg`. `steps` exactly matches that row's `required_steps`
(`step_1`, ... and `expected_result`). `fixtures` lists actual named fixture
dependencies. Missing dependencies become BLOCKED, never PASS. A declaration,
TODO, generic GET, unsupported fake response or source-only assertion does not
make a row executable. Browser rows require compiled real UI actions; EG bot
rows require the actual synthetic platform journey through real EG processes.
Code outside a supported local fixture remains explicitly blocked.

The async `run(ctx)` interface is fixed:

- `ctx.row`, `ctx.phase` (`before_restart`/`after_restart`), `ctx.iteration`,
  `ctx.am` (scheduled ingress), `ctx.state` (case-local state), `ctx.runState`
  (shared across that case's iterations and the controlled owner restart).
- `await ctx.step('step_1', async () => {...})`: execute each required step once.
  A step requires at least one assertion; incomplete/missing steps fail.
- `ctx.assert(condition, 'fixed_error_code')`: assert concrete observed behavior.
- `await ctx.request({actor:'user'|'admin'|'outsider', am:ctx.am, method, path,
  body, key, allowed:[200], agentId})`: real HTTP with response body/status/timing.
  Supply `agentId` for every Agent-bound route, including aliases. An explicit `am` is honored exactly, including owner requests. Omit `am`
  for automatic non-owner selection. Both retain owner/epoch/request correlation.
  Global operations run on each ingress without inventing an Agent owner.
- `ctx.fixture('browser')`, `ctx.fixture('eg')`, `ctx.fixture('agents')`, etc.:
  return the declared installed adapter; missing fixtures throw BLOCKED. Publish
  a `fixtures.json` beside your cases describing any required methods and
  concrete setup. Developer15 owns shared adapter integration. Send fixture
  requirements immediately so they do not appear only at handoff.
- `await ctx.observe('fixed_label', async () => actualObservation)`: retain the
  serializable observation privately and return it. Use for browser/platform
  receipts, final durable state, screenshots/artifact hashes, or effect counts.
- `ctx.defer('fixed_cleanup_label', async () => publicCleanup())`: register exact
  resource cleanup as soon as creation custody is known. Cleanup runs in reverse
  order on success or failure; cleanup failure prevents PASS and preserves the
  primary failure separately. Do not replay/rekey uncertain mutations.

Requests return `{status, body, timing:{request_id,am,...}}`. Reuse existing M6
helpers through relative imports where useful; direct calls must still pass
through the context so evidence joins are retained. Use stable keys for a single
intent, changed values for writes, fresh authorization, and exact public cleanup.
Do not record credentials in observations, errors, filenames, logs or commits.
Private response artifacts are not uploaded; public counts contain hashes only.

The parent runs N=3 before restart and N=1 after, binds the newest exact image,
and owns the single controlled owner restart and HPA scenario. Authors supply
real assertions and focused offline regressions; a mocked contract test is not
an actual feature PASS. No retry of a failed case silently replaces its result.
Performance verdicts stay separate from functional verdicts, with failures and
timeouts retained in the original denominator.

Handoff: commit, case IDs, implemented steps, named fixture requirements, exact
test command/results, known gaps, cleanup behavior. First fixture requirement
handoff is immediate; first executable slice by 16:30; complete author handoff
by 18:30 to leave integration/execution time before the 20:00 target. Report
honest blockers promptly. Developer15 publishes hourly counts per role for
registered, executable, passing, failing, blocked and not run; 87 APIs are never
added to the C275 denominator.

Browser adapter contract (shared, owned by Developer15):

- `await ctx.fixture('browser').open({actor, am, agentId})` returns a real
  isolated Playwright session `{page, origin, switchAM(nextAm), close()}`.
- `browser.otherAM(am)` returns the other ingress. `browser.evidence(page)`
  returns sanitized HTTP/ingress/owner receipts. Preserve cookie/session on
  switch; never convert an unattributed request into an owner-routing PASS.
- `await ctx.fixture('user-fixtures').take(name, ctx)` returns explicit setup
  data and cleanup. Named setups may expose low-level fault controls, never a
  `run(case)` result oracle. Missing authentic setups remain BLOCKED.
- Download, clipboard and file chooser use the real Page APIs; raw content and
  credentials stay private. Cases register cleanup with `ctx.defer` immediately.

Routing correction (LEAD16:31): `ctx.request({agentId, am})` honors that exact
ingress for all methods. Omit `am` for automatic non-owner routing. Agent cases
still require an actual non-owner receipt, and whole-row PASS requires receipts
from both AMs. A requested AM label is never evidence of the AM reached.
`session.switchAM(am)` sends subsequent reads to that exact AM while mutations
remain routed to the observed non-owner. Original owner proofs are retained.

Serving evidence follows the sole parent's `c275-serving-v1` contract. Preserve
`serving`, `serving_after`, `started_at`, `ended_at`, `attribution` and
`request_id` when projecting request receipts. Each snapshot binds the actual
local forward origin, Pod UID, process instance and container before and after
the request. Whole-row counts reject missing, changed or label-only evidence.
The browser proxy resolves the current forward from topology for HTTP, SSE and
WebSocket handshakes. It retains acknowledged responses without replay if a
subsequent observation cannot prove the serving process.

The parent may install `services.executable.browserLease` with an existing
Playwright Browser, compiled Web identity/origin, AM origins, explicit actor
cookie sessions and `resolveAgent`. No mixed bearer/cookie identity is accepted.
The participant closes only its own contexts and proxies; the parent owns the
browser process, static server, application lifecycle and final database disposal.

For the next broad parent, `parent-browser.mjs::installParentBrowserLease` binds
that lease plus artifact hashes and exact actor subjects into existing services.
The parent supplies its admitted `launchBrowser` hook; import starts nothing.
`../browser_session.py::capture_browser_session` wraps the existing real PKCE
login and validates the issued cookie against both AMs without Authorization.
It returns cookie state only in memory; retained evidence contains no cookie or
authorization code. Capture and lease helpers remain offline-qualified until the
sole parent executes them. Close the participant's contexts before the parent
lease, then complete public and infrastructure cleanup.

The participant emits `executable_progress` at phase start, after fixture binding
and after every case outcome. `provisional_roles` counts observed outcomes for
that phase; `accepted_roles` still requires the complete N3/restart/N1 proof.
Failures remain in both histories. A phase deadline leaves unattempted rows in
the denominator and emits `phase_budget_exhausted` without synthesizing results.
Checklist `steady` repetitions execute in `before_restart`; whole-row acceptance
also requires the shared verified restart and at least one `after_restart` run.
The runner and counter use the same phase mapping, preserving the catalog's
required repetitions and all serving, cleanup, and identity checks.

Parent configuration accepts `executable_phase_budget_ms` (default 600000),
`executable_case_budget_ms` (60000) and `executable_cleanup_budget_ms` (60000).
The phase reserves the cleanup interval before admitting a case. Context
requests are bounded by the remaining case budget; `ctx.signal` and
`ctx.remainingMs()` are available to setup/provider bindings. Deadline expiry
closes the case's browser sessions, joins the interrupted work, then gives public
cleanup its separate budget. It never races away from a live mutation or starts
another case while that mutation is unresolved. An external fixture that ignores
its signal/timeout still requires the parent's existing outer watchdog; that
condition cannot be counted as bounded successful execution.
