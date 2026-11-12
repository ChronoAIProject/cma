# Complete Agent-log scan accounting

This extends the read telemetry introduced in #1459/#1468. It replaces the
outcome-only scan label specification in `CMA_Operations.md`; Mongo consumed-read
and public-stream accounting retain their existing meanings. Related to #1582;
part of #1421. No production measurement or CPU improvement is claimed here.

Every invocation of the complete integrity scanner receives a closed `ScanCaller`
value. There is no implicit caller default or inherited task/span attribution.
The counters below keep their existing names and gain `caller` and `worker_step`
labels alongside `outcome`. Summing all caller/step series recovers the previous
aggregate. Consumers must include these labels in series identity before taking
per-series differences; successive samples of different callers are not resets
of one shared series. Mixed versions require a version/process boundary.

| Metric | Meaning |
| --- | --- |
| `cma_agent_log_scans_total` | Complete-snapshot validation attempts. |
| `cma_agent_log_scan_documents_total` | Consumed documents, including an offending document. |
| `cma_agent_log_scan_bytes_total` | Consumed raw BSON lengths, excluding reply envelopes. |
| `cma_agent_log_scan_duration_microseconds_total` | Sum of elapsed wall time inside the complete scanner, in microseconds. |

`outcome` is `success` only after the complete captured history passes traversal
and head validation. `failed` includes domain/driver errors, early physical-limit
refusals and cancellation. A successful scan may be followed by a rejected
publication, including a normal-capacity rejection. Counters measure actual
validation passes: transaction retries and unknown-commit proofs each count
when they reach the boundary. Pre-scan metadata failures do not count as scans.

Duration includes Mongo waiting, decoding, integrity validation and the caller's
visitor. It excludes preliminary metadata reads and work after traversal.
It is not CPU time. Divide duration deltas by scan-count deltas for mean elapsed
scan time; do not convert it into CPU-core use. Sub-microsecond durations round
down. Counts and durations saturate at `u64::MAX`.

| `caller` | Selected boundary |
| --- | --- |
| `worker_publication` | Ordinary fenced Agent work publication, including prepared startup. |
| `capacity_settlement` | Cleanup capacity/ledger retry proposals and capacity-failure settlement; classified before validation, so a rejected proposal retains its category. |
| `runtime_reconciliation` | Child log validation in runtime-loss reconciliation. |
| `recovery_proof` | Unpublished-runtime child history proofs. |
| `inactivity_proof` | Live-child activity evidence for maintenance. |
| `title_discovery` | Title discovery, claim and publication validation. |
| `public_window_prefix` | Shared page/range/prefix readers and pending-approval projections. |
| `conversation_publication` | Conversation mutations and first-message terminal projection. |
| `checkpoint` | Checkpoint capture/fencing and checkpoint conversation mutations. |
| `command_admission` | Command reserve/control/reconciliation, admission/reuse and lifecycle transitions. |
| `unknown_commit_proof` | Exact commit reconciliation for worker, append, conversation, title and created admission receipts. |
| `other` | Generic direct append validation. |

Worker publication and capacity settlement use the step loaded from the
transaction's current Agent, not its proposed next step. The fixed step inventory
is `install_skill`, `wait_for_workspace`, `add_worktree`, `materialize_home`,
`open_conversation`, `publish_ready`, `publish_connection`,
`dispatch_first_message`, `fence_interaction`, `close_process`, `publish_stopped`,
`ensure_process_closed`, `remove_home`, `remove_worktree`, `publish_deleted`,
and `none`. Other callers use `not_applicable`. The closed inventory contains
42 caller/step pairs and two outcomes; it cannot retain arbitrary strings.

Counts/bytes remain local during traversal. Dropping the guard adds one completed
observation to atomic process totals, including partial work on failure or
cancellation. The existing ten-second read-telemetry timer emits cumulative
samples for changed pairs, with `parent: None`; no per-document event, query,
copy or extra decode is added. The existing post-worker-drain shutdown emission
includes late completions and cancelled workers. Abrupt process death can lose
unemitted counters; totals reset on process restart. Atomic fields are sampled
independently, so a concurrent completion may straddle adjacent samples; use
interval deltas, not cross-field equality within a single instantaneous sample.

Complete scans overlap `cma_mongo_read_*` for `agent_log`; never add both families
as separate work. The caller categories partition this complete-scan boundary,
not all database reads or all end-to-end request costs. Shared public log reads
retain their boundary category even when consumed by another service component.
No identity, error text, document content or request-span field is emitted.

### Scan execution sections (#1787)

Each full-scan attempt now emits `cma_agent_log_scan_longest_section_seconds{caller}`
and `cma_agent_log_scan_driver_poll_seconds{caller}` once on drop, including
validation failure and cancellation. `cma_agent_log_scan_yields_total{caller}`
counts explicit cooperative yields. Labels use only the existing closed caller
names; no Agent, Sandbox, request, user, query or document content is retained.
The existing full-scan count, document/byte counts and success/failure accounting
retain their meanings. Measurements accumulate in local variables, with no
per-document atomics or logging; optional debug output is once per attempt.

An execution section closes only when an observed future actually returns
`Pending` or the scan explicitly yields. Time spent waiting to be polled again
is excluded. Initial-find suspension is observed too, so Mongo/network wait
does not become a CPU section. Driver-poll duration measures the final `Ready`
poll of cursor advancement, including driver batch copying; it does not include
I/O wait or the subsequent row decode, visitor and hash. Both histograms include
submillisecond, 2 ms, 5 ms and 10 ms boundaries, supporting the issue's A/B/C
decisions. These are wall-time observations, not CPU profiles or proof that
a particular task occupied a Tokio LIFO slot.

The legacy capture reader also observes its existing find/cursor awaits because
it shares scan accounting. This measurement preserves its existing
64-document batches and scheduling behavior.

The histograms aggregate all scan sizes for a caller. For case A's at-least-1,000-
row subset, the once-per-attempt debug event includes numeric `documents` and
`bytes`, outcome, section count and both maxima. Use those observations to
select the subset; an average documents-per-scan ratio cannot prove it.

These #1787 families add at most 444 series: 12 closed caller names times
(two histograms with 15 finite buckets, +Inf, sum and count, plus one counter).
This is separate from the #1805 bound of `2 * min(workers, 64) + 195`.
