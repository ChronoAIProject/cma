# Bounded response projection

`CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED` defaults to `false` and accepts only the exact strings `true` or `false`. It is independent of the admission/public-window, title and approval flags. The production composition retains a format-aware reader even when disabled, so already-adopted histories remain readable. Legacy requests perform an additional authorized format check; this flag still gates only the legacy bounded-window optimization. The release owner changes this process setting through an API Pod rollout; this optimization is not the durable segmentation gate.

The implementation reads a command-bound target turn in one read-only Mongo snapshot, reusing the shared prepared head/tail/extra-tail proof. It requires the captured Agent head, a retained model on the target TurnStarted, consecutive source rows, and either the captured tail or the next TurnStarted witness. It reads at most 200 source rows and 4 MiB BSON, including the witness. Unbound commands need no turn rows. The existing response projector and replay numbering receive complete target-turn input.

A coordinate-only lookup locates the first next turn within the same 200-row interval before reading payloads. It uses the existing `agent_log_sequence` index with explicit bounds and examines at most 199 candidate documents; no index is added. The shared interval reader validates the complete source through that witness in the same snapshot. Later-turn payloads cannot consume the target's byte budget. This adds one checkout for bound turns with a nonempty candidate interval, including a final turn without a witness. Consumed BSON telemetry includes the returned coordinate but does not measure the lookup's server-side document examination; disposable Mongo explain checks measure that separately.

Missing, unsupported, disabled or oversized legacy proof retains the existing complete read-only fallback. Corruption and quarantine are errors, never successful empty output. Public reads do not prepare, audit, quarantine or operate a transaction. Authority remains nonrenewing and is rechecked after dependency reads and before buffered stream frames. Current model settings do not prove an old turn's execution model.

This legacy optimization is the first S2 increment under #1349/#1320. It does not complete segmented response support: indexed long-turn pages, full compatibility-budget behavior and the source/projection contract must be integrated before segmented writers can activate. A segmented implementation must never select the legacy lifetime fallback. Existing final Responses frames embed full output; oversized results require the documented pagination-required behavior and complete history access, not silent truncation.

Verification compares complete versus bounded response objects and every deterministic replay event, including model changes, output gaps, unbound commands and failure outcomes. Mongo fixtures measure total checkouts and consumed rows with the shared proof included at 1,000/10,000 source rows. They exercise row/byte/model fallback, stale captured heads, invalid bindings and quarantine, and assert no write/transaction-control commands. Authority tests cover shared redaction and revocation while a source read waits. The paired disposable Mongo 8.0.13 fixture includes all head/tail/extra-tail proof checkouts. For the same ten-row final turn, the 1,000-row complete baseline consumed 7 checkouts, 1,002 log documents and 4,342,007 BSON bytes; bounded consumed 5 checkouts, 11 documents and 27,804 bytes. At 10,000 retained rows the complete baseline consumed 9 checkouts, 10,002 documents and 43,744,007 bytes; bounded remained 5/11/27,804. For a historical target with seven rows plus its next-turn witness, followed by an unrelated 4 MiB output, both lifetime sizes consumed exactly 5 checkouts, 10 log documents (including the coordinate and tail proof) and 15,003 BSON bytes. Mongo execution statistics showed seven documents examined by the boundary lookup in both cases. The row-limit fixture examined exactly 199 candidates whether the witness was at the last allowed row or one row beyond it; the latter selected complete fallback. Exact 4 MiB target input succeeded and one byte more selected fallback. These measure the response source, not the authorization/command reads or full HTTP request. Production savings depend on the eligible-turn/fallback mix; they are not inferred from these fixtures.

Rollout remains default-off. Compare response GET/replay and successful POST initial-observation p50/p95, scans and BSON bytes per request, checkout wait, fallback reasons and 5xx under matched route/cohort load. Keep long/legacy-model fallbacks visible. Kill the optimization by setting this flag false on the same compatible image; retain all source/proof/command records. Format rollback for already-segmented Agents follows the separate certified-reader floor, never this flag.

## Response activation packet for owner review

