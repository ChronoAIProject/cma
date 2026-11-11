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

## Route inventory

Inventory checked against the registered routers, handlers and public API docs.
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
| `/api/v2/agents/{agent_id}/responses/{response_id}/interrupt` | Cancel the selected turn and admit its replacement message; `responses/v2/{mod,interruption}.rs`, `API_Responses.md` | Create | Input |

Binding-key submissions retain Create while enabled. The authenticated middleware
marks these requests with `AuthMethod::BindingKey`; `composition/event_routing.rs`
can call `ensure_conversation_at_generation` for a new source-user conversation.
Route identity alone cannot prove an existing Agent on that branch. The limiter
does not inspect a body or perform an additional binding/database lookup to guess.
This deliberately also retains Create for binding submissions that would reuse
an existing Agent. A later narrower binding policy requires its own proof/review.

Sandbox, Workspace, Agent and conversation creation, Trigger Launch, binding
preparation and unknown POST routes remain Create. Pure cancellation, mixed
response controls, REST approval, stop and non-message interrupt routes are not
new input aliases in this slice and retain their existing budgets. The previously
special Default routes (repository resolution and connection check), health and
CORS preflight behavior are unchanged. Unknown paths ending in `input` retain the
old suffix behavior while off; the enabled policy only admits the exact inventory.
Query strings do not change a route's tier; extra segments, changed case, trailing
slashes, duplicate separators and encoded lookalikes do not match the allowlist.

## Activation and rollback

Deployment OFF preserves existing policy. After exact-head security review and
the owner's separate live policy go, converge all API processes on the same flag
and observe tier/refusal aggregates with unchanged rates and admission controls.
Do not run production probes alongside the designated acceptance campaign.
Use its bounded harness slot; this implementation does not authorize model turns
or resource creation. Preserve original keys and distinguish operation-limiter
429 from queue-full, binding-not-ready and admission-conflict refusals.

The input and create buckets already exist. There is no schema, collection,
index, receipt or resource-format change, backfill, new writer or reader floor.
Switching tiers does not transfer prior bucket usage: during activation a caller
can consume residual capacity in both existing buckets. Mixed flags during a
rolling change also permit both allowances; this is a bounded policy transition,
not evidence of a single combined allowance. Obtain the policy owner's go and
converge promptly; do not clear shared buckets or alter TTLs to force success.

Rollback sets the flag false on all compatible API processes. Retained Create
usage still applies and may cause immediate refusal; Input usage is retained.
Do not reset either budget. Request processing continues to fail closed with
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
Disposable real-Mongo tests exercise independent processes, restart, alias and
tier sharing, retained Create policy and shared-store failure without local fallback.
No live activation or production performance result is claimed by these tests.
