# Segmentation combined PR: current assignments and gate ledger

Updated Thu 2026-11-12 11:10 SGT. Integration owner: S2 cma-db-contention.
Branch: `tl/cma-log-segments/integration-20261111`.
One default-off combined PR to develop; target PR 14:00, review 15:30, merge 16:00,
evening train 19:00–22:00. Writer and recovery activation remain closed.
No issue closes without its required production evidence.

**Current outcome:** strict CMA/core/public/adapter is green at `92ed005a9`.
The combined selection passes 609 unit tests,361 ordinary Mongo contracts and the
separate authenticated migration/runtime-role contract. All have zero failures.
This qualifies the published source, not the unresolved source-review findings.
The PR is not open. The 14:00 target is at risk: S3/S4/S5 follow-up pins are absent
at 11:05, after the requested 10:30 source cutoff. No owner ETA has been confirmed.

## Handoffs included normally

| Owner | Published pin | Included work |
| --- | --- | --- |
| S1 cma-cost | `f0d9746ef` + `b24fb1c57` | Indexed response consumers, retained readers, legacy/v2 shared caller wiring and factory tests. |
| S4 cma-seg-lead | `d8e10b63d` | Scoped reconciliation, accepted-recovery composition, current activation caller and writer/recovery contracts. |
| S5 cma-seg-3 | `2144909ce`, ancestor of S4 | Original-witness recovery, sequence-only/later-turn progress, restart/loss fixes. |
| S3 cma-seg-2 | `e58162249` | Transcript checkpoint, title format discriminator and suggestion snapshot handling. Bounded-consumer findings remain. |
| S2 cma-db-contention | `c4e4b995a`, `3fc57ec55` | Transaction-bound current-publication proof, canonical schema/index/runtime roles, shared registrations and configuration split. |
| Develop | `b8aeef2af` | #1726 retirement guards and #1713 skill/reopen/always-on changes. |

All pins above are ancestors of 92ed. Final handoffs eliminate S1's historical
22 normal/20 test dead-code/import errors without warning allowances or synthetic
callers. Source is immutable during qualification; exact test names and source
hashes are recorded in 0600 local evidence.

## Remaining work: exact owner and next handoff

Paths below are relative to `api/crates/cma/src/` unless otherwise stated.
The 10:30 source cutoff has passed. The next requested source/ETA update is 11:30;
this is an integration request, not an accepted delivery promise. A later pin
must include its exact selected tests/results so S2 can budget final qualification.

| Owner | Required item and owned files | Current state / dependency |
| --- | --- | --- |
| S1 cma-cost | Final response disposition for `projections.rs`, `projections/response_readers_tests.rs`, allocated response/core/public paths and 01:55/05:20 shared callers. Reconcile legacy live replay's selected-turn cost with the declared request bound. | Integrated response/factory/scale tests now pass on 92ed; no extra build required for a no-code evidence handoff. S1 retains response edits. |
| S3 cma-seg-2 | P1-4/P2: `store/agent_log_segment_reads.rs`, transcript/usage/title projections, `store/agent_titles/bounded.rs`, selected Layer2 folds/tests and `store/suggestions/context.rs` proof-refusal handling. | No follow-up pin. Bound requested-page work at 1/10/100/1000 segments, long first turns and over 10,128 rows / 4 MiB; add parity, continuation, final-authority, stale/corrupt proof and no-write matrix. |
| S4 cma-seg-lead | P1-1: `store/child_writer_claims.rs`, `store/agent_commands/reserve.rs`, `store/agent_commands/targeted_control/apply.rs`; inventory ordinary roots. P1-N1: `store/agent_log_segments/writer_prepare.rs` and writer plan/persist permission. Own root tests under `store/sandbox_mongo_integration_tests/agent_segment_tests/`. | No follow-up pin. Exact v2 capture authority must replace v1-only Agent predicates; parent stays independently qualified. Segmented CapacityRecovery needs an independent storage-only successor, never relabeled Rollover. Preserve `retirement_archive_pending()` guard. |
| S5 cma-seg-3 | P1-N1service tests in new `store/sandbox_mongo_integration_tests/agent_capacity_recovery_tests/segmented_admission.rs`; necessary fixes in recovery preparation/admission/authority/records/receipts/restart/accepted_authority. | No follow-up pin. Use existing frozen service/writer signatures below; success needs S4's implementation. Test actual segmented exhausted-source admit, atomic successor/carrier, abort, Unknown/restart, wrong/closed gate and original witness. S2 registers the child. |
| S2 cma-db-contention | Combined integration, shared adapter/registration/transaction scope, canonical migration/schema/index/runtime grants, source-size/conflict resolution, gate inventory and PR. | Current strict diagnostics0; all selected combined gates pass. Merge owner deltas normally and qualify their final combined source before claiming readiness. |

