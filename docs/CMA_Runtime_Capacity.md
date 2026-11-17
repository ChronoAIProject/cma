# Runtime stream and request capacity

Issue #1487 (M-SCALE-06) fixes the native adapter's process-local mapping.
This is an implementation contract, not evidence of the M-SCALE-04 or M-CONC-01
production load gates. Activation follows [staged scale apply](CMA_Scale_Apply.md).
No fleet-wide Ready-instance admission gate is introduced; HPA min/max are unchanged.

## Mapping and lifetime

One `RuntimeCapacityAuthority` is injected into the native production adapter and
the independently constructed sandbox-metrics client. Clones and transports
share that authority. Creating another client does not multiply process capacity.
The scope is one control-plane process, across all its native Sandboxes/Agents.
Retained OpenSandbox transports keep their existing transport contract.

| Operation | Runtime stream permits | Runtime request permits | Sharing / class |
| --- | --- | --- | --- |
| New Agent conversation link | 1 until the transport closes | 1 during Open and each separate HTTP operation | Ordinary; receiver ownership negotiation is continuity |
| Retained/admitted Agent conversation reopen | 1 until the transport closes | 1 per HTTP operation, including nested status reads | Continuity: restart adoption, owner handoff, rolling takeover, revive/recovery |
| N public observers of the same Agent (AG-UI, Responses, v2 Agent events) | 0 additional | 0 for subscription/replay | Share the Agent owner's single runtime conversation; each has a separate public body and live subscription |
| Conversation input, interrupt, cancel, approval response on an existing link | 0 additional | 0 additional for the multiplexed conversation protocol | Existing stream permit; durable command receipt remains authoritative |
| Desktop WebSocket | 1 for the connection | Separate discovery/control requests use 1 each | New desktop admission is ordinary; existing desktop ownership rules still apply |
| Runtime status/resources, file/environment/history reads, model/skill capability probes | 0 | 1 per in-flight native HTTP operation | Ordinary; nested reads in an admitted continuity scope inherit continuity |
| Lifecycle, stop/close, credentials, ownership | 0 unless reopening a link | 1 per in-flight native HTTP operation | Control (ownership/recovery reads use continuity) |
| File writes/deletes, attachment upload, browser observation/control | 0 | 1 per native HTTP operation | Ordinary; these do not borrow lifecycle reserve |
| Native startup verification/recovery exec | 0 | 1 for the exec operation | Control; shares the same request authority |

HTTP connection setup borrows proof of the operation's existing request permit;
it never acquires a second permit. An idle cached connection holds no request
permit. The existing **16 native HTTP slots per transport**, across its control
and ownership lanes, remain an independent bound. Idle slots can be evicted;
16 busy slots yield a typed request-capacity refusal. Thus 32 physical request
permits do not promise 32 simultaneous distinct-runtime HTTP exchanges. Per-lane
checkout remains serialized under the existing bounded operation deadlines.

In replica mode, each public observer holds one relay slot on its ingress
process and, if remote, one on the Agent owner. An observer on the owner uses
one total. The owner middleware retains its slot through the response body;
the live handler does not charge it again. The public stream limit also covers
the complete response body. Relayed synchronous requests hold one slot on each
participating process. Reads, uploads and file/browser edits are ordinary; skill lifecycle commands
and lifecycle reopens use control capacity. These counts must be included at **both** ends
when sizing skewed traffic. No public observer creates a runtime conversation.

## Defaults and activation profile

Every value is per process. All numeric values are strict unsigned decimal
integers; malformed values, limits outside bounds, or reserve >= limit fail
startup before serving traffic. Limits/reserves cannot resize a live authority.

