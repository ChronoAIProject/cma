# USER executable cases, part 1

Issue #2149. These 35 definitions implement the first 35 USER rows in the
lead-1532-v2 assignment, from `account.sign-in` through `agent.copy-response`.
The shared catalog and assignment remain the authority. No transferred row is
registered here. `index.mjs` exports the cases and four setup factories; the parent owns execution.

The cases drive real Playwright controls on the compiled Web. Setup lives in
`user-fixtures.take(name, ctx)` and has an explicit contract in `fixtures.json`.
Setup may prepare resources, hold a bounded runtime turn, or interrupt a real
observation. It cannot execute a case or supply a PASS verdict. The assertions
check visible controls, exact admission/command/resource identity, public durable
readback, and actual execution effects. Missing authentic setup is BLOCKED.

The source incorporates shared routing base `f5d2d08d3`. Mutations use the
observed non-owner AM. Readback inspects the actual browser write receipt,
switches the existing session to the opposite serving AM, and uses that explicit
AM for public reads. Browser evidence must contain matching Agent hashes,
request IDs, stable owner proofs and distinct serving AMs. Changing a requested
ingress label cannot satisfy this assertion. Client preferences and other global
features do not invent an Agent owner.

Cleanup is registered immediately after fixture acquisition and before opening
the browser. Creation setups must retain uncertain admission custody as well as
returned operation IDs. Browser listeners, sessions and exact public resource
cleanup run on success, failure and BLOCKED outcomes. Pending request tests keep
the original key; they never replay an unknown effect under a new identity.
Public polling is bounded and no faster than two seconds.

Run the focused offline suite from repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/developer-17/cases.test.mjs \
  .github/scripts/multi-am-features/executable/runner.test.mjs \
  .github/scripts/multi-am-features/executable/browser.test.mjs
```

These tests validate failure handling, cleanup, exact assignment/steps, SSE
receipt parsing, truncated/foreign transcript refusal, deduplication and actual
cross-AM evidence. They are not live feature results. No application or Mongo
lane is launched by these modules. The parent must run N=3 before its controlled
owner restart and N=1 afterward on the exact image.

Runtime integration requirements remain explicit:

- `account-authentication` requires genuine browser session authentication and
  real local NyxID. Bearer injection is refused before sign-out is exercised.
- `task-machine` requires real WebSocket forwarding and an actual rendered Linux
  desktop/Chromium frame. The initial shared adapter rejects upgrades; that
  setup must remain BLOCKED until the real bridge is available.
- Navigation needs a prepared inventory large enough for pagination and known
  personal/shared/organization permissions. Filter expected rows come from the
  independent setup manifest. Insufficient cardinality is not a feature PASS.
- Turn, approval and queue cases require a real deterministic runtime with
  bounded barriers and an effect journal. Queue edit/remove races pause only
  browser observation while the real worker dispatches. Approval replay is
  checked against a different pending request.
- Repository, capacity, uncertain-admission and Trigger quick-message fixtures
  require the specific local dependencies documented in `fixtures.json`.

The first authentic setup providers are now implemented in `setups.mjs`:

| Setup                 | Rows enabled when parent primitives are bound             | Real preparation                                                                                                                         |
| --------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `account-preferences` | Appearance, text size, build version, Docs, legacy routes | Real signed-in browser; independent compiled artifact metadata; fetched and hash-checked `build-info.json`; actual clipboard permissions |
| `task-draft`          | Task draft                                                | Two verified browser subjects plus one fresh public Sandbox; opens the real location-scoped New task dialog                              |
| `agent-public-ready`  | Open                                                      | Public Sandbox, empty Workspace, lifecycle admission and exact completed transcript; Agent-bound browser                                 |
| `navigation-tasks`    | Tasks navigation                                          | Public hierarchy, actual foreign Sandbox and denied read, completed Workspace projection, real one-shot GET failure control              |

The exact inputs live under `parent_primitives` in `fixtures.json`. Smallest runnable
subset is the five preference rows: bind `compiledWeb`, `identities.user`, and the
existing browser with real session storage and clipboard permissions. The build
identity comes from served bytes checked against the parent's independent digest;
local builds keep their truthful local label. It is never inferred from an API image.

Draft and Agent setups additionally need `userResources`: a pinned public Sandbox
profile, exclusive disposable scope, a parent custody lease, and an independent
read-only resource-finality observer. Each POST retains its original intent before
dispatch. Accepted IDs are captured even when request attribution fails. Unknown
outcomes are not retried, and keep the lease unresolved. Setup writes go through
AM A (or the selected ingress); exact public readback checks the actual response
receipt from the other AM. Public retirement requires matching owner/name, the
retained retired state, zero children and zero pending admissions. A 404 cannot
stand in for disposal. Cleanup is idempotent; any unresolved cleanup fails the row.

The navigation setup also requires a real one-shot inventory GET fault handle;
it never supplies a successful response or runs the case. Remaining setup names
stay delegated to the parent and BLOCKED if absent. Large navigation inventories,
search sentinels, authentication round trips, deterministic turn/queue/approval
controls, Trigger/repository/capacity setups and real Machine transport are still
separate dependencies, enumerated in `remaining_unimplemented_setups`.

The focused command above can include `cases/developer-17/*.test.mjs` to run the
new setup contract checks as well. Install the shared harness's declared Node
dependency with `npm ci --ignore-scripts` in `.github/scripts/multi-am-features`
before the browser adapter tests. No live CMA application is launched by these
checks; browser transport tests use disposable loopback HTTP/WebSocket fixtures.

Until those primitives are installed, registration means authored coverage, not
executed coverage. Functional and latency verdicts stay separate. Create-to-
usable, Send admission and confirmed interruption timing receipts supplement
parent step timing; no performance success is inferred from offline tests.
