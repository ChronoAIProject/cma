# Existing-Agent input rate policy

Part of #1472; implementation issue #1673 (W46 P0).

`CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED` is a strict boolean, default `false`
in code and the deployment manifest. Enablement changes the classification of
the exact registered POST routes below into the existing Input tier. Both the
local protection and the shared Mongo budget still run, with the same verified
subject key and configured `CHRONO_SERVER_RATE_INPUT_PER_MINUTE` (default 120).
Real creation continues to use `CHRONO_SERVER_RATE_CREATE_PER_MINUTE` (default 10).
No authentication, authorization, queue, command, idempotency or retry limit is
removed, and no body or caller-supplied header selects a budget.

With the separate `CHRONO_SERVER_RATE_CONTROL_ENABLED=true`, recovery and
controls of admitted work use the independent Control tier in both Input flag
modes. The Control switch is strict and defaults to false in code and infra.
While it is false, all prior classifications are preserved: v2 interrupt uses
Input only when the Input flag is true and the caller is not binding-authenticated;
other listed controls use Create. With Control enabled, its classification takes
precedence over Input: v2 interrupt uses Control for either Input flag value; its
replacement message still follows the existing domain admission checks. The flag
continues to govern input submissions exactly as shown below.

## Route inventory

Inventory checked against the registered routers, handlers and public API docs.
The Control entries below describe the explicitly enabled Control policy.
All entries require POST; other methods retain their existing classification.
Parameterized entries require the router's exact `MatchedPath` template.

