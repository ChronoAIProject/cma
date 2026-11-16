# API supplement author handoff

Issue #2149; integration PR #2200 is owned by Developer15. The frozen assignment
is **87 API operations**, counted separately from the 275 checklist rows. The
former 14 manual admin rows were reassigned; this directory exports no C275 rows.
All API:API-SUPPLEMENT-001 through 087 are authored, preserving step_1,
step_2, step_3 and expected_result from the frozen catalog.

| Family | Operations | Count | Assertions and setup |
| --- | --- | ---: | --- |
| Profiles, marketplace, authoring, Event Worker Profiles | 003–011, 036–041, 044–048, 053–061, 072–080 | 38 | Actual changed drafts, immutable revisions, permissions, public/private visibility, forks, idempotency, retirement, retained receipts after restart; 047–048 require pinned authoring configuration. |
| Responses, AG-UI, conversations | 024–028, 030–032, 049–052, 084–085 | 14 | Raw bounded SSE, exact response/run identity, ordered frames, pagination, native effect counts, interruption, original-key replay, outsider denial, retained response after restart. |
| Capacity recovery | 013–017 | 5 | Original immutable intent, real unknown admission outcome, reconciliation, exact attachment and retained data; no input or preparation replay. |
| Agent metadata | 012, 018–023 | 7 | Artifact hash/bytes, provenance, provider/parent lineage, exact feedback, deterministic deliverable references, terminal refusal, no runtime mutations. |
| Documents | 034–035 | 2 | Exact committed source/history hashes, two owners' comments, exact target deletion, stale revision and denial, other content preserved. |
| Capability manifests and telemetry | 029, 066, 083, 086–087 | 5 | Independent exact-image manifest, current account identity, authentication boundary, content-free event count. |
| Launches and Triggers | 042–043, 067–068 | 4 | Public placement redirect without a new Agent, launch cancellation/history, distinct unpublished Trigger deletion, owner isolation. |
| Sandbox/Workspace/Agent hierarchy | 033, 062–063, 081–082 | 5 | Real hierarchy creation and defaults, pinned profiles, original receipt replay, nested parent joins, worktree readiness, native creation counts, reverse cleanup. |
| Delegation and service connections | 001–002, 064–065 | 4 | Linked grant identity, inner admission receipt, wrong claims/revoked/expired/unavailable provider, exact handoff lifecycle, scope isolation, no credential fields. |
| Usage | 069–071 | 3 | Independent closed allocation intervals, paginated resources, CPU/RAM/disk unit calculations, paused disk accounting, owner totals and isolation. |

`index.mjs` exports the cases plus `setups` and `requiredPrimitives`.
`setups.mjs` implements document preparation through public APIs: reserve cleanup
custody, create source, commit changed source, grant a recipient comment access,
and create separately attributed owner/recipient comments. It also validates
independently pinned profile-authoring configuration. These providers do not
launch an application or connect to a database. The document setup reuses the
shared `disposableDatabase` lease and `identities` primitive contract.

`fixtures.json` lists the exact fields, methods, controls and native facts for
all ten named setups. The exported providers require:

- `disposableDatabase`: scope LOCAL_DISPOSABLE_TWO_AM, disposable flag, and a
  reserve/retainIntent/retainDocument/release lease with a parent cleanup barrier.
- `identities.user`, `.recipient`, `.outsider`: distinct synthetic subjects with
  credentials retained exclusively by the transport.
- `profileAuthoring`: independently pinned synthetic repository request/commit
  and safe services catalog, marked PARENT_PINNED_SYNTHETIC_CONFIGURATION.

The other eight named setups still require authentic parent implementations:
capacity-recovery, compatibility, delegation, agent-metadata, capabilities,
hierarchy, launches and usage. The JSON contract is a requirement, not an
installed binding. Missing fixtures/data/methods produce BLOCKED. In particular,
the inspected recovery runtime proof producer is unsupported; recovery must
remain blocked until a real approved producer exists. Streaming needs raw SSE
and safe response headers, a deterministic provider, hold/release controls and
independent input/interrupt counts. Service delegation needs scoped synthetic
broker claims; capabilities need a separate exact-image manifest and isolated
telemetry collector. Never construct expected evidence from the tested response.

The transport must include user, outsider and profile-denied actors; user and
outsider can author their own profiles, profile-denied cannot. Marketplace
policy and the managed model defaults must already be configured by the parent.
The sole local campaign owner installs these bindings; no second app is started.

Routing requires shared fix f5d2d08d3 or newer. Agent requests omit am to use the
independently observed non-owner, unless an explicit opposite ingress is needed.
Cross-AM assertions derive the next ingress from the preceding actual response
receipt and validate the returned timing.am. Global stores use both ingresses
without inventing Agent ownership. All calls pass through ctx.request so owner,
epoch, request ID and ingress evidence remain attributable.

Run the author checks from the repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-04/*.test.mjs
node --test .github/scripts/multi-am-features/executable/*.test.mjs
git diff --check
```

The author suite has **32 passing offline verifier tests**. All 87 full flows
run against explicit transport models. Fault tests reject wrong content,
private-list leakage, mutable published history, missing allocation entries,
duplicate native effects despite a replayed receipt, stream ordering/identity
errors, missing setup methods and cleanup/custody failures. Setup tests exercise
partial creation failures and require cleanup registration before mutation.
These results establish verifier behavior only; **live feature PASS remains
unproven for all 87 operations**.

The parent must run N=3 before restart and N=1 after, bind exact source/image,
prove both real AMs and fresh owner epochs, retain original failures, and prove
final namespace disposal. Retained profile receipts/revisions and the main
compatibility Agent/response survive the restart boundary. Compatibility setup
must explicitly transfer the retained main Agent to parent cleanup custody;
new conversation children are cleaned after each invocation. Merely declaring
retention is insufficient for campaign acceptance.

Cleanup is registered before creation where possible and preserves original
resource IDs. An accepted response attached to an attribution error retains its
ID. Unknown creation is never retried with a new key; it fails cleanup or is
reconciled by the reserved parent namespace. Public profile retirement and
hierarchy cleanup run in reverse custody order; cleanup errors preserve the
primary failure. Documents and other resources without deletion APIs require
parent namespace teardown evidence. No author scratch process or application
is left running. The author worktree remains under lead cleanup ownership.

No default-branch merge or runtime result is claimed by this handoff. Developer15
owns integration and the existing PR. Raw credentials, event capability values,
synthetic bodies and private runtime receipts remain in the protected evidence
sink; public reporting contains only safe counts, classifications and hashes.