S4 retains accepted-work `service.rs/accepted_service.rs/worker*.rs/transactions.rs`
and `sandbox_reconciliation/**` wiring. S5 does not edit those files. Any recurring
`TransactionScope::insert_one`/export/canonical-registration error belongs toS2.
No remaining compiler diagnostic is assigned from historical slice-only counts.

## Current review disposition

| Finding | State at92ed005a9 and required closure |
| --- | --- |
| P1-1 ordinary v2 roots | **OPEN S4.** Agent CAS remains v1 at `store/child_writer_claims.rs:205`; reservation uses the same v1 filter for Agent and parent at `store/agent_commands/reserve.rs:154`; targeted control uses the legacy scanner. Require actual adoption→discovery→worker claim→ordinary/contextual/targeted admission→output, with refusal and abort cases. |
| P1-2 runtime roles/carrier/index | Source and executable gates pass; reviewer confirmation remains. Canonical authenticated migration1/1; actual nonunique discovery/index drift and unregistered format-1 carrier refusal pass. |
| P1-3 later progress | Source/executable closure ready for review: all 31 recovery Mongo cases pass, including all four historical later-service/restart failures. Production native confirmation remains typed Unsupported; that separately prohibits activation. |
| P1-4/D6-2 large-history consumers | **OPEN S3**, with response disposition S1. Response compatibility refuses bounded-vector overflow and indexed pages succeed beyond both old limits. Transcript traversal still starts at zero and title reconstruction retains/refolds a growing first-turn prefix. No unlimited compatibility-vector closure is claimed. |
| P1-N1 segmented capacity admission | **OPEN S4+S5.** `writer_prepare.rs:123` rejects supplied segmented admission unless Rollover. Existing 31 recovery tests do not cover successful already-segmented CapacityRecovery admission. |
| P2/D6-5 parity/failure matrix | **OPEN S3**, response portion S1. Need segmented transcript/title/usage serialized parity, long turns, page bounds, invalid proof, snapshot/final-authority and no-write coverage. |
| P2-N2 under500lines | Composition reaches 500 after develop sync; removing one empty import/impl separator makes 499, with every nonblank line unchanged. All 379 changed source files pass the strict under-500 check. Earlier store split retained. |
| P2-5 unresolved keyed attempt; P2-6 total child budget; strict BSON/discovery; D6auth/enum/config | Source fixes retained; relevant combined units/Mongo pass. Reviewer final readback remains. |
| P3 trailing blank | Corrected; working and base-to-head whitespace checks pass. |

S3 must distinguish an unpublished append from ordinary read/claim/completion.
`context::current` runs with both snapshot true and false; `!snapshot` alone is
not permission to ignore Corrupt/SequenceConflict and read raw rows. If enqueue
needs an exception, require explicit validated pending-publication authority.

## Frozen S4/S5 admission interface

No new signature is needed for S5's service tests:

```rust
// agent_capacity_recovery/service.rs, within impl RecoveryService
pub(crate) async fn admit(
    &self,
    caller: &Caller,
    request: &RecoveryRequest,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;

// agent_log_segments/writer_commit.rs
pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError>;
```

Already-segmented recovery uses bootstrap None and the original independent
CapacityRecovery admission. Preserve source/operation/result identity and fresh
capture; publish seal/successor/Agent/carrier atomically. No event/input replay,
new conversation, premature Ready, or inference of KnownAbort from absence.
Frozen carrier pin `daf605ab410fc4737633aa3997e67192d4c4112b` stays format 2 only;
format-1 drafts never register or relabel. S4 owns internal writer permission;
S5 owns actual service success/refusal tests; S2 owns shared module registration.

## Executed gates on 92ed005a9

