# Segmentation combined PR: current assignments and gate ledger

Updated Thu 2026-11-12 08:11 SGT. S2 owns
`tl/cma-log-segments/integration-20261111`. One default-off combined PR to develop;
strict-green and PR by14:00, reviewer3 by15:30, explicit relay/merge by16:00,
evening train19:00–22:00. Writer and recovery activation remain closed.
No issue closes without its required production evidence. Native recovery remains
Unsupported; source qualification does not claim supported production activation.

## Final handoffs normally integrated

Combined source `ff0f161b8` now also merges final S3 `e58162249` normally;
the prior qualified source is `e28cb6beb`. Current-head strict CMA passes with zero diagnostics (2m36s).
All currently published handoffs are included:

| Owner | Pin / branch | Included work |
| --- | --- | --- |
| S1 cma-cost | `f0d9746ef` response-consumers + `b24fb1c57` reader-foundation | Indexed response consumers, retained readers, legacy/v2 shared caller wiring, actual factory/public tests. |
| S4 cma-seg-lead | `d8e10b63d` writer-publication | Scoped reconciliation, accepted-recovery composition, positive current activation caller, invariant agreement, final writer/recovery gate evidence. |
| S5 cma-seg-3 | `2144909ce` capacity-recovery (ancestor of S4 final pin) | Sequence-only progress correction `00426512f`, original-witness recovery, later-turn fixture and qualified restart/loss cases. |
| S3 cma-seg-2 | `e58162249` transcript-consumers | Final published checkpoint normally merged. Title-format and suggestion snapshot fixes retained; P1-4 bounded transcript/title work and selected parity/scale evidence remain open. |
| S2 cma-db-contention | `c4e4b995a` proof; `3fc57ec55` config split | Positive transaction-bound current publication, canonical schema/checksum/runtime roles, scheduler and integration. |
| Develop | `3aee1ff68` | Retained-context and decoded-JSON policies normally included. |

Conflicts preserve every flag/parser/test once. Shared response reader attaches
before either router is built. The response binding helper consumes the existing
FeatureFlags to retain both independent policies within its argument bound.
The duplicate quota child from S1 is removed only after verifying the existing
registered assertions. No suppression or synthetic production consumer added.

## Exact current owner assignments — lead relay now

All paths below are relative to `api/crates/cma/src/` unless stated otherwise.
Owners start from this combined source (or merge it normally) before new fixes.
Publish small pins plus exact selection/count/failure evidence. Do not edit other
owners' source. S2 owns shared registration and resolves small module registrations
from owner pins; owner tests should live in named new child files below.

| Owner | Exact remaining item and file ownership | Deadline Thu SGT |
| --- | --- | --- |
| **S1 cma-cost** | P1-4 response/reader half: verify final shared caller dispatch, retained v2 reads with admission/optimization off, large-history/long-turn/indexed-event parity and cost bounds on the combined pin. Own `projections.rs`, `projections/response_readers_tests.rs`, response-store/core/public paths from the01:55 allocation; report any new response diagnostic. Qualification selection must include both factory tests, public `responses::`, actual response projection/snapshot/scale cases and retained-context success/refusal imported from develop. Provide a complete evidence inventory for the final PR; do not fix capacity/reconciliation reachability. | Pin or no-code evidence map09:00; source fixes10:00; selected actual results11:30. |
| **S4 cma-seg-lead** | **P1-1 still open:** ordinary v2 claim/reserve/target-control roots in `store/child_writer_claims.rs`, `store/agent_commands/reserve.rs`, `store/agent_commands/targeted_control/apply.rs`; inventory remaining ordinary roots. **P1-N1 still open:** `store/agent_log_segments/writer_prepare.rs` and writer-plan/persist permission for an independently admitted segmented CapacityRecovery successor. Own writer/source-contract fixes and tests under `store/sandbox_mongo_integration_tests/agent_segment_tests/`. Retain fresh capture, fencing, immutable receipt and no input replay. Own any recurring strict diagnostics in `sandbox_reconciliation/**` or accepted-work composition `service.rs/accepted_service.rs/worker*.rs/transactions.rs`. | Exact successor contract09:00; source pins10:30; root/adoption/abort/unknown tests11:30. At09:00 flag any item that cannot fit. |
| **S5 cma-seg-3** | Own **P1-N1 actual service admission coverage** after S4's writer contract: already-segmented exhausted source through preparation/admission to atomic seal/successor/carrier; abort, Unknown/restart, closed/wrong admission gate and immutable original witness. New test child `store/sandbox_mongo_integration_tests/agent_capacity_recovery_tests/segmented_admission.rs`; request registration from S2. Own required fixes in `agent_capacity_recovery/{preparation,admission,authority,records,receipts,restart,accepted_authority}.rs`, with no S4 service edits. Reconcile the combined four old runtime failures against final2144909ce and provide exact closure mapping. Keep production native Unsupported. | Old-failure/contract map09:00; source/test pin10:30; actual service/refusal results11:30. |
| **S3 cma-seg-2** | P1-4 transcript/usage/title and P2 selected-consumer matrix. Own `agent_log_segment_reads.rs`, transcript/usage/title projection paths, Layer2 folds and selected-reader tests already allocated. Bound requested-page work across1/10/100/1000segments; >10,128rows/>4MiB, long turns, continuation, serialized parity, corrupt/stale proofs, generation/retirement, snapshot races, final authority, no read-side writes. | Published final source10:30; bounds/parity matrix11:30; risk report09:00. |
| **S2 cma-db-contention** | Integration, `transaction_scope`/shared exports, canonical migration/schema/index/runtime grants, source-size/conflict resolution and all combined gates. Own any recurring `TransactionScope::insert_one` or canonical registration diagnostics. Run strict CMA first on e28cb6beb, then current recovery/scheduler/adapter/authenticated Mongo and partitioned contracts; add final S3/owner deltas and qualify the final head. Prepare PR body/evidence, rollout/rollback reader floor and exact blocker dispositions. | Initial strict inventory ASAP; freeze source11:30; combined strict/all required gates13:30; PR14:00. |