Status: **NOT APPROVED; activation held**. Related implementation #1885 supports
#1674; production acceptance remains open. The lead takes this packet to the owner.
Deployment engineers use the reviewed release controller only after approval.
Record the exact merged source SHA, immutable image digest, deployed configuration,
process generations, approval ID, private Agent allowlist and baseline window
before execution. Unknown values hold activation. Merge is not approval.

| Separate approval unit | Change and initial scope | Rollback |
| --- | --- | --- |
| Response source switch | Set `CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED=true` on the qualified image. First use only the owner's explicitly selected test Agents in the shared admission selector with fleet percentage `0`; prepare trusted proof only through separately approved admission operations. | Set this flag `false` and roll the same compatible image. Retain all source, command and proof records; no data reversal is needed. |
| Shared cohort change or expansion | Each change to `CHRONO_AGENT_LOG_ADMISSION_AGENTS` or its fleet percentage changes admission/public-window exposure as well as response eligibility. Submit its exact configuration separately under the [Agent-log activation packet](CMA_Agent_Log_Activation.md). The response switch has no independent percentage/owner selector. | Restore the approved selector via the release controller; preserve accepted writes and reader compatibility. |
| Paged observation switch | `CHRONO_RESPONSE_PAGED_EVENTS_ENABLED=true` independently changes legacy event observation timing and quiet-poll projection. Its process scope is not restricted by the shared admission selector. An owner-test-only canary requires verified traffic isolation before approval; otherwise hold this switch and submit its actual exposure. | Set this flag `false` on the compatible image; adopted histories retain their indexed reader. |

Keep the paged observation switch at its existing approved setting while evaluating
the response source change. Other default-off switches, Mongo indexes, recovery
operations and held-import release each retain their separate owner approval.
The response change adds no index, stored field, migration or data-changing read.