| Config key | Deployment default | Recommended activation candidate | Validation |
| --- | --- | --- | --- |
| `CHRONO_RUNTIME_STREAM_LIMIT` | 64 | 64, pending owner sizing decision | 1..4096 |
| `CHRONO_RUNTIME_REQUEST_LIMIT` | 32 | 32, subject to request qualification | 1..4096 |
| `CHRONO_RUNTIME_STREAM_RESERVE` | 0 | 16 | 0..limit-1 |
| `CHRONO_RUNTIME_REQUEST_RESERVE` | 0 | 8 | 0..limit-1 |
| `CHRONO_RUNTIME_REQUEST_QUEUE_LIMIT` | 0 (disabled) | 64 | 0..1024 |
| `CHRONO_RUNTIME_REQUEST_QUEUE_MAX_WAIT_MS` | 1000 (inert with queue disabled) | 1000 | 1..60000 |
| `CHRONO_AGENT_MANAGER_RELAY_STREAM_LIMIT` | 256 | 384 | 1..4096 |
| `CHRONO_AGENT_MANAGER_RELAY_CONTROL_RESERVE` | 0 | 64 | 0..limit-1 |
| `CHRONO_PUBLIC_STREAM_LIMIT` | 0 (off) | 320 | 0..4096 |

The checked-in ConfigMap keeps the deployment defaults. The authority-sharing
and HTTP accounting corrections are active immediately. Reserve partitioning,
request queueing and public stream admission require explicit staged activation.
The validation ceiling of 4096 is **not** a qualified operating limit. Raising
physical limits requires memory/ingress and provider qualification against the
existing **32 MiB live** and **128 MiB ingress** budgets, including their required
feature gates, retained-runtime ACK support, and ordinary non-stream RSS.

## Enforcement and N+1 arithmetic

Let `S = stream_limit - stream_reserve`, `R = request_limit - request_reserve`,
`A` be simultaneous active conversation links plus desktops and other ordinary
stream consumers, and `Q` be concurrent runtime request operations (not public
request rate and not observer count). With qualified uniform placement:

```text
required Ready instances = max(ceil(A / S), ceil(Q / R)) + 1
ordinary stream cap after losing one instance = (Ready - 1) * S
ordinary request cap after losing one instance = (Ready - 1) * R
```

For the candidate 64/32 with 16/8 reserves, S=48 and R=24. The 200-active target
requires `ceil(200/48)+1 = 6` Ready instances: after one loss, 5*48=240 usable
streams, leaving 40 ordinary slots plus the separate 5*16 continuity/control
reserve. **Current HPA max 4 gives only 3*48=144: 200-active N+1 is NOT met.**
Even neutral defaults give 3*64=192. The product owner must choose a larger
replica shape or a higher qualified physical stream limit. With four replicas,
the mathematical minimum is 67 usable streams/process (83 physical with reserve
16), giving only one aggregate spare; this is not an operating recommendation.

Requests require their own measured concurrency envelope: Q=120 needs six
Ready instances by the semaphore formula; Q=200 needs ten. The 16-slot HTTP
cache, per-runtime serialization, Mongo, provider and ingress budgets can lower
the effective request capacity further. No test here claims a measured Q or an
SLO. SRE must qualify the target mix, including existing NyxID/direct consumers,
recovery bursts, ingress placement, owner skew and desktops. Reserve is shared
headroom for continuity/control, not an additional independent pool for each.

300 public observers add zero runtime permits. With relay 384/reserve 64,
320 ordinary relay slots are available per process; 300 observers of one owner
leave 20 ordinary slots and 64 reserved control slots on that owner. Mixed
ingress/owner traffic can consume more than one relay slot per observer across
the fleet. Per-process admission remains authoritative under skew; aggregate
arithmetic alone cannot guarantee placement. Already admitted traffic is never
evicted to enforce a new ordinary limit.

Ordinary admission may use only the non-reserved physical capacity. Continuity
and control bypass the ordinary request FIFO and may take **any free physical
permit**, including reserve. No stream permit waits in a queue. With the request
queue enabled, at most 64 ordinary waiters in the candidate profile receive FIFO
service, with a 1 s absolute queue deadline. Cancellation removes a ticket
synchronously and releases any held permit. Queue full, deadline expiry, physical
exhaustion and busy HTTP slots are distinct low-cardinality refusal reasons.