| Gate | Result |
| --- | --- |
| Strict CMA/core/public/adapter, all targets/features, warnings denied | PASS, zero diagnostics, 2m52s. |
| Combined unit selection | 609 passed / 0 failed, 5069 outside selection/ignored, 8.894s: CMA 193 / public 122 / core 225 / adapter 69. Both production response factories, all 7 recovery scheduler cases and imported skill opt-in regression executed. |
| Authenticated canonical migration/runtime roles | 1 passed / 0 failed, 2732 skipped, 1.239s. Uses real canonical migration, scoped runtime grants and format-2 carrier. |
| Ordinary Mongo inventory/execution | **361 distinct selected tests passed, zero failures** across 10 fixtures, at most 37 per fixture. Exact executed set equals the immutable nextest inventory; no duplicates or missing tests. |
| Partitions 1–5 | 37/36/36/36/36 passed; 101.250/74.822/177.783/58.227/102.683s. |
| Partitions 6–10 | 36/36/36/36/36 passed; 58.083/120.023/70.054/263.795/76.801s. |
| Current recovery coverage | 31 cases, including all four prior later-progress failures. This count is a subset of 361, not additional tests. |
| Current response Mongo coverage | 11 response-specific cases, including fixed-request scale 1/10/100/1000, long-turn numeric seeks, snapshot/corrupt/foreign/refusal and bounded compatibility. Also a subset of 361. |
| Canonical index/schema | Discovery keyset, missing/extra indexes/collections, exact index drift and unregistered carrier migration refusal all execute and pass. |
| Archive compatibility | Unit catalogue and actual every-unsupported-namespace nonempty refusal/empty success both pass. |
| Light gates | Format and config docs/key matching pass; canonical inclusive size 6328 files passes. All 379 changed source files are under 500 lines; six canonical structure checks and base-to-head whitespace pass. |
| Secret history | 4093 commits, zero findings. Final documentation head gets its own readback. |

The four old failures now pass on this exact source:

- `capacity_later_service_retries_crash_and_refuses_foreign_or_stale_activation`
- `capacity_later_service_unknown_claim_never_activates_and_retry_uses_original_completion`
- `capacity_later_progress_recaptures_parent_and_publication_before_acknowledgement`
- `capacity_later_progress_restarts_with_current_target_without_rewriting_completion`

Response scale asserts 3 source operations / 13 source rows / <16 KiB source bytes,
<60 checkouts/commands and <256 KiB wire replies per fixed sample, no writes or
transaction-control, and no collection scan/blocking sort. This is executable
local bound evidence, not measured production latency. Historical author 6408 B
and 24–26 checkouts remain historical;3 source operations never means 3 checkouts.
Legacy live replay uses bounded original-row pages but total work grows with the
selected turn to preserve intermediate updates. Do not claim constant total work
for sparse legacy replay or silently remove intermediate tool updates.

Evidence under `/Users/chronoai/code/work/tech-lead/notes/` (0600):

- `2026-11-12-segments-integrated-results-92ed005a9.json`: 14 successful gate steps.
- `2026-11-12-segments-mongo-pass-inventory-92ed005a9.json`:selected/executed equality,
  partition timings, hashes and named review regressions.
- `2026-11-12-segments-unit-inventory-92ed005a9.json`: 609 actual unit passes by binary.
- `2026-11-12-segments-composition-equivalence.json`:one blank removed, no nonblank change.
- `2026-11-12-segments-combined-pr-draft.md`:prepared body, not an opened PR.

## PR plan and schedule risk

One combined PR remains the plan. A schema-only split would not close the open
writer/consumer findings or qualify an activated state, and is not needed for
this disabled code deployment. Link #1348/#1349/#1350/#1351/#1674without premature
issue closure. Require all source blockers resolved and a final reviewer verdict.

At11:05all published owner branches, including alternate cma-cost/segmentation
refs, remain unchanged. S3/S4/S5 pins requested by 10:30 are missing. S1 may hand off
a no-code response evidence disposition using the actual current combined results.
The next requested owner source/ETA update is 11:30. S2 can begin final integration
immediately on those pins; the time for strict green/PR 14:00cannot be guaranteed
without them. The lead's pending request for updated pins/ETAs remains unanswered.
No gate is dropped to preserve 16:00. If source is still incomplete, report the
exact unresolved findings for the Friday fallback rather than call this ready.

