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
No author edits another directory or the shared registry. All contributors receive this same contract. Revision lead-1532-v2 assigns
35 user rows to Developer17, 34 to Developer06, 34 to Developer16, and the
6 file rows to Developer15, 3 Agent settings rows to Developer14, and 8 document rows to Developer08. Developer01 owns 39 admin rows and Developer09 owns 37 admin
rows; ProdVerifier04 owns the 87 API supplement operations. Existing handlers
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
  Supply `agentId` for every Agent-bound route, including aliases. The harness
  routes to the observed non-owner and records owner/epoch/request correlation.
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
