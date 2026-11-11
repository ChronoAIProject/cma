# Bounded ordinary CloseProcess retries

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

Only an ordinary single `WorkStepFailed` CloseProcess retry, preserving the
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

Capacity markers, metadata-only settlement, at/over-entry-capacity retries,
successful effects and other worker steps keep their complete audit. Unknown
commits continue to use the exact full snapshot receipt proof and never infer
success from the summary or reappend to resolve uncertainty. Byte and document
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

Deploy default-off after review, lead-authorized merge and a fully green final
develop batch artifact. Target Thu 2026-11-12 morning, review-ready by 02:00 SGT
and merge by 06:00. Production execution is release-owned. Do not alter the
ongoing admission/public-window fleet experiment to activate this shortcut.

After separately authorized activation on a fixed shared fleet/pool/worker
configuration, compare matched natural failed CloseProcess publications:
caller/step scans/s and bytes, completed retry/work counts, publication timing,
checkout counts/waits, route latency/errors, CPU and healthy recovery. Preserve
capacity_settlement/close_process separately; its scans are intentionally not
eliminated. Global avoided-write counters include all trusted writers, so use
caller/step attribution and actual sample coverage; do not claim a per-retry or
CPU saving from fleet totals. No synthetic failure or production fault injection.

Hold expansion for missing eligible exposure, untrusted fallback growth or
unmatched windows. Roll back on altered pending work/receipt/stop semantics,
corruption regression, failed recovery or existing checkout/latency/health
triggers. Disable only this independent flag on the same compatible image,
prove all enabled processes are gone and reverify. Retain history, quarantine,
proof metadata and the shared cohort. Full #1320/#1349 acceptance remains open.
