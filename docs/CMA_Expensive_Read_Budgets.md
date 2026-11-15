# Expensive read admission

The public control plane applies independent per-process budgets to buffered Agent,
document and retained report descriptor reads. This is the expensive GET slice of #1472/#1596. It complements
the existing network, authenticated-operation and identity-verification limits.

## Contract

- Each lane admits at most two selected GET/HEAD requests per process using
  nonwaiting semaphores. Excess requests receive `429 buffered_read_capacity_exhausted`
  before downstream activity, rate-limit storage, resource storage or provider
  work. This shared-capacity refusal is distinct from `429 rate_limited`, which
  retains its caller allowance semantics. Diagnostics name only the fixed lane
  and its refusal count, sampled at powers of two.
- Bulk history/document reads retain a 64 MiB ceiling, matching native history
  export. Artifact list and individual metadata responses retain 256 KiB.
  Agent usage/resources use a separate two-slot lane with 64 KiB per response.
  Report descriptors use another two-slot lane with 1 MiB per response.
  Bulk and Agent observations return `Retry-After: 1`; report descriptors return
  `Retry-After: 2`. No request waits in a capacity queue.
  Response collection checks actual bytes, including chunked bodies and bodies
  with an absent or incorrect Content-Length, before releasing any payload.
- Handler execution and body collection share a 120-second deadline for bulk
  and Agent observation reads; report descriptors share a five-second deadline.
  Existing shorter route/service/provider deadlines still apply. Timeout, body failure
  or oversized output returns typed `503 request_budget_unavailable` without
  releasing partial payload or internal error text.
- A permit survives handler completion and remains attached to the response
  body until consumption or drop. Cancellation, deadline and error paths release
  it. No waiting queue or detached capacity-release task exists.
- POST/PUT/PATCH/DELETE, including stop and revoke, do not consume this read
  allowance. Their existing authentication, authorization and rate limits apply.

Covered Agent routes: events, transcript, history, usage, resources, artifact
list/metadata/download, and file tree/content/download. All document GET/HEAD
routes are covered. Report descriptor admission covers exactly GET/HEAD
`/api/v1/usage/metrics/view`, `/api/v1/admin/usage/metrics/view`, and
`/api/v1/{admin/}usage/metrics/reports/{report}`. Series, Sandbox pages/detail,
owner pages and owner-allocation projections retain their existing contracts;
valid grouped series can exceed 1 MiB and do not inherit the descriptor cap.
Classification uses Axum's matched route template so query parameters and
encoded resource identifiers cannot bypass admission.

Authentication runs before this budget. Passing admission grants no resource
authority: the existing owner/share checks and final policy fences still decide
whether a response may be returned. Refusals are private, typed responses with
no identity, credential, path or provider error added to logs.

## Artifact metadata

The store's list contract returns metadata records with retained ancestry and
publication evidence. MongoDB uses the same inclusion projection as individual
metadata reads, excluding `record.content_base64` at the database boundary.
The in-memory store likewise copies metadata only. Pagination remains ordered
and bounded to `limit + 1`, with a maximum public page of 50.

The service validates every selected metadata record, including lookahead,
against its binding, order and historical publication evidence, then rechecks
current authorization. A metadata page does not attest payload integrity.
Explicit download still reads and verifies the immutable content length/digest
before the final authorization fence. Corrupt metadata refuses a page; corrupt
payload refuses download without forcing metadata reads to decode the payload.
There is no schema migration, persisted budget state or cached authorization.

## Evidence and remaining qualification

Local tests cover successful reads, 1,000 saturation refusals without downstream
work, control admission during saturation, encoded paths, GET/HEAD, exact byte
limits, missing/false length headers, body errors, cancellation, handler/body
deadlines and permit recovery. Existing artifact tests retain owner/shared
success, denied callers, revocation after I/O, stale ancestry and deadline
coverage. Disposable Mongo tests cover projected pages, corrupt metadata and
corrupt downloads after reopening the store.

The selected response payload ceilings per process are bulk 2 × 64 MiB =
128 MiB, Agent observations 2 × 64 KiB = 0.125 MiB, and report descriptors
2 × 1 MiB = 2 MiB: 130.125 MiB total. Each lane admits at most 2N reads at
N replicas (6N across all three lanes); this is not a pooled cluster semaphore.
A hot process can refuse while another has spare slots. Caller rate limits stay
shared across replicas and unchanged. The optional retained-frame policy extends
permit ownership beyond body consumption while response byte clones remain alive.

Two bulk slots remain intentional because increasing them doubles large payload
exposure for each extra pair. A tab's concurrent usage and transcript previously
consumed both slots; two such tabs could refuse each other. Separating fixed-shape
usage/resources allows two transcript and two usage reads together with only
128 KiB extra observation payload. Usage is not exempt: its fallback can still
scan historical events. Report descriptor polling likewise remains bounded and
independent. This is admission sizing, not a measured throughput/RSS guarantee.
The matched frontend fixture measured 30 HTTP reads/minute with desktop open and
28 with it closed, well below the declared shared Default allowance of 600/minute;
no caller-rate allowance increase is justified by that fixture.

This is not an aggregate RSS measurement: store/provider working
memory, decoder allocations, transport buffers, other routes, SSE and desktop
streams have separate ownership. Agent log scan amplification remains with
#1475/#1591; this change does not claim the event scanned/returned byte ratio,
global ingress limit, RSS below 70%, or full M-CONC-02 qualification.

The release owner records candidate SHA/image digest, configuration, environment,
time window, sample count and raw evidence for approved synthetic read/slow-reader
checks. Verify admitted reads, typed saturation, response-drop recovery, stop and
revoke availability, metadata page size and absence of content, and process RSS.
Application contributors do not operate production to collect this evidence.
Rollback requires a compatible reader for all other changes in the release;
removing this budget restores the previous unbounded read-concurrency behavior.
