## Review fixes applied 2026-11-13

The corrections below are prepared locally on source `095cbab547eeec8e29a97f69822cd1e1f96c3817`; publication to `feat/platform-spend` awaits the lead’s push approval. They supersede only the specifically identified implementation statements below. The author’s original description, validation history and footer remain above.

| Finding | Correction | Commit |
| --- | --- | --- |
| Stale evaluators can re-enter STOP after a lift | Closure persistence fences current settings and accounting evidence; stale lift, correction, release and policy snapshots are discarded. | `9727d6d81` |
| Cached STOP can cancel newly admitted work | A new cancellation receipt or platform pause requires current STOP authority in its committing transaction. | `07bdc3b08` |
| Missing or stale metering evidence can fail open | Admission derives refusal from current policy, clock and period evidence without depending on evaluator timing. | `04818783e` |
| Requests without owner policy can bypass reservation | Active platform metering refuses unvalued turns and auxiliary requests before provider dispatch. | `1b05f4506` |
| Unpublished recovery can reopen during STOP | Recovery checks current platform admission inside its transaction and preserves retained work until lift. | `af169e314` |
| Queued retained holds can bypass STOP after owner policy removal | Every queued reserved turn checks platform admission even when its owner policy is removed. | `25adbd3cd` |
| Exact Revive retries lose receipt recovery during STOP | Current resource authorization and the fingerprint are checked before new-commitment refusal. | `88d0bdb9b` |
| Every metered turn updates one shared period document | Optional sixteen-counter accounting removes routine period writes; admission keeps the existing settings fence and exact transactional sum. | `c5e64f502` |

The counter switch `CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED` is **false by default** in code and the ConfigMap. The first local comparison measured concurrent admission p95 **96.564 → 114.219 ms**, with reply payload **82,140 → 168,090 bytes** across 30 admissions, while settlement p95 fell **64.965 → 14.338 ms**. Root writes fell from 30 to zero in each measured accounting phase, and the profiler found zero collection scans. A second sample on the shared host again showed admission cost and settlement benefit. These are internal accounting transactions, not full provider latency; activation remains a later measured decision.

The final source sample measured concurrent admission p95 **63.224 → 55.244 ms** and settlement p95 **30.606 → 9.797 ms**, while admission reply bytes remained doubled. That timing variation does not erase the earlier admission regression or qualify activation. The original reproducible harness and measurements are committed in `5fee34d44` and `docs/notes/reviews/2026-11-13-pr1741-counter-evidence.md`.

Exact receipt recovery is not a new commitment: the same key and fingerprint return the existing durable Revive receipt during STOP after current resource authorization is rechecked. A new key remains refused, and changed input under the same key returns an idempotency conflict. This confirmed decision supersedes implementation NOTES round 3/R7 and follows the migration contract and same-key HTTP retry guidance. The regression fails before the fix and passes after; tests also cover revoked resource authority, new-key refusal and changed-fingerprint refusal.

The census extension belongs to this PR (`7ce0e2bec`), composed with #1754's classification at `49860d93` by `dc23792a3`. It recognizes exactly `platform-counter:<start-seconds>:<partition>` with sixteen fixed partitions, excludes independent accounting despite incidental ownership fields, refuses malformed keys, and preserves reservation `period_start`/`period_shard` bytes. The combined fixture includes the root `counter_layout: 1` marker (`e36a272d9`). The candidate normally merges develop `3bf6b51cf` through `095cbab54`, including #1754's merged history at `dd528496831e68ee008d38e2f81137b97830b486`. Serving, rollback and census readers must support the complete layout before conversion is enabled; OFF preserves accounting for already converted periods.

**Superseded statements in the original description:**

- **Superseded: single-period-row accounting.** The original period-ledger description applies to unconverted periods. The optional layout stores model balances in the bound counters; retained settlement follows the original period and partition.
- **Superseded: closure and STOP concurrency rules.** Refusal closures use a transaction fencing policy, root and all contributing counter revisions. A cached positive STOP schedules a transactional check before any new cancellation receipt or pause. The optional counter path removes the local settlement queue while retaining the admission queue.
- **Superseded: failed-pause recovery follow-up.** Automatic unpublished recovery refuses a new reopening during STOP (`af169e314`); this is no longer an outstanding defect.
- **Superseded: blanket Revive refusal.** STOP refuses new commitments. Exact authenticated receipt replay remains available, as described above.
- **Superseded: census shape limitation.** The composed reader recognizes existing period, observation and STOP shapes from #1754 plus the exact counter family from this PR. #1754 has merged at `dd5284968`; the complete reader must still be deployed before enabling the corresponding writers.
- **Superseded: no existing record rewrite or executable migration.** Optional first admission atomically converts one period and creates sixteen counters without reservation backfill. Closing the switch does not restore predecessor-reader compatibility. Recovery requires the period, its counters and retained reservations together.

Closes #1496. **Superseded: the original Refs-only linkage and statement that merge does not close #1496.** This closing link records the source implementation; production verification, compatible release artifacts and activation remain release-owner work and are not certified by these local tests.

The independent primary review of all 126 files at original head `821790b422b336684256abbf2b00d30386f40e99` and the full `8d318edc..821790b4` security-head delta is committed in `55d55f176`: `docs/notes/reviews/2026-11-13-pr1741-primary-review.md`. Correctness baselines and corrected results are in `docs/notes/reviews/2026-11-13-pr1741-fix-evidence.md`.

The complete backend gate at `e80ef5a7b1417e1cf124053f0477bd0339762ba0` passed **5,575/5,575** ordinary tests across five crates and **1,092/1,092** Mongo contracts across 32 disposable fixtures, including the isolated large-scale fixtures; wrapper exit **0** after cleanup. All-target/all-feature clippy with warnings denied, doctest compilation and Rust formatting passed. The final merge of the two newer develop changes is qualified separately below; the earlier full run is not attributed to that later API tree.

The TypeScript SDK passed **4,441 tests**, with one existing endpoint-dependent skip and **96.51% line coverage**, plus lint, formatting, typecheck, build, 54 documentation examples and package-consumer checks. Its tested tree is byte-identical at `095cbab54`, as are the tested Layer 4 source and full Mongo harness.

Final integration at `095cbab547eeec8e29a97f69822cd1e1f96c3817`: **4,957/4,957** ordinary tests across the affected control-plane and adapter crates, **91/91** focused Mongo contracts, five-crate all-target/all-feature clippy, doctest compilation and formatting all passed; wrapper exit **0** after cleanup. Source-size, deployment-key and configuration-documentation checks passed. The source review is **GO**; publication still awaits the lead's push approval.

The committed final source review, complete results and log fingerprints are in `docs/notes/reviews/2026-11-13-pr1741-candidate-review.md`. The final documentation-only publication head will be recorded in the lead's push request after checking its code/configuration/harness trees against the tested source. Remote CI still describes `821790b4` (23 passing, Mongo contract and dependent Backend checks failing); the unchanged terminal-settlement assertion passes in the complete local run. CI has not run on the unpublished corrections.
