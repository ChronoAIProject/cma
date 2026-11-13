# Routed binding queue deadlines

The routed existing-Agent path has **no gateway timer that measures the entire
accepted CMA queue wait plus Agent run and fails it at 300 or 600 seconds**.
Short HTTP and worker-invocation budgets cause read-only recovery of the same
accepted response. The hard retained-execution boundary is seven days, which
covers the maximum 86,400-second CMA queue wait plus a 30-minute run.

This conclusion assumes a provider acceptance was durably observed and that
storage, credentials and channel authority remain usable. Network failure,
revocation, corrupt records, unknown delivery effects and retention exhaustion
remain explicit failure/reconciliation cases; a queue policy cannot remove them.

## Execution boundary

Gateway admission commits an immutable plan and wakes the scheduler. CMA command
admission is a subsequent provider POST. The gateway considers it accepted only
after observing the response ID and `response.created` with `status: queued`.
It then drops that submission stream and uses GET snapshot/replay requests. Thus
waiting in the **CMA lane** does not keep the submission request open.

Production installs `HttpExecutionPort` and `RoutedTargetExecutor`, passing only
`DeliveryConfig` to the executor. Only the shutdown drain is copied from
`TurnConfig` into the routed scheduler. The legacy `TurnRunner` is a separate
path. See [production composition](../api/crates/cmaeg/src/serve_routed.rs#L135),
[submission](../api/crates/cmaeg/src/providers/frozen.rs#L19),
[accepted-state dispatch](../api/crates/cmaeg/src/turns/routed/dispatcher.rs#L267)
and [GET recovery](../api/crates/cmaeg/src/turns/routed/adapter.rs#L105).

Durable Custom routing and provider replay must be enabled; this evidence does
not authorize legacy fallback. GitHub business dispatch now uses this ordinary routed path with
`durable_dispatch: true`; this audit still does not claim a live GitHub end-to-end
run.

## Timer inventory

Values below are code/deployment defaults. Configured values must still be read
from the live deployment before online acceptance.

| Mechanism and source | Bound | Effect on a multi-hour CMA queue wait and long run |
| --- | --- | --- |
| Ingress body/receipt handling: [Custom](../api/crates/cmaeg/src/http/events.rs#L261), [GitHub](../api/crates/cmaeg/src/github/http.rs#L299) | 5 s per incoming operation | Applies before durable acceptance, not over the Agent run. A caller may need an exact retry/readback after an uncertain receipt response; it does not authorize another accepted CMA submission. |
| Incoming Custom signature freshness: [HTTP contract](../api/crates/cmaeg/src/http/events.rs#L70), [verification](../api/crates/cmaeg/src/auth/webhook.rs) | Signed timestamp within 300 s | Checked only on ingress. The persisted accepted response is not reauthenticated against the old webhook timestamp during queue recovery. A later exact ingress retry needs a fresh valid signature; it restores the same admission. |
| GitHub receipt recovery lease: [receiver](../api/crates/cmaeg/src/github/receiver.rs) | 30 s lease, 5 s tick, 64 records per owner page, 16 attempts per tick | Recovery is owner-scoped through the `(owner, kind, _id)` index; expired leases are reclaimable and a fenced worker cannot overwrite a newer update. |
| Admission/reference eligibility: [candidate](../api/crates/cmaeg/src/ingress/routing_store/candidate.rs#L315), [dispatcher](../api/crates/cmaeg/src/turns/routed/dispatcher.rs#L267), [pending expiry](../api/crates/cmaeg/src/turns/routed/expiry.rs#L16) | Event references are pinned to the seven-day admission retention for GitHub pending dispatch; ordinary sources retain their existing 30-minute reference. | Can refuse gateway work which has not yet reached CMA acceptance. `Accepted` dispatch takes the GET-replay branch before the event-expiry check. It does not expire a command already queued in CMA. A lost/ambiguous POST is not treated as definitely unaccepted. |
| Provider connection: [configuration](../api/crates/cmaeg/src/config.rs#L259), [connect](../api/crates/cmaeg/src/providers/transport/connect.rs#L29) | 2 s per new connection | Connection failure is an operation error, not an elapsed-turn cap. Accepted-response GETs can reconnect later. |
| Provider concurrency acquisition: [constant](../api/crates/cmaeg/src/providers/client_types.rs#L135), [acquire](../api/crates/cmaeg/src/providers/client.rs#L401) | 50 ms | Before POST, failure proves no request was submitted and permits a bounded retry. During recovery it delays a GET; it does not resubmit or fail an accepted command. |
| Provider connection pooling: [HTTP pool](../api/crates/cmaeg/src/providers/client.rs#L52), [socket pool](../api/crates/cmaeg/src/providers/transport/mod.rs#L99), [body drain](../api/crates/cmaeg/src/providers/transport/body.rs#L73) | 90 s idle; 100 ms optional body drain | Discards/replaces idle transport connections. Neither the response ID nor the execution journal expires with a socket. |
| Ordinary turn/conversation preparation: [production builder](../api/crates/cmaeg/src/turns/routed/builder.rs#L85), [conversation PUT](../api/crates/cmaeg/src/providers/conversation.rs#L46), [media preparation](../api/crates/cmaeg/src/media/prepare.rs#L43) | Whole preparation 20 s, capped by remaining event/plan lifetime; provider PUT default 30 s is inside that bound; attachment preparation default 15 s and capped at 20 s | Existing-Agent targets do execute ordinary preparation before POST. Timeout never submits a turn; it can defer preparation or reach the pre-start reference expiry. It does not measure an accepted CMA queue wait. Attachment work is absent from this text/event case. |
| Replacement hierarchy preparation: [dispatcher](../api/crates/cmaeg/src/turns/routed/dispatcher.rs#L194) | 20 s per inspect/reserve step, with original event-expiry checks | Existing-Agent targets skip this path. It creates no additional deadline after their CMA command is accepted. |
| Frozen provider submission: [submit_frozen](../api/crates/cmaeg/src/providers/frozen.rs#L45) | `min(provider_request_timeout, 20 s)` | Ends at the queued acceptance frame. A timeout after entering transport is **Unknown**, not permission to repost. Slow acknowledgement can require reconciliation, independently of how long the accepted CMA command waits. |
| Snapshot/replay HTTP observation: [snapshot/replay](../api/crates/cmaeg/src/providers/recovery.rs#L63), [stream forwarding](../api/crates/cmaeg/src/providers/recovery/stream.rs#L18) | `min(provider_request_timeout, 60 s)`; deployed default 30 s | Each GET/stream has a fresh budget. Idle stream expiry, EOF or observation errors retain the response and cursor; recovery uses GET only. They do not mark the CMA response failed. |
| Executor read/preparation observation: [executor](../api/crates/cmaeg/src/turns/routed/executor.rs#L98) | 15 s for projection load or snapshot-plus-reattach | A timeout retries observation. This can lose a local attempt or lease during an outage, not the accepted response identity. |
| Executor invocation and reconnect budget: [constants](../api/crates/cmaeg/src/turns/routed/executor.rs#L12), [replay loop](../api/crates/cmaeg/src/turns/routed/executor.rs#L128) | 60 s, 3 reconnects, 4,096 frames per invocation | Exhaustion returns `Retry(5 s)` and releases local ownership. Each scheduled invocation starts a fresh budget; there is no total-attempt or total-runtime counter. |
| Execution lease: [journal](../api/crates/cmaeg/src/turns/routed/journal.rs#L137), [lease constant](../api/crates/cmaeg/src/turns/routed/model.rs#L18), [idle renewal](../api/crates/cmaeg/src/turns/routed/executor.rs#L167) | 30 s ownership, renewed every 10 s while idle and around reads/frames | A blocked/stopped worker can lose its lease. Fencing prevents that worker from committing; a new worker reclaims the accepted journal and GETs the same response. Lease expiry does not fail or repost the CMA command. |
| Scheduler retry, polling and cooldown: [configuration](../api/crates/cmaeg/src/turns/routed/scheduler.rs#L68), [cooldown/scan](../api/crates/cmaeg/src/turns/routed/scheduler.rs#L354) | Retry 5 s, clamped to 250 ms–60 s; scan pages 250 ms and sweeps 5 s; settled cooldown 60 s | These schedule bounded revisits, not lifetime limits. Bounded in-memory queues are hints; a durable manifest scan rediscovers work after restart or backpressure. No retry count cap discards a queued accepted response. |
| Admission/manifest retention: [retention](../api/crates/cmaeg/src/ingress/routing_store/types.rs#L15), [pinned deadline](../api/crates/cmaeg/src/ingress/routing_store/candidate.rs#L236), [retained load](../api/crates/cmaeg/src/ingress/routing_store/store.rs#L166), [recovery](../api/crates/cmaeg/src/ingress/routing_store/recovery.rs#L37) | 7 days from admission, not extended by retry | This is a real recovery limit. A maximum-day queue plus 30-minute run fits. At seven days, discovery/read authority expires and eventual completion is no longer promised; there is no automatic resubmission. |
| Execution body, projection, reply index and delivery receipts: [body](../api/crates/cmaeg/src/turns/routed/body.rs#L25), [projection](../api/crates/cmaeg/src/turns/routed/projection_store.rs#L75), [reply index](../api/crates/cmaeg/src/turns/routed/tool_reply_index.rs#L32), [receipts](../api/crates/cmaeg/src/turns/routed/delivery_receipts.rs#L61) | Same pinned plan retention | No shorter 300-second response-tracking TTL. The response ID and cursor live in the durable `Accepted`/terminal state, not a process-local tracker. |
| Record and credential-registration IO: [record client](../api/crates/cmaeg/src/records/client.rs#L99), [record writes](../api/crates/cmaeg/src/records/client_write.rs#L61), [Mongo configuration](../api/crates/cmaeg/src/serve_storage.rs#L49), [worker history](../api/crates/cmaeg/src/worker_credentials/mongo.rs#L186) | Record HTTP connect 2 s, GET/readiness 1.5 s, PUT/delete/list 5 s, version proof 2 s; worker-history operations up to 4 s; native Mongo uses its URI/driver settings | These are operation/availability policies, not a total execution clock. Native Mongo is configured separately from the HTTP record adapter. Transient unavailable/uncertain storage or lost lease defers recovery. They do not accumulate across queue time. Permanent outages can exhaust the seven-day recovery horizon. Worker-key positive history is permanent, not a credential/response TTL. |
| Channel cache/current restore: [defaults](../api/crates/cmaeg/src/config/cache.rs#L20), [current lookup](../api/crates/cmaeg/src/channels/resolver.rs#L173), [authority mapping](../api/crates/cmaeg/src/turns/routed/dispatcher_authority.rs#L70) | Positive cache 60 s, negative cache 10 s; each current restore 800 ms; cold cached restores 10/minute with burst 3 | Routed authority bypasses the cache and cold-restore limiter but still uses the 800-ms current-restore bound. Timeout maps to unavailable/retry, without POST or command failure. Cache lifetime is not turn lifetime. Actual credential revocation or channel changes can refuse work independently of age. |
| Reply reference and reply bookkeeping: [settlement validation](../api/crates/cmaeg/src/turns/routed/model.rs#L109), [tracker](../api/crates/cmaeg/src/turns/tracker.rs#L132), [effect gate](../api/crates/cmaeg/src/turns/routed/delivery_gate.rs#L13) | Original reference expiry; tool-reply suppression at most 30 min; send window 20 s | Already-accepted Custom/GitHub completion remains recordable. Expired replies/webhooks are rejected rather than renewed; live tool calls still expire. A timed-out external send can be Unknown and require reconciliation, not a new provider submission. The automation channels need no outbound reply sink. |
| Original Custom sync waiter: [stream](../api/crates/cmaeg/src/delivery/sync.rs#L193), [drop](../api/crates/cmaeg/src/delivery/sync.rs#L53), [keepalive](../api/crates/cmaeg/src/http/events.rs#L172) | Ordinary waiter has no execution deadline; keepalive 15 s | It is ephemeral transport, not work ownership. Disconnect/removal does not cancel the admitted plan or CMA turn. Optional media waiters have a separately bounded pending response; that ends only the stream. After reference expiry, only a content-free terminal control may reach the original waiter. |
| Gateway ingress connection and shutdown: [backend policy](../infra/cmaeg/gcpbackendpolicy.yaml#L14), [scheduler drain](../api/crates/cmaeg/src/turns/routed/scheduler.rs#L281), [pod lifecycle](../infra/cmaeg/deployment.yaml#L59) | HTTP/LB 660 s; connection drain 660 s; local scheduler drain default 660 s; pod grace 735 s with 30 s pre-stop | Can close a long-lived observer connection or stop a gateway process. It does not cancel an accepted CMA command. Durable records and expired execution leases allow replacement-process GET recovery. |
| Activity feed: [model](../api/crates/cmaeg/src/activity/model.rs#L5), [buckets](../api/crates/cmaeg/src/activity/store.rs#L388), [observer](../api/crates/cmaeg/src/activity/observer.rs#L30) | 7-day buckets; latest 50 items; observer IO 3–5 s | Display/history limits, not execution timers. A busy channel can evict an item from the latest-50 view earlier. The execution journal remains authoritative; activity loss/expiry does not authorize a POST or convert the command to failed. |
| Activity SSE observer: [stream](../api/crates/cmaeg/src/http/activity.rs#L104) | Connection 300 s; refresh 1 s; read/auth attempt up to 5 s | Closes only that authenticated activity observer, which reconnects with its cursor. It neither owns nor cancels provider execution and cannot turn a queued response into failure. |
| Legacy total-turn cap, **not this path**: [legacy runner](../api/crates/cmaeg/src/turns/runner.rs#L111), [defaults](../api/crates/cmaeg/src/config/turns.rs#L17), [routed wiring](../api/crates/cmaeg/src/serve_routed.rs#L147) | Acceptance 20 s, total turn 600 s, busy retry 180 s | A legacy turn can hit this cap. Routed execution does not use it. No setting needs to be raised to hours, and enabling a legacy fallback would invalidate this proof. |

Media download/transform/transcription and platform-interaction timers are not
on this text/event automation path. The same is true of owner-management request
authentication caches: the already-admitted worker uses current binding/channel
credentials, not the owner's original HTTP session. Credentials must remain valid;
expiration/revocation is an authority change, not an elapsed-turn allowance.

## Clock-advanced evidence

Provider SSE frames remain bounded at 1 MiB by the decoder. Routed delivery
hashes the full decoded JSON for replay identity and acknowledgement; only the
digest is persisted. The 256 KiB serialized delivery-effect limit, 256 KiB
automatic text limit and 1 MiB projection storage limit are separate and
unchanged. A terminal snapshot exceeding 256 KiB does not by itself prevent
settlement, even with empty sinks or an expired Custom EventRef.

An execution retained in `Accepted` by the former raw-frame guard needs no
resubmission or record migration. After deployment, the manifest scan finds it
again, a fenced worker claims it, and recovery GETs the original response after
its last acknowledged cursor. If the terminal projection was already saved,
the replay must match its recorded hash and reuses its bounded pending effect.
Delivery then acknowledges the terminal cursor and completes the journal and
activity. Original reply expiry and current authority checks still apply, as
does the seven-day admission retention horizon; expired retention does not
authorize another provider POST.

The [oversized-frame regressions](../api/crates/cmaeg/src/turns/routed/oversized_frame_tests.rs)
exercise the real executor, delivery engine, authority and frozen-turn builder
against provider and record HTTP fixtures. They cover empty sinks, expired
Custom references, recovery of a pending terminal projection, changed replay
body rejection, idempotent completion, and the unchanged SSE decoder limit.

[`routed_existing_agent_queued_nearly_a_day_then_runs_thirty_minutes_completes_once`](../api/crates/cmaeg/src/turns/routed/long_queue_tests.rs)
uses the real routed planner, encrypted admission/journal records over HTTP,
`HttpExecutionPort`, provider HTTP snapshot/replay, and `RoutedTargetExecutor`.
The provider fixture models CMA's durable queue; the production CMA admission and
queue-deadline behavior has separate [public-interface coverage](../api/crates/layer1-public/src/responses/v2/existing_binding_tests.rs#L12).
Preparation and current-authority fixtures supply the frozen request and channel;
ordinary production preparation is separately bounded as listed above. Replay GETs
end with EOF between invocations, and the test advances time between those
invocations. It does not claim a continuous day-long HTTP stream.

The test selects an existing-Agent target and an 86,400-second binding policy.
It advances both the journal/store clock and Tokio timers across 301, 601, 1,801,
7,200 and 86,340 seconds while provider snapshots remain queued. The executor
retains the same response and cursor, yields retry, and the manifest remains
recoverable. A fresh executor then sees the same response running for 30 minutes
before completion, beyond even the original queue deadline. Terminal replay
performs no further provider IO; the completed checkpoint stays unchanged and
exactly one provider POST occurred.

A temporary negative control inserted an aggregate 300-second expiry into the
executor. The test then failed at the first checkpoint beyond that bound. The
control was removed; no production executor change is part of this audit.

The executor's production invocation/lease/reconnect bounds are not enlarged.
The HTTP client uses the deployed 30-second request default. Existing
[`idle_stream_renews_lease_and_bounded_invocation_releases_ownership`](../api/crates/cmaeg/src/turns/routed/executor_tests.rs)
separately proves active idle-stream lease renewal and invocation release, and
provider-recovery tests prove bounded empty/idle streams return observation
errors without advancing cursors.

This is deterministic protocol evidence, not a measured live chrono-goals run.
G3 still requires the real runtime's sync, check, build, status PR, exact-head CI
wait and merge timestamps.


## Expiry before an original Responses admission

An expired `Pending` or `Prepared` target with no response projection or delivery
checkpoint settles as `Refused` with `event_expired_before_start`. `Prepared`
means that the original Responses POST has not started; it can still contain
continuation preparation and an indexed media work head. `Starting` and `Unknown`
remain uncertain and keep their original recovery semantics.

Prepared expiry makes no provider POST. Media effects independently check the
original expiry and refused parent before fetching, uploading, appending or
sending. The media worker refuses outstanding indexed obligations and removes
the head only after terminal evidence is durable. Before cleanup the expired
work is inert; after cleanup a late derivative cannot recreate a head. Retained
provider-side preparation receipts grant no supplement or delivery authority.

The precise `429 conversation_not_ready` and `503
verified_sender_context_unavailable` paths retain the same frozen body and key
and use the existing five-second cooldown. A separate sealed receipt retains the
last precise non-admission code without changing the strict target-state schema.
On expiry, the public Activity API reports `state:failed`,
`refusal_code:event_expired_before_start`, and `last_non_admission_code` when
known. A restart repairs these fields from the terminal target and receipt.
There is no automatic Feishu failure reply.

Clock-advanced tests cover 429 recovery, prepared media with a retained upload
and an in-flight transcript, restart and active-head cleanup, and public Activity
reporting after repeated 503 responses. They prove no original POST, upload,
supplement or late reply after expiry.
