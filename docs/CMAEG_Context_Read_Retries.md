# Legacy Custom context-read retries

The gateway checks retained admission before sending a legacy Custom event to
CMA. Only an explicit context GET 404 proves absence. A timeout does not prove
that a previous attempt failed to admit the event.

`CMAEG_CONTEXT_RETRY_ENABLED` defaults to false. When enabled, the legacy
`cma_codex` Custom path makes at most two sequential context GETs. Each attempt,
including body transfer and credential tracking, is bounded to three seconds;
the entire observation is bounded to eight seconds. A transient transport
failure, timeout, HTTP 429 or HTTP 5xx permits one retry after at least 500 ms
plus zero to 500 ms jitter. Retry-After seconds and HTTP-date minima are honored.
A minimum that cannot fit a full attempt into the remaining budget is returned
to the producer without probing early. An unparseable Retry-After disables the
automatic retry.

Explicit 401/403, other HTTP refusals, invalid configuration, malformed context,
identity mismatch and retained-content conflict never trigger this automatic
retry. Failure remains visible. Context bytes, credentials, event identifiers
and arbitrary upstream error text are not logged; attempt logs contain only
attempt count, elapsed time, an allowlisted outcome/error class and HTTP status.

The two attempts use the same binding, conversation, event and credential. A
GET retry never changes the provider submission key, rendered Activity or reply
ownership. No provider POST occurs until absence is established; an exact
retained event acknowledges its original admission without emitting its output
again. Changed retained content still returns 409. The same bounded read policy
also applies when concurrent first submissions reconcile a command conflict.

If the read cannot establish absence or retained admission, the request remains
HTTP 503 `provider_context_unavailable`. Transient failures with a known retry
minimum include Retry-After; authorization and malformed-context failures do not.
The problem JSON shape is unchanged. The elapsed acceptance budget does not
turn this refusal into 202, and no failure callback is emitted for a turn that
has not been submitted.

## Producer contract and durable queue boundary

A producer may retain the exact event and retry after the advertised minimum,
with bounded exponential backoff and jitter. It must keep the event identity,
content, producer timestamp and idempotency identity, and refresh only its ingress
authentication/signature as required. A 409 needs conflict resolution, not a new
event ID generated to bypass retained admission.

This is read retry within one ingress attempt, not durable gateway queueing.
Nothing has been committed when the legacy preflight fails. Returning queued or
202 at that point would acknowledge work that can disappear on process loss.
The existing routed queue is not a transparent substitute: routing adds an
Activity routing snapshot and has different execution/reply recovery contracts.
Existing channels retain their saved execution mode. Do not enroll them or change
retained Activity bytes as a timeout mitigation.

True durable legacy queueing requires a separate reviewed admission/reader
contract, original-event reconciliation, exact reply ownership, bounded capacity
and retention, leases, crash recovery and old/new reader evidence before its
writer can be enabled. This retry flag writes no new journal or retained format.

## Deployment and measurement

The API companion is PR #1693 from `tl/cmaeg-release/context-lookup`. Its
`CHRONO_CMAEG_RETAINED_CONTEXT_LOOKUP_ENABLED` flag optimizes the same retained
context route and preserves its authority fence. The gateway change has no API
source overlap and requires no changed response shape. Both flags remain off
until the release owner approves their separate rollouts and exact artifacts.

Retrying can add one context GET and hold an ingress permit longer. It cannot
repair sustained API slowness. First measure the API lookup improvement and
remaining timeouts, then compare matched natural-traffic windows before enabling
this flag. Record first/second-attempt counts, deadline exhaustion, retained/absent
results, upstream status classes, ingress latency and capacity refusals. Keep
producer retries and distinct event outcomes separate from request counts.

Local coverage exercises actual HTTP ingress and provider sockets: slow read
then absence, retained replay and conflict, auth refusal, malformed response,
transient exhaustion, Retry-After, default-off behavior and no duplicate provider
admission/output. Paused-time tests cover deadline/backoff bounds and cancellation.
These fixtures do not establish production latency or capacity.

Rollback disables the flag and restores the single-read path; no journal or
schema rollback is required. Production execution belongs to the release owner.
Keep #1665 open until the reviewed API/gateway artifacts are deployed and matched
production evidence confirms the scoped improvement without duplicate or lost
accepted work.