Before public headers/command admission, exhaustion returns HTTP 429 with
`Retry-After: 1`, using `runtime_request_capacity_exhausted`,
`runtime_stream_capacity_exhausted`, `relay_observer_capacity_exhausted` or
`relay_capacity_exhausted`. Existing `public_stream_capacity_exhausted` remains.
No capacity detail reveals identities or occupancy. A lost owner/live lane keeps
durable replay and reconnection available; only observed relay saturation refuses
a new observer. Once accepted, reconnects retain the subscriber and durable cursor.
Approval responses can fall back to durable replay if a fresh live observer cannot
fit; observer saturation cannot block their control command. Private owner calls
preserve typed runtime capacity or a stable `relay_capacity_exhausted` detail
through the existing recoverable failure path.

An admitted worker whose continuity reopen finds physical capacity exhausted
keeps its existing command/open receipts and gets the existing retryable
`agent_runtime_unavailable`/RuntimeUnavailable outcome with stable capacity detail.
No new persisted enum is written. Existing `agent_resource_quota` remains for
actual resource quota failures. Saturation does not prove runtime loss, stop an
Agent, replace its home, acknowledge uncommitted output, or discard a terminal
event. Durable replay and the existing runtime output ACK protocol remain the
terminal-event authorities; ephemeral live fragments retain their gap semantics.

## Observations and deterministic evidence

The metric inventory is [CMA Observability](CMA_Observability.md). Adapter gauges
report physical/reserved/usable limits, occupancy, availability and queue depth;
counters report acquisitions/refusals by kind/class/reason, and a histogram
reports queue wait by outcome. Relay/public admission have occupancy and refusal
counters. No Agent, owner, Sandbox, request ID or URL is a metric label.

Local tests cover all three workload classes at full ordinary capacity, physical
exhaustion/release, FIFO/timeout/cancellation, neutral config and invalid startup
values, N+1 arithmetic, native new-connection setup with the last request permit,
the typed 16-slot HTTP refusal, shared observation, remote relay saturation,
single owner-body accounting, and public typed error contracts. These fixtures
use local HTTP and in-memory stores; they do not qualify k6/staging load or RSS.

## Release and recovery record

Migration identity: `2026-11-16-am-runtime-capacity-1487`; owner: Agent Manager /
Runtime capacity maintainer; execution: authorized deployment operator. Phase:
additive implementation with neutral admission defaults; activation is separate.

| Required migration field | Evidence / requirement |
| --- | --- |
| Affected stores and writers | No schema, index, validator, TTL, home, journal, client journal or runtime wire change. Existing worker failure writers may persist capacity detail in the existing RuntimeUnavailable/string fields. |
| Preservation and reader floor | Existing Agent IDs, conversations, receipts, command identities, runtime pins and homes survive. No new persisted enum; the preceding develop release reads the existing shapes. This source compatibility statement is not an executed predecessor-image recovery qualification. |
| Source / executable | Record exact commits, candidate digest and qualified recovery digest in the operator activation packet; no image was built/deployed as part of local implementation. Recovery must also meet every other release-batch reader floor. |
| Writer gate and cohort | All reserve/queue/public admission defaults remain neutral/off. Enable the candidate values only after compatible serving/worker readers and capacity/continuity qualification, per `CMA_Scale_Apply.md`. |
| Backfill / backup | No transformation or deletion: no backfill or data backup operation is needed for this change. Existing operational backup requirements remain. |
| Rollout | Deploy neutral values first, observe counters and ownership handoffs, then enable one bounded cohort. Do not lower available capacity below retained-link takeover demand; halt new admissions on failures and retain accepted work. |
| Rollback | Restore neutral reserves/queue/public limits with a continuity-qualified rolling rollout; keep physical limits sufficient for retained work. Disabling admission cannot evict accepted work. Keep runtime images/homes unchanged; prefer a compatible forward fix. |
| Completion limits | Local deterministic tests are implementation evidence only. Candidate/production image matrix, SRE load, memory, ingress, provider limits, placement and N+1 qualification remain unverified until their owners record results. |

Product-owner decision remains open: replica shape versus qualified physical
stream limit. Preserve the neutral defaults until that decision and qualification;
do not claim the 200-active N+1 gate passed under the current HPA.
