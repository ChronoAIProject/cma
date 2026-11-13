# Bounded CloseProcess and capacity-settlement retries

Part of #1320; follows the #1421 scan attribution. In the later November 10
production window, worker_publication/close_process accounted for 0.6127 complete
scans/s, 13.09% of scan count and 2.1249 MiB/s. The earlier independently counted
CloseProcess retry path accounted for 22.9% of complete scans. These historical
caller shares are not current checkout-wait shares or a measured eligible fleet
fraction. The already-implemented duplicate-capacity retry selection remains
separate. Admission is owned by #1591; bounded public windows/titles/approvals
have their existing independent qualification.

`CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED` is a strict boolean, default `false`
in code and the API ConfigMap. It additionally requires the existing shared
explicit-ID or fleet-percentage selection and fresh trusted metadata. It is not
enabled by the fleet switch alone. Settings are process configuration; enabling
or disabling requires an API rollout through the release owner.

The original append shortcut selects an ordinary single `WorkStepFailed` CloseProcess retry, preserving the
Agent status and work step, with no Worktree/event publication and fewer than
10,000 retained events is eligible. It must use normal append capacity. The
existing trusted-summary proof validates the exact Agent incarnation/revision,
semantic state, tail hash and absence of extra rows in the publication's own
transaction snapshot. The ordinary mutation validator and final CAS still fence
identity, Worktree, lease holder/fence/expiry, revision, sequence, stop intent and
receipts. The failure record, retry timing and pending cleanup are unchanged.
No external close/remove effect is introduced or acknowledged by the shortcut.

Missing, stale, unsupported or semantically untrusted metadata retains the full
scan/audit and rebuilds the ordinary checkpoint only as part of the existing
atomic publication. This path adds no lazy preparation or stored format. A
compatible full-path writer can interleave; stale proof never grants trust.
Corruption or quarantine retains the shared refusal/durable quarantine policy.
Like admission, the bounded path relies on immutable middle history and explicit
audits to revoke a disproved checkpoint; it does not rescan every middle row.

#1788 extends this same switch and cohort to zero-entry retries of any step,
including existing capacity/ledger markers and ordinary repeated failures selected
by `CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED`. With single-marker selection
enabled, all failure appends (including first and class-changed CloseProcess
failures) fully verify their prefix; the older ordinary CloseProcess shortcut
remains available when that switch is off. The first repeated retry in each
store process and the first retry after one hour since its last confirmed audit
fully scan. Between audits, fresh trusted prefix proof plus an exact tail point
read replaces the scan. Missing/stale/untrusted proof falls back; first markers,
capacity state changes, successful effects and step advances retain full audits.
The invocation-local conflict cache requires byte-identical integrity/quarantine
BSON and an identical Agent; it never moves the parent Sandbox fence earlier.
See [the integrity contract](CMA_Admission_Append.md) for the facts proved and the
remaining immutable-prefix assumption. Unknown commits continue to use the exact
full snapshot receipt proof and never infer success from the summary or reappend
to resolve uncertainty. Byte and document
capacity checks use the same trusted byte totals and encoded append buffers.

## Verification and rollout

Paired disposable-Mongo tests compare all CMAP checkouts, complete scan counts,
consumed log documents/bytes and the exact resulting Agent/integrity document at
1,000 and 9,990 events. Other contracts cover disabled/untrusted fallbacks,
corrupt tail/identity/quarantine, preserved mutation fences, full fallback gaps,
capacity/metadata-only and successful-close exclusions, restart, independent
store races and mixed writers, transient retries and actual ambiguous commits.
All changed Rust files remain below 500 lines.

Paired local Mongo 8.0.13 results (4-KiB message fixtures; one retry publication):

| Retained entries | Checkouts, off → on | Complete scans | Consumed log documents | Consumed BSON bytes |
| --- | --- | --- | --- | --- |
| 1,000 | 10 → 7 | 1 → 0 | 1,001 → 1 | 4,380,944 → 329 |
| 9,990 | 12 → 7 | 1 → 0 | 9,991 → 1 | 43,981,894 → 329 |