These stages also inherit the [complete-operation acceptance limits and evidence
requirements](CMA_Agent_Log_Activation.md#measurements-and-expansion-criteria).
Measure the full client tab-open/send/interrupt flow when claiming improvement to
those targets; a response endpoint span or admission ACK alone cannot qualify it.
Retain all limit exceedances and first-use/error/cancellation samples. New-Agent
readiness and the admin Sandbox report remain separately measured controls or
explicitly unverified; source-read savings cannot establish either target.

Capture a matched off baseline and at least 30 minutes per approved canary stage.
Retain nonzero successful response GET, replay and initial POST observation counts
separately, source bounded/fallback/failure deltas, consumed BSON/rows, Mongo query
and checkout counts, checkout-wait p95, route p50/p95, CPU/memory and 5xx. Include
small historical targets followed by large later turns, model fallback, long-turn
fallback, queued/terminal outcomes and replay after reconnect. Compare exact
response objects, durable event IDs and per-frame revocation behavior using only
approved test activity. Retain first failures and process counter resets. Require
at least 100 successful matched samples per claimed route before claiming p95
improvement; an unobserved route remains pending.

Abort on any output/model/replay mismatch, authority leak, unexpected mutation,
quarantine/corruption increase or candidate-attributed readiness/503 failure.
Also abort after two consecutive five-minute windows with route or checkout-wait
p95 more than 10% above the matched baseline. Existing stricter release gates win.
Missing attribution, zero samples or an untested rollback hold expansion. The
rollback approval should be included in each stage's owner decision so the release
controller can close the switch promptly. Send the result, first failures and
rollback status to the lead; each next stage requires a new owner approval.

The existing ten-second read telemetry emitter reports `cma_response_source_reads_total` with fixed outcomes `bounded`, `legacy_untrusted`, `legacy_budget`, `legacy_model`, and `failed`. These are per-process cumulative counters; compare deltas within each process lifetime and retain restarts. Disabled legacy optimization adds no bounded-window source attempt; format dispatch and adopted-history reads are independent of this counter. Failures include cancellation/timeouts; fallback outcomes mean the bounded source requested the unchanged legacy path, not that the later full read succeeded. The `bounded` count measures avoided complete-prefix source reads, not HTTP requests or Mongo commands. Pair it with Mongo consumed-document/byte counters and route samples. No identity, command, native handle or output content is logged.

## Paged response observation

`CHRONO_RESPONSE_PAGED_EVENTS_ENABLED` defaults to `false` and accepts only `true` or `false`. It independently opts response event streams into deferred source observation. The release owner changes this process setting by API Pod rollout. Turning it off restores the previous observation path for legacy records without modifying stored history. Already-adopted records keep indexed observation.

With the flag enabled, admission/current authorization precede HTTP headers, while source projection happens when the stream body is consumed. An unavailable source emits a connection `error` without a durable SSE id; the admitted command is unchanged. Cursor syntax and integer-overflow checks still precede headers. A cursor beyond the durable events emits `invalid_replay_cursor` after headers. Clients must retain their last successful durable id and reconnect with fresh authorization.

Every idle poll rechecks the current command and Agent head under the nonrenewing authorization. An unchanged head and command skip source reads and projection; a queued rejection is still observed even when no Agent event is appended. Every buffered frame receives its own release-time authorization check. This does not cache or renew permission.

## Retained segmented response consumers

Adoption and new segmented writes remain default-off. A reader recognizes an
already-adopted envelope before considering either legacy optimization flag.
Missing or invalid certified output evidence returns an explicit read failure;
it never selects a complete-history fallback or returns successful empty output.
A legacy stream that encounters adoption after opening ends its observation;
the client reconnects with the same last successful durable cursor.

V2 retrieval combines scalar turn facts and requested original source points.
It uses the publication/audit contract for historical model knowledge, original
terminal/prewrite failure references and the latest usage before the next turn,
including usage after terminal. Replay uses immutable output-change counters;
later tool output changes current items without rewriting first-terminal frames.
A resumed event cursor is located with logarithmic ordinal probes followed by
an ordered numeric index seek, not a scan from the beginning of the turn.

Each output/replay page handles one item-change group. The underlying port
accepts up to eight changes and 32 original points, but the public consumer
selects one group because the index does not carry payload-size summaries.
This keeps the selected original payload group within one source request's
4 MiB limit; metadata is independently bounded. Ordinary response objects have
a 4 MiB serialized budget. A large retained lifetime with a small requested
result remains supported; lifetime row/byte thresholds do not decide the result.
Individual items beyond the source/page budget fail explicitly.

For v2, `GET /api/v2/responses/{response_id}/output` returns
`object: cma.response.output_page`, `response_id`, `data`, `has_more`, and
`next_after`. Indexed responses also return their captured `through` coordinate.
Send the returned cursor as `?after=...`; it is scoped to that response and
source format. Indexed cursors identify the next immutable change ordinal;
legacy cursors identify the next item ordinal. A page may contain no new item
when its change only terminates a prior tool; advance its returned cursor.
No caller should infer completion from an empty `data` array alone.

An oversized complete v2 result returns `response_pagination_required`.
`/events` still pages immutable output frames; if the final embedded response
cannot fit, it emits a connection error without a durable ID instead of an
incomplete or truncated successful terminal. Fetch `/output` pages to obtain
every bounded item. Reconnect retains the last successfully delivered event ID.

Legacy retrieval supports
`GET /api/v1/responses/{response_id}?output_page=true&output_after=<cursor>`.
Omit `output_after` on the first request, then use the returned numeric
`next_after`; these are original message ordinals, not source log offsets.
An oversized complete result returns the same pagination-required error.
The legacy wire contract contains message items; v2 output pages additionally
expose tool items. Both paths keep their existing authorization and redaction.

Legacy live streaming preserves every original intermediate tool update using
bounded source pages over the requested turn. It does not allocate a whole-turn
history vector, but it still traverses that turn's original rows: the immutable
output-change index records first-terminal output, not every legacy tool update.
It reads one source row at a time, retains at most 4 MiB of tool state and a
bounded event group, and reports an explicit observation limit rather than
silently dropping output. This path is not claimed to have fixed cost for sparse
long turns; v2 replay and both indexed retrieval paths avoid that traversal.

All observations keep their original authority and deadline. Every buffered
frame is revalidated after backpressure; slow reads, deletion, expiry and share
revocation cannot release a stale private result. Capture/snapshot resources end
with each source observation and are not held across idle polling or clients.
The canonical reader Mongo selector includes the response-reader tests. Gate
results and exact candidate identity are recorded in the integration handoff;
this document alone is not an activation or production-verification claim.
