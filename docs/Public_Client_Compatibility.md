# Retained public client compatibility: Friday slice

This is deterministic, offline evidence for part of M-FUNC-02 (#1537), not a
measurement of the full release gate. The gate remains open. Its current/previous
external consumer matrix and production acceptance still need executable artifacts
and candidate-bound receipts. No feature, provider, origin or retry policy is
activated by these tests.

## Immutable baselines

The November 10 release handoff supplies the deployed identities below. They are
bounded regression baselines for this slice, not a new universal support window
or a claim that every external consumer used the repository SDK at these commits.

| Surface                           | Previous revision                          | Current released revision                  | Tested candidate                                                |
| --------------------------------- | ------------------------------------------ | ------------------------------------------ | --------------------------------------------------------------- |
| Core REST/discovery wire fixtures | `93d4a199a3d36d47f5a89f9117d2bc735900bf4c` | `4f03c35a80322673ec00df7a3b6b9399015f5a68` | The checkout's real Layer 1 handlers and current TypeScript SDK |
| Web desktop preparation client    | `f779e5310595fb62e19537a76f664ba52e8342f9` | `4f03c35a80322673ec00df7a3b6b9399015f5a68` | The checkout's desktop preparation module                       |

`api/crates/layer1-public/tests/fixtures/retained-clients/manifest.json` records
source paths, full source-file SHA-256 values, frozen selection SHA-256 values,
exact case IDs/counts and source commits. The selected cases retain their original
requests, responses and symbol declarations. The six JSON snapshots use one wire
case per line. They are immutable input, independent of the current shared corpus;
the existing contract recorder cannot overwrite them.

The released desktop modules are byte-identical at both pinned revisions. One
exact source copy under `web/src/test/compatibility/` represents both artifacts;
the test checks its digest and executes it separately from develop's module.
This is the actual retained preparation module, not a rewritten model of it. It
is imported only by tests and is excluded by the existing test-directory coverage
rule. The full historical Web bundle/noVNC runtime is not vendored or executed.

To audit provenance, use `git show <full-commit>:<source_path>` and compare its
SHA-256 with the manifest. Extract only the recorded case IDs in original order
and compare each case to the frozen selection. This authoring/audit step is local;
the tests need neither Git history nor network access. New support baselines must
be reviewed explicitly. Do not refresh the old snapshots merely to make a failing
candidate pass.

## What runs

| Contract                                                                                                                   | Cases per release | Mechanism and limits                                                                                                                  |
| -------------------------------------------------------------------------------------------------------------------------- | ----------------: | ------------------------------------------------------------------------------------------------------------------------------------- |
| Agent list/detail, events/transcript/usage/resources, legacy creation and stop/delete/control refusals                     |                20 | Original request method/path/query/body/headers against real handlers; exact normalized response status/headers/body and named errors |
| Sandbox, Workspace and Worktree lists/details; creation/lifecycle/removal; validation, absence, authority and quota errors |                27 | Same in-memory router/store/adapter composition and response oracle; no real provisioning or database                                 |
| Desktop discovery for ready/starting, anonymous, other-owner, forbidden, headless, paused, isolated and missing Agent      |                 9 | Real discovery handlers; exact URL/response/refusal shapes; no WebSocket upgrade                                                      |

That is 56 wire cases per revision, 112 replayed cases. Existing symbol binding
normalizes generated identifiers and timestamps; fixture expectations remain
independent of current handler output. The same replay engine still serves the
current shared corpus. Deliberate route, status, response-header and response-field
mutations must fail the retained oracle.

The current TypeScript SDK emits and decodes those 112 captured exchanges using
an injected fetch function. Typed invalid-input cases must fail before fetching;
raw malformed requests retain their documented server error shape without
pretending the typed SDK can emit them. Required-field deletion, unknown Agent
status and an invalid desktop URL type must fail decoding. An extra ordinary
Agent response field is tolerated and omitted from the typed result. This does
not promise that closed conformance documents or enums accept unknown values.

For the previous, released-current and develop desktop modules, fake WebSockets
and fake timers prove the same route/Agent escaping, HTTP-to-ws and HTTPS-to-wss
selection, binary RFB greeting shape and ordered one-time handoff. They prove
wrong owner/Agent, text/oversized greeting, expiry and transport failure are
refused without input. No socket is opened and no fetch is allowed. This is not
a graphical desktop, server handshake or persistent desktop lease test.

## Commands and CI

From `api/`:

```sh
cargo fmt -p layer1-public --check
cargo test -p layer1-public --lib retained_clients --locked
CMA_PUBLIC_CONTRACT_FILE=agents.json cargo test -p layer1-public --lib shared_public_contract_matches_real_backend --locked
cargo clippy -p layer1-public --all-targets --locked -- -D warnings
```

From `sdk/typescript/`, after locked dependency installation:

```sh
npm run test -- tests/retained-clients.test.ts
npm run typecheck
npx eslint tests/retained-clients.test.ts tests/retained-clients/fixtures.ts --max-warnings=0
```

From `web/`, using the CI-supported Node major and locked dependencies:

```sh
npm run test -- src/agent/desktopCompatibility.test.ts src/agent/desktopPreparation.test.ts
npm run typecheck
npx eslint src/agent/desktopCompatibility.test.ts src/test/compatibility/desktopPreparation.release.ts --max-warnings=0
```

The existing Backend, TypeScript SDK and Frontend suites discover the new tests;
no manual-only or ignored gate is added. Heavy local Rust work uses the shared
build lock, at most two Cargo jobs and the disk floor. Dependency installation
may need the package registry; the tests themselves use no network.

## Explicitly not covered

- Historical TypeScript/Rust SDK executables, full previous desktop bundles,
  NyxID/Outreach/merchant consumers and their exact supported immutable revisions.
  A historical wire corpus is not evidence that those executables ran.
- All other REST operations, desktop capability/creation negotiation, full OpenAPI
  semantic compatibility, enum additions outside the selected responses, and the
  complete auth, discovery/conformance, MCP/A2A and Image Manager surfaces.
- AG-UI/Responses v1/v2 streams, official OpenAI Node/Python interoperability,
  two-turn identity, cursor replay, cancellation, 600-second streams, reconnect
  timing and raw proxy HTML 429/503 metadata preservation.
- Caller-object mutation during asynchronous credential resolution, committed
  writes with lost responses, cross-owner idempotency, repeated effects and stale
  settings revision rejection. Captured legacy creation shapes do not authorize
  automatic retries or prove recoverable creation receipts.
- Actual WebSocket upgrades, cookies/bearer authentication, origin/CORS policy,
  noVNC rendering/input, lease expiry/reauthentication, multi-replica exclusivity,
  bounded shutdown, transport recovery and the 20-run desktop acceptance matrix.
- Real runtime/Mongo/provider integrations, existing-client access/distribution,
  quotas under load, p95/p99 latency, 100-client/SSE concurrency or seven-day flake.
- The exact train image artifacts, public origin and retained-client production
  acceptance. An offline pass has no production sample count or image digest and
  must not be posted as a green `/measure M-FUNC-02`.

The largest safe Friday deliverable is this reviewed offline regression slice,
merged before an eligible train and linked to its CI evidence. The complete gate
cannot be closed by Friday from these fixtures: external executable cells and
owner-authorized connected acceptance remain missing. The Wednesday Secret-first
CMAEG release step preempts this work. Record train deployment/verification only
from the coordinator's actual receipts; do not infer it from merge or CI.