Both runs produce the exact same durable Agent and integrity metadata. These
are total client checkouts and consumed log BSON, not network bytes, production
latency or a fleet saving. The full path consumes its proof tail plus the
complete history; the shortcut consumes only that tail.

At 500 retained entries, disabled/cohort-off reads use 10 checkouts. Missing or
unsupported metadata uses 8; stale metadata uses 10; semantically untrusted
metadata uses 12. Each retains one complete scan. The last case repeats bounded
proof reads before the full fallback, so fallback coverage matters when deciding
whether activation helps. A successful ordinary full append rebuilds its
checkpoint; legacy semantically unsupported history stays untrusted.

Deploy default-off through the owner-authorized release process. Production
execution and activation are release-owned. Do not alter the shared
admission/public-window fleet selection without explicit activation authority.

After separately authorized activation on a fixed shared fleet/pool/worker
configuration, compare matched natural failed CloseProcess publications:
caller/step scans/s and bytes, completed retry/work counts, publication timing,
checkout counts/waits, route latency/errors, CPU and healthy recovery. Preserve
capacity_settlement/close_process separately; #1788 bounds eligible repeated
marker scans to the complete-audit frequency. Global avoided-write counters
include all trusted writers, so use caller/step attribution and actual sample
coverage; do not claim a per-retry or CPU saving from fleet totals. No synthetic
failure or production fault injection.
Publication timing logs only emit at 100 ms or above, or after multiple attempts;
their count is not the total publication denominator. Missing complete exposure
must be reported as partial evidence.

Hold expansion for missing eligible exposure, untrusted fallback growth or
unmatched windows. Roll back on altered pending work/receipt/stop semantics,
corruption regression, failed recovery or existing checkout/latency/health
triggers. Disable only this independent flag on the same compatible image,
prove all enabled processes are gone and reverify. Retain history, quarantine,
proof metadata and the shared cohort. Full #1320/#1349 acceptance remains open.


## Capacity-settlement activation evidence (#1905)

The existing bounded path includes zero-entry capacity-marker settlement at
10,001 rows; it does not require the ordinary single-marker failure switch.
Both the close-retry switch and shared cohort selection must be active in the
same process. A new image alone does not establish either prerequisite.
First-marker publication still scans fully. The first subsequent metadata-only
retry audits fully, then subsequent retries use the persisted byte/entry totals,
semantic checkpoint, hashed tail and exact retained capacity marker. First use
after restart/eviction and the first retry after an hour still audit fully.
The audit cache grants no mutation authority and does not replace the fresh
transaction snapshot, lease/claim validation, parent fence or final Agent CAS.

`cma_agent_capacity_settlement_reads_total{worker_step,decision}` counts selected
verification paths per transaction attempt, including attempts that later fail
or abort. It is neither completed cleanup nor a count of published retries.
Labels contain no Agent IDs, request IDs or failure contents:

| Decision | Meaning |
| --- | --- |
| `flag_disabled` | A zero-entry retry used full verification because the close-retry flag was off. |
| `cohort_excluded` | The flag was on, but this Agent was outside the shared cohort. |
| `full_required` | The operation requires full validation, including first capacity markers and capacity state changes. |
| `audit_due` | The first/restart/expired repeated-retry audit selected a complete scan. |
| `proof` | A fresh trusted summary and retained tail were read; mutation validation still follows. |
| `proof_fallback` | Missing, stale, unsupported or untrusted metadata selected a full scan. See `cma_agent_retry_audit_fallbacks_total{worker_step,reason}`. |
| `proof_error` | Proof validation/read failed; the existing error/quarantine behavior is unchanged. |
| `conflict_reuse` | An identical aborted snapshot reused this invocation's history, without another scan. |

Use the existing `cma_agent_log_scans_total`,
`cma_agent_log_scan_documents_total` and `cma_agent_log_scan_bytes_total` with
`caller="capacity_settlement"` and the same worker step for actual scan volume.
Existing retry audit counters distinguish completed audits from selected paths.
Segmented publications use their separate bounded path and are outside this
legacy-history selection counter.

