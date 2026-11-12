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
it shares scan accounting. It retains its existing 64-document batches; the
cooperative switch targets the complete integrity scanner (`scan_rows`).

The histograms aggregate all scan sizes for a caller. For case A's at-least-1,000-
row subset, the once-per-attempt debug event includes numeric `documents` and
`bytes`, outcome, section count and both maxima. Use those observations to
select the subset; an average documents-per-scan ratio cannot prove it.

These #1787 families add at most 444 series: 12 closed caller names times
(two histograms with 15 finite buckets, +Inf, sum and count, plus one counter).
This is separate from the #1805 bound of `2 * min(workers, 64) + 195`.

### Local cooperation comparison (2026-11-13)

The disposable-Mongo `--scan-cooperation` contract runs an eight-worker Tokio
runtime with six concurrent scanning tasks, eight scans per task, and 200 pairs
of sequential point reads. Each scan traverses the same indexed 10,001-row
fixture with 256-byte message content. After warming both strategies, the
measured order is off/on/on/off. The native macOS ARM64 debug build uses the
pinned toolchain and Mongo 8.0.13 on a shared development host. Reproduce with
`bash .github/scripts/mongo-contract-tests.sh --scan-cooperation`.

Every measured round completed exactly 48 scans, 480,048 rows and 262,586,256
raw BSON bytes, plus 200 point-read pairs. Each made 448 find and 48 getMore
commands, for 496 command round trips. Both strategies had zero failed scans.
No query batch limit was set. The comparison retains each round's percentiles;
only means are combined using observation counts.

| Observation | Off (rounds 0, 3) | On (rounds 1, 2) |
| --- | --- | --- |
| Longest execution section p99, ms | 430.68; 435.01 | 4.04; 4.04 |
| Longest driver poll p95, ms | 3.81; 3.73 | 3.81; 3.74 |
| Maximum task poll across both rounds, ms | 435.03 | 4.04 |
| Mean scan latency across both rounds, ms | 864.40 | 485.26 |
| Point-read pair latency p95, ms | 428.02; 427.00 | 4.48; 17.03 |
| Checkout wait p99, ms | 426.85; 427.32 | 1.51; 2.42 |
| Mean checkout wait across both rounds, ms | 16.94 | 0.072 |
| Maximum checkout/check-in event gap, ms | 432.34 | 296.06 |
| Maximum connections in use / configured limit | 7 / 30 | 7 / 30 |
| Explicit yields across both rounds | 0 | 19,010 |

Mean scan latency decreased by 43.86% in this local run. Metadata-only command
tracing counts round trips; CMAP events supply checkout durations and event
spacing without installing a command-event handler that decodes reply bodies.
The raw benchmark log SHA-256 is
`6b877092165870dd4058b0751e9f327777910605e953e5871986bf8726ca38b9`.

This reproduces long uninterrupted scans and improved scheduling with unchanged
work on this fixture. It does not identify production CPU costs, prove Tokio
LIFO-slot residency, or satisfy the production A/B/C decision. Driver-poll p95
remained below 5 ms here, so no optional batch-size setting was added. The switch
stays off until the release owner collects the required production baseline,
checks scan failures and publication retries, and verifies activation/rollback
under the procedure in [Operations](CMA_Operations.md#cooperative-full-scan-activation-and-rollback).

The full-contract repeat after develop integration used the same scan and
benchmark implementation and preserved all of those work and command counts,
but did not reproduce the first run's sub-10 ms section tail. Disabled section
p99 was 469.99 and 443.17 ms; enabled section p99 was 23.84 and 28.76 ms.
Mean scan latency was 806.03 ms off and 512.75 ms on
(-36.39%). Enabled point-read pair p95 was 14.79 and 43.29 ms, versus 430.99 and
427.25 ms disabled; enabled checkout-wait p99 was 3.21 and 2.97 ms, versus
436.93 and 423.64 ms disabled. Driver-poll p95 stayed below 4.76 ms in all four
rounds, although one enabled round contained a 22.84 ms driver poll. The cause
of these wall-time outliers has not been isolated. With only 48 scans per phase,
the reported nearest-rank p99 is also that phase's maximum. Both runs support
keeping the measurements and the default-off option; neither establishes a
reliable 10 ms section bound or substitutes for the required production study.
The full-contract repeat log SHA-256 is
`9638f1a4e0fdc5b54aedfac01478fd8063d12db7fbbc816d753b8996ba4b3570`.