S4 and S5 agree the segmented capacity-successor contract in this note by09:00;
use the existing independent CapacityRecovery admission rather than relabeling it
Rollover. Source at e28cb6beb still rejects any segmented supplied admission other
than Rollover (`writer_prepare.rs`), so the31recovery passes do not close P1-N1.
S4's ordinary v1 predicates likewise remain P1-1, not an inferred closed item.

## Strict diagnostics: attribution and current evidence

S1 b24fb1c57 had22normal/20test unused imports/dead roots: capacity admission,
authority, records, source/status; scoped sandbox reconciliation; and
TransactionScope::insert_one. Those callers were absent from that slice branch.
Combined **b3052f1b6 strict CMA passed with warnings denied, zero diagnostics**
in2m58s. Therefore these historical counts are not current combined blockers.
If they recur after final integration, route capacity primitives toS5, accepted
service/reconciliation toS4, transaction/export registration toS2. No allowance,
feature-gate suppression or fake call is an acceptable closure.

## Executed gate ledger

| Source | Gate | Actual result / limit |
| --- | --- | --- |
| `b3052f1b6` | Strict CMA all targets/features, -D warnings | PASS, zero diagnostics,2m58s. |
| `b3052f1b6` | Scheduler/config | 56passed/0failed,2638skipped; inventory verified all7accepted-recovery scheduler tests. |
| `b3052f1b6` | Adapter | 47passed/0failed,698skipped. |
| `b3052f1b6` | Canonical authenticated Mongo | 1passed/0failed,2693skipped; runtime roles/schema/carrier/index contract. |
| `b3052f1b6` | Recovery Mongo | **26passed/4failed**,2664skipped,71.548s. Two S4 later-service and two S5 later-restart cases failed. Keep as failed evidence; retained gate did not run. |
| `2144909ce` (S5 final author) | Recovery Mongo | 31passed/0failed/0ignored,90.81s; corrections now integrated, combined rerun required. |
| `2144909ce` (S5 final author) | CMA/core/public/adapter | 75/49/8/7passed;76ignored CMA tests are not passes. Four-package strict passed2m40s. |
| `2144909ce` (S4 author) | Writer/recovery Mongo | 71passed/0failed (40writer +31recovery),177.547s; CMA/core166passed; strict passed. |
| `b24fb1c57` (S1 caller author) | Fresh configuration/factory/public response | 17+2+104passed,0failed;1existing SDK fixture ignored. Earlier stale-binary attempt rejected. |
| `b9d390634` (S1 response author) | Actual Mongo/scale | 13passed/0failed; at1/10/100/1000sealed units fixed requested output uses3operations/13rows/6408BSON bytes. |
| `44ad38198` | Positive publication API | 13units +9actual Mongo passed; schema checksum-only delta c4e4b995a. |
| `c3ca9e3aa` | Mongo orchestration runner | 31mocked tests passed; not actual Mongo evidence. |
| `557c07593` | Secret history | 3992commits,zero findings; later final head needs scan. |
| `e28cb6beb` | Strict CMA all targets/features, -D warnings | PASS, zero diagnostics, 2m36s. Final S1/S4/S5 assembled source. |
| `e28cb6beb` | Strict core/public/adapter all targets/features, -D warnings | PASS, zero diagnostics, 1m04s. Behavioral gates continue. |
| `e28cb6beb` | Shared caller/configuration | 26passed/0failed,2674skipped; both required factory tests and strict default-off flag tests selected. |
| `e28cb6beb` | Scheduler/configuration | 57passed/0failed,2642skipped; all7accepted-recovery scheduler tests verified in the inventory. |
| `e28cb6beb` | Public response tests | 108passed/0failed,513skipped. |
| `e28cb6beb` | Consumer/core tests | 205passed/0failed,1305skipped. |
| `e28cb6beb` | Adapter tests | 47passed/0failed,711skipped. |
| `e28cb6beb` | Mongo/supplemental remainder | Intentionally canceled before authenticated Mongo when final S3 arrived; status143 is supersession, not an executed test failure. |
| `ff0f161b8` | Strict CMA all targets/features, -D warnings | PASS, zero diagnostics,2m36s; formatting also passed. Remaining combined gates continue under heavy-1. |
| `ff0f161b8` | Strict core/public/adapter | PASS, zero diagnostics,58.94s; canonical size6259files and config-document checks passed. |
| `ff0f161b8` | Shared caller/configuration | 26passed/0failed,2674skipped; both required production factory tests selected. |
| `ff0f161b8` | Scheduler/configuration | 57passed/0failed,2642skipped; all7accepted-recovery scheduler tests selected. |
| `ff0f161b8` | Public/core/adapter | 108/205/47passed, zero failures;513/1305/711skipped respectively. |
| `ff0f161b8` | Canonical authenticated Mongo | 1passed/0failed,2698skipped. |
| `ff0f161b8` | Recovery Mongo | 31passed/0failed,2668skipped,74.194s; all four prior later-progress failures now pass. |
| `ff0f161b8` | Retained/discovery Mongo | **3passed/1failed**,2695skipped,7.550s. Discovery fixture incorrectly expects recovery eligibility while PublishConnection remains pending. Supplemental gates stopped. |

