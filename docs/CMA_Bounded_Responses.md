# Bounded response projection

`CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED` defaults to `false` and accepts only the exact strings `true` or `false`. It is independent of the admission/public-window, title and approval flags. The production composition retains a format-aware reader even when disabled, so already-adopted histories remain readable. Legacy requests perform an additional authorized format check; this flag still gates only the legacy bounded-window optimization. The release owner changes this process setting through an API Pod rollout; this optimization is not the durable segmentation gate.

The first implementation reads a command-bound target turn in one read-only Mongo snapshot, reusing the shared prepared head/tail/extra-tail proof. It requires the captured Agent head, a retained model on the target TurnStarted, consecutive source rows, and either the captured tail or the next TurnStarted witness. It reads at most 200 source rows and 4 MiB BSON. Unbound commands need no turn rows. The existing response projector and replay numbering receive complete target-turn input.

Missing, unsupported, disabled or oversized legacy proof retains the existing complete read-only fallback. Corruption and quarantine are errors, never successful empty output. Public reads do not prepare, audit, quarantine or operate a transaction. Authority remains nonrenewing and is rechecked after dependency reads and before buffered stream frames. Current model settings do not prove an old turn's execution model.

This legacy optimization is the first S2 increment under #1349/#1320. It does not complete segmented response support: indexed long-turn pages, full compatibility-budget behavior and the source/projection contract must be integrated before segmented writers can activate. A segmented implementation must never select the legacy lifetime fallback. Existing final Responses frames embed full output; oversized results require the documented pagination-required behavior and complete history access, not silent truncation.

Verification compares complete versus bounded response objects and every deterministic replay event, including model changes, output gaps, unbound commands and failure outcomes. Mongo fixtures measure total checkouts and consumed rows with the shared proof included at 1,000/10,000 source rows. They exercise row/byte/model fallback, stale captured heads, invalid bindings and quarantine, and assert no write/transaction-control commands. Authority tests cover shared redaction and revocation while a source read waits. The paired disposable Mongo 8.0.13 fixture includes all head/tail/extra-tail proof checkouts. For the same ten-row target turn, the 1,000-row complete baseline consumed 7 checkouts, 1,002 log documents and 4,342,007 BSON bytes; bounded consumed 4 checkouts, 11 documents and 27,804 bytes. At 10,000 retained rows the complete baseline consumed 9 checkouts, 10,002 documents and 43,744,007 bytes; bounded remained 4/11/27,804. These measure the response source, not the authorization/command reads or full HTTP request. Production savings depend on the eligible-turn/fallback mix; they are not inferred from these fixtures.

Rollout remains default-off. Compare response GET/replay and successful POST initial-observation p50/p95, scans and BSON bytes per request, checkout wait, fallback reasons and 5xx under matched route/cohort load. Keep long/legacy-model fallbacks visible. Kill the optimization by setting this flag false on the same compatible image; retain all source/proof/command records. Format rollback for already-segmented Agents follows the separate certified-reader floor, never this flag.

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