Final PR gates: fmt, strict CMA/affected-layer lint, exact selected test inventory,
core/public/adapter/factory units, authenticated schema/runtime permissions,
partitioned real-Mongo scope/capture/index/writer/roots/recovery/consumer contracts,
large-history bounds/parity, canonical structure/config/size/whitespace, secret
history, and reader-floor/rollback documentation. Full CI's exact batch artifact
remains a release gate. Merge requires the exact-head review and explicit lead relay;
no combined PR merge relay exists. The 09:12 freeze lift permits normal branch work.

## Migration, archive and activation boundaries

Migration is additive and explicit; startup verifies without DDL/adoption. Keep
all writer/recovery admissions closed, preserve raw rows, IDs, receipts and homes,
and never downgrade retained v2 records to a v1-only binary. A source capability
name is not a qualified rollback artifact. The release owner must pin a compatible
image, coherent backup/restore evidence and native capability before activation.
Production native recovery remains typed Unsupported, so recovery cannot activate.

The imported retirement archive currently proves only the legacy ownership
closure. All 9 segmentation sidecars use its explicit Empty policy; any retained
row, including a rollout gate, refuses capture without omitting data. Closing a
gate does not empty it. Empty additive schema is compatible, as qualified above.
A reviewed populated-sidecar archive/restore closure is an additional activation
dependency owned byS1with archive-owner review. No deletion or bypass is authorized.

## Historical evidence and resource protocol

Prior failures/superseded gates are retained, not relabeled as current passes:
[qualification history15](2026-11-12-segments-coordination-history-15.md), with
links to earlier assignment/ownership histories 13/14. In particular, a3b935269's
first Mongo partition executed 40 passing cases but exited 127 because an external
helper changed while Bash was running. It is a failed orchestration gate;
remaining 8 partitions did not run. Cleanup was verified. Current 92ed helpers were
unchanged throughout execution and all 14 steps returned 0.

Heavy steps use one existing heavy-1 slot, `lockf -k -n -t 3600`, two Cargo jobs,
and release the lock between steps. Device/inode 16777231/160080032 must match.
One owned target; never alter shared locks or other worktrees. Current fixtures
cleaned up on exit; verify own resources absent before the final report. Keep
30 GB free. Current executable gates use immutable source 92ed; subsequent changes
to only documentation or the blank composition separator are separately proved,
never falsely labeled a different tested source hash. Production untouched.


## S4 ordinary v2 admission allocation and 13:00 pin

Branch `tl/cma-log-segments/ordinary-v2-admission` starts at `4b148fba5`.
S4 owns the exact Agent envelope changes in `store/child_writer_claims.rs`,
`store/agent_commands/reserve.rs`, `store/agent_commands/targeted_control/{apply,fences}.rs`,
and the bounded transaction-local proof in `store/agent_log_segments/ordinary.rs`
(with its single module registration). Parent fences and retirement archive guards
remain independently enforced. Tests live under `store/sandbox_mongo_integration_tests/
agent_segment_tests/{ordinary_roots,admission_contexts}.rs` and
`admission_contexts/support.rs`; the existing discovery fixture exposes its
connection-completion helper for the actual adopted claim/publication regression.

P1-N1 uses a distinct `RolloverPermission::SegmentedRecovery`, supplied only by
an exact durable CapacityRecovery admission. It forces a seal and empty successor,
without event replay or sequence movement; it refuses legacy-source mismatch,
empty source, appended events and terminal reserve classes. Writer preparation
preserves conversation/worktree state and requires a non-Ready result. S5 keeps
actual service admission success/refusal tests and the original dual-gate contract.

Additional P1-1 inventory: `store/agent_conversation_store::apply_mutation`,
`store/agent_conversation_store::checkpoint::{apply,write}` and
`store/agent_commands::reconcile::apply` still use legacy publication/settlement.
The three named claim/admission fixes alone therefore do not close P1-1's full
post-adoption output requirement. Exact additional ownership/dependency was
requested through the lead. Keep writer/recovery activation closed and both
issues In Progress. Final-source gate counts and the exact pin are appended below;
no unexecuted test or previous-pin pass is a qualification of this pin.