Original failed recovery cases: S4
`capacity_later_service_retries_crash_and_refuses_foreign_or_stale_activation`,
`capacity_later_service_unknown_claim_never_activates_and_retry_uses_original_completion`;
S5 `capacity_later_progress_recaptures_parent_and_publication_before_acknowledgement`,
`capacity_later_progress_restarts_with_current_target_without_rewriting_completion`.
The final handoff corrects revision-only assumptions and missing OwnerMessage
fixture evidence. S5 owns the precise case-to-correction mapping; final combined
execution must actually select the cases.

## Review ledger and activation boundaries

| Finding | Current disposition |
| --- | --- |
| P1-1 ordinary v2 roots | OPEN, S4; must qualify actual worker claim through ordinary/control admission and output. |
| P1-2 canonical runtime roles/carrier/index | Source-closed, S2; canonical authenticated Mongo passed onb305. Final combined rerun remains. |
| P1-3 later-progress activation | Source and author gates qualified2144909ce; final combined rerun/reviewer required. Native Unsupported remains a separate activation prohibition. |
| P1-4 / D6-2 large-history response/transcript | S1 source/author response gates handed off; S3 bounded transcript and final shared parity pending. |
| P1-N1 segmented capacity admission | OPEN, S4writer + S5service tests; not covered by legacy bootstrap success. |
| P2 / D6-5 selected-consumer matrix | OPEN, S3, with response portionS1. |
| P2-N2 changed file exactly500lines | Source closed by split; final size gate required, task limit <500. |
| P3 archived trailing blank | Source corrected; base-to-head diff check remains required. |
| P2-5 unresolved keyed attempt; P2-6 child total bound; S5 strict BSON/discovery; D6auth/enum/config | Source-closed; retain existing success/refusal regressions in final selections. |

One PR to develop with issue links; no automatic closure claim for unmet
production acceptance. Final PR-ready checks: fmt, strict CMA and affected-layer
lint, current source inventory, required core/public/adapter/unit tests, actual
Mongo scope/capture/schema/isolation/writer/root/recovery/consumer contracts,
large-history bounded cost/parity, config/manifest/size/structure, secret history,
reader-floor/rollback documentation and explicit reviewer verdict. Full CI on
the exact release artifact is the release owner's gate. Merge only on lead relay.

At09:00, name each missing dependency/owner and whether13:30qualification can
finish. Do not hide a P1 behind default-off or trade a required gate for16:00.
Native capacity recovery activation has no defensible ETA until independently
pinned-runtime confirmation exists. Writer/recovery admission remains closed
through this PR; release owner handles production, deployment and verification.