Analyst-03's production baseline is generation 186, image
`sha256:417490fda8491701f9edc00b1534c1a2c1fb189640e31bdd691915ce1051466f`,
source `52a06780b` as attested by the lead. Direct private metric snapshots cover
2026-11-13 12:23:38.838390–12:24:50.304102 UTC (71.465712 seconds), on the same
Pod/image. These are end-minus-start counters, not summed emissions:

| Capacity step | Complete scans | Documents | BSON bytes | Scans/min | MiB/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| CloseProcess | 49 | 490,049 | 247,390,066 | 41.139 | 3.301 |
| EnsureProcessClosed | 15 | 150,015 | 70,268,405 | 12.593 | 0.938 |
| RemoveHome | 9 | 90,009 | 38,920,905 | 7.556 | 0.519 |

All three steps scanned exactly 10,001 rows per scan. Combined consumption was
4.758 MiB/s. All 73 execution-section samples had longest uninterrupted wall
sections above one second; concurrent wall durations are not CPU shares, and
counter/histogram flush boundaries can differ. No per-step Mongo command count
was available. A published retry does not establish successful terminal cleanup.

A later read-only deployment check found an explicit fleet percentage of zero
and an explicit empty cohort, with the close-retry flag absent from visible
configuration. An unread envFrom Secret prevents inferring its effective value
from absence alone. This check describes visible configuration, not runtime
activation or performance improvement. Owner activation and a fresh same-image,
same-Pod production window remain necessary before claiming savings or improved
end-to-end request latency.


A second analyst window after batch 2, generation 187/image
`sha256:793f31a7d19cce87ac02ffb88e51ab948961204e1b2e55a48632fcf204fdf49b`,
retained this traffic. Its paired metric duration was 621.668392 seconds, ending
2026-11-13 13:10:03.019540 UTC. The lead-attested source is
`0d9e5ba7db24d59a2fc034d9015ccd32cf9a36f5`; it was not derived from the binary.
CloseProcess consumed 2,393,613,620 bytes in 476 scans (3.671938 MiB/s),
EnsureProcessClosed 384,337,761 bytes in 83 scans (0.589596 MiB/s), and RemoveHome
95,139,990 bytes in 22 scans (0.145950 MiB/s). Every scan still consumed 10,001
rows. Capacity settlement accounted for 4.407484 MiB/s and 78.02% of scan bytes
among series present in both scrapes. New series without a baseline are excluded
from that share. This is residual work after batch 2, not an activation result
for bounded settlement or a CPU-share attribution.

The [machine-readable local receipt](evidence/capacity-settlement-20261113.json)
records the paired Mongo 8.0.13 measurements. Capacity-marker fixtures contain exactly 10,001 retained
rows, confirmed by a separate collection count. One first full audit remains;
subsequent zero-entry retries produce identical Agent and integrity documents:

| Worker step | Full → bounded consumed documents | Full → bounded consumed BSON bytes | Full → bounded complete scans |
| --- | ---: | ---: | ---: |
| CloseProcess | 10,002 → 2 | 4,229,015 → 1,400 | 1 → 0 |
| EnsureProcessClosed | 10,002 → 2 | 4,308,077 → 1,416 | 1 → 0 |
| RemoveHome | 10,002 → 2 | 4,209,027 → 1,396 | 1 → 0 |
| RemoveWorktree | 10,002 → 2 | 4,248,559 → 1,404 | 1 → 0 |

The extra full-path consumed document is its proof-tail read. The bounded path
reads that tail for integrity and again for the exact capacity marker; the
beyond-head existence query returns no document. Each case passes with and
without an injected update conflict: an unchanged retried snapshot consumes no
additional log rows. These bytes describe synthetic retained failure rows, not
production payload sizes or network traffic. The tests also preserve full
verification after restart/expiry and missing/stale/untrusted proof, reject
damaged prefix/tail/truncated history and stale authority, and retain complete
receipt reconciliation after an actual ambiguous commit.