| Registered route | Behavior and source | Flag off | Flag on, ordinary authenticated caller |
| --- | --- | --- | --- |
| `/api/v2/responses` | Existing Agent input; `responses/v2/{mod,routes}.rs`, `API_Responses.md` | Create | Input |
| `/api/v2/agents/{agent_id}/responses` | Same submission through the Agent alias; same sources | Create | Input |
| `/api/v1/responses` | Compatibility submission, resolves and reads an existing Agent; `responses/{mod,create}.rs`, `CMA_Public_APIs.md` | Create | Input |
| `/api/v1/agui/run` | Existing Agent message or approval answer; `agui/{mod,run}.rs`, `API_AGUI.md` | Create | Input |
| `/api/v1/agents/{id}/input` | Existing Agent message; `rest/agent_operate.rs`, `API_REST.md` | Input | Input |
| `/api/v1/agents/{id}/steer` | Input to an existing active turn; same sources | Create | Input |
| `/api/v2/agents/{agent_id}/responses/{response_id}/queued-input` | Edit an existing queued input; `responses/v2/{mod,queued_input}.rs`, `CMA_Public_APIs.md` | Create | Input |
| `/api/v2/agents/{agent_id}/responses/{response_id}/interrupt` | Cancel the selected turn and admit its replacement message through existing domain admission; `responses/v2/{mod,interruption}.rs`, `API_Responses.md` | Control | Control |
| `/api/v2/agents/{agent_id}/responses/{response_id}/controls` | Answer approvals or cancel admitted work; `responses/v2/{mod,controls}.rs`, `API_Responses.md` | Control | Control |
| `/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | Cancel an existing queued input; `responses/v2/{mod,queued_cancellation}.rs`, `API_Responses.md` | Control | Control |

Binding-key input submissions retain Create while enabled. Recovery and controls
use Control regardless of the input flag or authentication method. The middleware
marks these requests with `AuthMethod::BindingKey`; `composition/event_routing.rs`
can call `ensure_conversation_at_generation` for a new source-user conversation.
Route identity alone cannot prove an existing Agent on that branch. The limiter
does not inspect a body or perform an additional binding/database lookup to guess.
This deliberately also retains Create for binding submissions that would reuse
an existing Agent. A later narrower binding policy requires its own proof/review.

Sandbox, Workspace, Agent and conversation creation, Trigger Launch, binding
preparation and unknown POST routes remain Create. Pure cancellation, mixed
response controls, REST approval/denial, stop, interrupt and recovery use the exact
Control inventory in [Request rate tiers](CMA_Operations.md#request-rate-tiers).
Sandbox revive, capacity-recovery resume and Revive commands retain their existing
domain admission checks. The previously special Default routes (repository
resolution and connection check), health and
CORS preflight behavior are unchanged. Unknown paths ending in `input` retain the
old suffix behavior while off; the enabled policy only admits the exact inventory.
Query strings do not change a route's tier; extra segments, changed case, trailing
slashes, duplicate separators and encoded lookalikes do not match the allowlist.

## Activation and rollback

The Input flag OFF preserves the input submission policy. The separate Control
flag determines whether recovery and controls use their independent allowance. After exact-head security review and the owner's separate live policy
go, converge all API processes on the same flag
and observe tier/refusal aggregates with unchanged rates and admission controls.
Do not run production probes alongside the designated acceptance campaign.
Use its bounded harness slot; this implementation does not authorize model turns
or resource creation. Preserve original keys and distinguish operation-limiter
429 from queue-full, binding-not-ready and admission-conflict refusals.

The input and create buckets already exist. Control adds an opaque `control:` key
namespace to the same store. There is no schema, collection, index, receipt or
resource-format change, backfill, additional writer or changed reader floor.
Switching tiers does not transfer prior bucket usage: during activation a caller
can consume residual capacity in both existing buckets. Mixed flags during a
rolling change also permit both allowances; this is a bounded policy transition,
not evidence of a single combined allowance. Obtain the policy owner's go and
converge promptly; do not clear shared buckets or alter TTLs to force success.

Rollback sets the flag false on all compatible API processes. Retained Create
usage still applies and may cause immediate refusal; Input usage is retained.
Recovery and controls remain in Control when only the Input flag is disabled
and Control remains enabled. Set the Control flag false to restore the predecessor
classification without rolling back the executable. Do not
reset any budget. Request processing continues to fail closed with
`request_budget_unavailable` if shared enforcement fails. A limiter refusal
retains typed `rate_limited` and positive `Retry-After` through the current handler.

The reported 24-hour window had 890 response POST 429s. Eight sampled warnings
confirmed operation/Create misclassification; complete per-request attribution
was unavailable. Correct classification does not prove all those refusals were
caused by this limiter or repair the separate slow-successful-response defect.

## Validation

Focused tests exercise the registered templates through an Axum router, enabled
and disabled policies, aliases sharing one budget, method/path negatives, binding
provisioning protection, authenticated subject separation, anonymous fallback,
120-input/10-create local boundaries, strict configuration and custom rates.
Control tests cover both flag modes, precedence over Input, approval denial and
recovery, including controls remaining available after ten creates are exhausted.
Disposable real-Mongo tests exercise independent processes, restart, alias and
tier sharing, retained Create policy and shared-store failure without local fallback.
No live activation or production performance result is claimed by these tests.


## Prepaid Default budgets

`CHRONO_SERVER_RATE_DEFAULT_LEASE_ENABLED` accepts only exact `true` or `false`
and defaults to `false`, including the deployment manifest. Activation is a
separate owner decision. When disabled, the shared Mongo command sequence and
all tier classifications stay unchanged. Create, Input, Control, the local
admission guard, and health requests never use leases.

When enabled, Default requests first pass the existing local guard. Each key
uses credits already atomically deducted from Mongo's GCRA bucket using the
server clock and majority writes. A process reserves at most
`max(1, per_minute / 20)` credits (30 at 600/minute), with a 10-second lifetime
including the Mongo wait. The previous 10-second observation window's arrival
count chooses the next batch, clamped to that bound; a new or inactive key starts
at one. Requests consume remaining credits locally. Empty leases refill under
one per-key async lock. A refused multi-credit charge immediately retries one
credit and preserves that result and `Retry-After`; a database error does not
retry locally or create credits.

The table holds at most 16,384 keys. New keys beyond capacity use exact per-request
Mongo charging. Idle expired entries are reclaimed on insertion; an in-flight
or waiting caller pins its entry to prevent duplicate concurrent refills.
Expired unused credits are discarded, never refunded. Restart also discards
remaining prepaid credits. Across replicas, admitted requests cannot exceed the
credits Mongo charged; wasted credits make admission stricter. One expiration
or restart wastes at most one lease's remaining credits per key per process.
This is a prepaid-accounting guarantee: a credit can be consumed up to ten
seconds after its Mongo debit, so enabled request timing is not identical to
per-request GCRA timing. Include that bounded shift in activation load tests.
An outage can spend only credits paid before the failure, until their expiry;
all attempts requiring a new charge fail with the existing unavailable response.

`cma_rate_default_decisions_total{source="leased|charged|refused|unavailable"}`
counts outcomes using only these fixed source values. `charged` counts admitted
requests that needed Mongo, not the number of Mongo commands (a failed batch can
need a second charge). `cma_rate_default_lease_discarded_total` counts observed
expired/evicted credits; a process crash cannot emit a discard metric. No metric
or lease log adds keys, subjects, or network addresses.

Before activation, retain the analyst's production command/checkout measurements
and compare request-rate-limit command/request ratios with the switch off and
on. The initial 10-second/30-credit settings are bounds, not a promised command
reduction. Compare the no-I/O route's latency and Mongo checkout wait in the same
window. Roll back with `CHRONO_SERVER_RATE_DEFAULT_LEASE_ENABLED=false` and restart.
Bucket format, indexes, and TTL are unchanged; discarded credits age out through
the existing GCRA/TTL behavior and need no data repair.
