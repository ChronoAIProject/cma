# USER executable cases, part 1

Issue #2149. These 35 definitions implement the first 35 USER rows in the
lead-1532-v2 assignment, from `account.sign-in` through `agent.copy-response`.
The shared catalog and assignment remain the authority. No transferred row is
registered here. `index.mjs` exports the cases; the parent owns execution.

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

Until those fixtures are installed, registration means authored coverage, not
executed coverage. Functional and latency verdicts stay separate. Create-to-
usable, Send admission and confirmed interruption timing receipts supplement
parent step timing; no performance success is inferred from offline tests.
