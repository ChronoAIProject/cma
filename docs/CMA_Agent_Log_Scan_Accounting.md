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
| `command_admission` | Command reserve/control, admission/reuse and lifecycle transitions. |
| `command_reconciliation` | Dispatched-command evidence reads, including trusted-prefix validation. |
| `unknown_commit_proof` | Exact commit reconciliation for worker, append, conversation, title and created admission receipts. |
| `other` | Generic direct append validation. |

Worker publication and capacity settlement use the step loaded from the
transaction's current Agent, not its proposed next step. The fixed step inventory
is `install_skill`, `wait_for_workspace`, `add_worktree`, `materialize_home`,
`open_conversation`, `publish_ready`, `publish_connection`,
`dispatch_first_message`, `fence_interaction`, `close_process`, `publish_stopped`,
`ensure_process_closed`, `remove_home`, `remove_worktree`, `publish_deleted`,
and `none`. Other callers use `not_applicable`. The closed inventory contains
43 caller/step pairs and two outcomes; it cannot retain arbitrary strings.

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

For comparisons with images before #1785, sum `command_admission` and
`command_reconciliation` for scans, documents, bytes and duration. Splitting the
label changes attribution only; it does not reduce any reads.