## Resource and source protocol

All heavy steps use one shared slot, `lockf -k -n -t 3600`, at most2Cargo jobs,
with lock release between gates. Never recreate/delete shared locks. Heavy-1
still has original inode160080032; own runner verifies device/inode before and
during execution. One Cargo target per worktree; minimum30GBfree, stop before
crossing the floor. Remove only owned target exports/containers/volumes after
use; do not touch others' worktrees. Gate archives pin source independently of
ongoing merges and clean workspace-package artifacts when source identity changes.
Actual counts, skipped tests and failures remain tied to their exact source.

## Handoff sources and prior ledger

- S1: `2026-11-12-response-caller-handoff.md` and response-consumer-handoff.md.
- S4: `docs/notes/2026-11-12-s4-later-progress.md` and the linked Markdown gate record.
- S5: `2026-11-12-s5-final-handoff.md`.
- Frozen recovery carrier: `daf605ab410fc4737633aa3997e67192d4c4112b`, format2only.
- S2 proof: `2026-11-12-current-publication-proof.md`.
- Prior central ledger (verbatim, split below500lines):
  [2026-11-12-segments-coordination-history-13.md](2026-11-12-segments-coordination-history-13.md)
  [2026-11-12-segments-coordination-history-14.md](2026-11-12-segments-coordination-history-14.md)

## Final-handoff gate notes

Light gates at assembled e28 source: formatting, config documentation and its4
unit tests passed; canonical size6259files passed. Secret scan at11d917dd9
covered4015commits with zero findings. Imported S4 JSON evidence was preserved
in Markdown to satisfy the docs-only-Markdown structure rule; original source
counts/meaning unchanged. The combined PR body is prepared locally at
`2026-11-12-segments-combined-pr-draft.md`; it is not an open PR.

Current exact-source gate inventory verifies both new production factory tests,
raw-output and retained-JSON flags, reconciliation flag, and all7capacity scheduler
tests before executing them. No stale-target attempt will be accepted.

Strict CMA on the assembled final handoffs now passes without suppressions.
The historical S1 22/20 diagnostics are resolved by integration. Affected-layer
lint also explicitly selects Layer3. Remaining source findings retain the owner
assignments above; strict lint is not their behavioral closure. Supplemental
selection covers remaining segment/operator/proof units, recovery HTTP cases and
the partitioned real-Mongo inventory after the ten-step batch succeeds.
All changed Rust/TypeScript files are independently confirmed below500lines.

Current qualification status: new source ff0f161b8 is exported immutably and
strict CMA passed with zero diagnostics in2m36s. The e28 runner completed both lint gates plus26caller,
57scheduler,108response,205core and47adapter tests. Preserve their separate
selection counts; overlapping suites are not summed as unique test coverage.
The obsolete e28 Mongo/supplemental remainder was canceled before execution
when S3 arrived. New-source gates include those Mongo requirements afresh.

S3 merge resolution: retain the already-wired production caller/service roots,
FeatureFlags argument boundary, one copy of each flag/parser/test, the existing
row-and-byte-bounded compatibility helper and its empty-range proof. Do not
import S3's isolated-branch dead_code/too_many_arguments suppressions. Its title
format discriminator and suggestion fallback snapshot changes are included.
The handoff adds no new transcript parity/scale tests; P1-4/P2 stay withS3.
Current transcript traversal still starts at zero inagent_log_segment_reads.rs;
selected fallback/title work still needs fixed-request bounds and the matrix.
All idle owners retain the exact assignments above. New strict diagnostics,
if any, are assigned from the actual combined compiler output, not inherited
historical slice counts. Writer/recovery activation remains closed.

S3 qualification must explicitly cover suggestion snapshot fallback after a
segmented proof refusal. The new bounded_log branch catches Corrupt,
SequenceConflict, capacity, invalid-page and missing-source outcomes before
raw range lookup; prove that retained/current source validation is not bypassed
and separate unpublished append handling from ordinary read validation.
This is assigned toS3 with the existing refusal matrix, not an assumed pass.

## Combined discovery fixture correction — S2

S2 owns only `store/sandbox_mongo_integration_tests/agent_segment_tests/worker_discovery.rs`
for the new executed failure at line54. S4 retains ordinary production roots.
`ready_source` returns Ready status with PublishConnection still pending; the
candidate contract correctly requires work None and no lease before recovery.
The correction finishes the legacy connection through claim/commit, then checks
both positive completed eligibility and refusal for pending adopted work. Both
stored documents must remain unchanged by discovery. No production predicate,
fence, recovery admission or writer boundary changes. Targeted rerun is required;
do not count this correction as passed before execution.
