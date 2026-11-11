# Public stream admission

Part of #1472; implementation slice #1678. Runtime mapping and measured capacity
remain under #1487. `CHRONO_PUBLIC_STREAM_LIMIT` defaults to `0` (disabled).
An explicit integer from 1 through 4096 enables a shared **per-process** cap.
The validation maximum is not a qualified operating value.

## Scope and behavior

A slot is acquired without waiting before the selected handler parses its body,
projects history, admits a command or resolves a binding. It remains charged
while the handler waits for headers and throughout the response body. Normal
end, body error, cancellation, disconnect or drop returns the slot. Body errors
terminate the wrapper; later frames cannot continue after releasing capacity.
Frames, trailers and headers are forwarded without collection or prefetch.

All route/router clones share the same semaphore. The layer is installed before
the internal delegation router receives its clone, so delegated Responses
submissions consume the same capacity as direct calls. Existing authentication,
resource authority, per-frame revocation, operation rate limits and distributed
rate enforcement retain their paths. The cap neither authorizes a request nor
changes history/command identity or replay semantics.

At saturation the handler does not run. The response is HTTP 429 with code
`public_stream_capacity_exhausted`, `Retry-After: 1`, and `Cache-Control: no-store`.
It contains no occupancy or fleet counts. Reconnect with the durable cursor or
retry the original input idempotency key. The cap has no admission waiting queue;
it does not stop admitted work when a consumer disconnects.

## Exact route inventory

Classification uses the router's `MatchedPath` and method, never a URL suffix,
query parameter, `Accept` header or a guess from the request body.

| Methods | Registered template | Implementation |
| --- | --- | --- |
| GET / HEAD | `/api/v1/agui/events` | `layer1-public/src/agui/replay.rs` |
| GET / HEAD | `/api/v1/agui/launches/{id}/events` | `agui/launch_companion.rs` |
| GET / HEAD | `/api/v2/agents/{agent_id}/events` | `agent_v2/stream.rs` |
| GET / HEAD | `/api/v2/responses/{response_id}/events` | `responses/v2/stream.rs` |
| POST | `/api/v1/agui/run` | `agui/run.rs` uses AG-UI replay |
| POST | `/api/v1/responses` | `responses/create.rs` uses Responses streaming |
| POST | `/api/v2/responses` | `responses/v2/routes.rs` |
| POST | `/api/v2/agents/{agent_id}/responses` | Same v2 submit handler |

All requests on those POST routes are charged, including nonstreaming and
invalid submissions, because classification precedes handler effects. HTTP
errors return their slot through the same body lifetime. HEAD follows GET's
admission because Axum can run the same handler before discarding the body.

Recovery controls (stop, cancel, interrupt, approval, queued cancellation),
ordinary input routes without streaming, document revoke, resource creation,
discovery and history/snapshot reads do not consume this cap. They retain their
other limits. Responses v1 retrieve rejects streaming query options; v2 retrieve
is JSON, and v2 events is the streaming route. Neither retrieve is included.
Desktop WebSocket transport is a separate runtime capacity contract. New public
stream routes must extend this inventory and its route coverage tests.

## Activation and rollback

Keep the flag at zero until the capacity owner supplies an exact candidate,
chosen cap, process count including rolling overlap, public-to-runtime mapping,
normal controls baseline and a reviewed bounded acceptance window. A fleet of
N processes can admit up to N times the configured value; this is deliberately
not a distributed lease. Sticky routing or replica count alone does not prove
runtime capacity. Changing this value does not raise any existing rate budget.

Deploy the compatible binary with zero first. Enable only through the lead's
release/activation relay and verify effective configuration for every process.
Existing streams in older processes remain governed by their original policy;
do not sever their bodies or restart active Sandboxes to force convergence.
Rollback to zero removes this admission cap without changing durable schemas or
cursor format. Respect ordinary continuity/drain requirements for any rollout.

The proposed acceptance phase is limited to two simultaneous observers, ten
requests, two minutes total, ten seconds per request and 45 seconds per open
body. It uses only existing fixtures, with zero new Sandbox or model turn and
at least 1.05 seconds between request starts. Keep the harness's response-byte
cap and reserve two requests for final owner controls; closing bodies is local
cleanup, not another fixture write.

Saturation proof requires a capacity-owner-approved process isolated to test
traffic, configured with cap one. Hold one observation, verify one immediate
excess refusal, close it, then verify recovery. Do not lower the shared
production cap or fill it with observers to obtain this result. If isolated
routing/configuration is absent, saturation remains blocked; low-count checks
on the shared candidate prove only access and disconnect behavior.

Verify normal owner read controls and foreign denial, record exact process
routing/candidate and close every test-owned observer. Abort on protected
bytes, unexpected handler effects, unexplained 5xx, control failure, leaked
observers or cancellation failure. Use the reviewed security harness and a
coordinated slot; source delivery does not activate the flag or authorize a
configuration change. Do not repeat passing controls without acceptance value.

## Evidence boundaries

This cap bounds concurrent handler/response lifetimes only. It does **not**
establish a 128 MiB decoded/replay memory bound, total RSS, durable ingress
backpressure, fairness between owners, active-turn capacity, or physical N+1
reserve. Those #1472 gaps remain open. Hosted ingress can still retain unbounded
updates, and legacy projection can retain complete history within each admitted
stream. The response projection workstream owns bounded Responses v2 reads;
its implementation is not duplicated here.

Local tests cover default-off/invalid config, all eight route templates, HEAD,
concurrent saturation before handler effects, ordinary controls, cloned internal
dispatch, slow header cancellation, lazy body consumption, disconnect, error,
trailers, EOF and retry after release. Local contracts do not replace the
candidate-bound production campaign or its teardown receipt.
