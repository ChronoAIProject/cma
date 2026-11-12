# Strict-lint assignment chronology snapshot

Preserved from the lead assignment note at Thu 00:29 SGT.

Release order: additive schema under explicit reviewed operator authority; segment-aware readers everywhere and qualified rollback image; writer cohort with recovery admission closed; separately qualified capacity-recovery cohort; wider activation only with matched route/checkouts/scan evidence. Close new admission on any authority/integrity/duplicate-input/unknown-outcome classification fault while retaining accepted-work recovery, compatible readers and fitting appends. Never roll back to a binary unable to read already-written envelopes/sidecars.

Production acceptance needs representative open-Agent/response/transcript/desktop samples, bounded scan/byte/checkout improvements, no correctness or availability regression, at least 36 minutes of canary watch **and a real natural rollover**, then staged expansion Fri–Sat and stable watch through Sun 11-15. No generated user input, erased history, broad Failed-to-Ready transition or inferred native state may fill an evidence gap. Recover the capacity-exceeded cohort only with original identity and exact receipts; parent/ledger readiness remains an external prerequisite. Close #1348–#1351 only after their own verified release evidence, not after source merge.


### Merge-freeze update — Wed 2026-11-11 22:58 SGT

The lead lifted the develop merge freeze after retaining the exact `207be186c` release bytes. This supersedes earlier freeze status in this historical note. Segmentation still requires its own exact-head reviewer verdict, passed targeted local gates and explicit `MERGE #N (admin ok)` relay. Never merge over a failed job; for the known apt/DNS browser-install failure, rerun failed jobs first and record it. Use `--merge --admin --match-head-commit` at the reviewed head and leave protection settings as instructed. Monitor the resulting develop push CI and fix forward or revert a real regression within one hour. No segmentation merge is currently requested or ready.


## Pre-review closure ledger — cfce4f4b8, received Wed 23:01 SGT

Source: `notes/reviews/2026-11-11-segments-prereview.md` (four P1, two P2). These are explicit PR completion gates. **Writer and recovery activation remain closed until every P1 is fixed and qualified.** The freeze lift does not change this requirement. A newer source pin or disappearance of a warning is not a closure verdict.

| Finding | Owner / fix boundary | Current status and required closure evidence | Target checkpoint (SGT) |
| --- | --- | --- | --- |
| P1-1: v2 adoption strands ordinary writers | S4 | OPEN. Convert worker/control/lifecycle and every ordinary publication root through the shared transaction, preserving all domain fences. `a2e29f32a` adds exact keyed/lifecycle reconciliation but is not evidence that all v1-only mutation roots are converted. Test actual segmented child mutations, not just an empty child list, plus rollback/refusal and unknown outcomes. | S4 01:30 checkpoint; full caller gates by Thu 14:00 estimate. |
| P1-2: missing runtime grants and canonical carrier migration | S2 runtime inventory/composition, S1 schema, S5 exact codec | OPEN. Register gates, admissions and format-2 work in runtime authority; install the frozen witness/three nonunique discovery indexes through the canonical migrator; checksum exact schema sources. Test successful isolated-role operations, missing/foreign grants, real migrator restart/drift, and carrier insertion through that schema. Remove ad hoc carrier schema creation from qualification. Existing unique indexes must remain exact while nonunique discovery indexes are accepted only as specified. | S2 authority patch after current immutable gate batch; S1 schema estimate Thu 02:00, combined Mongo by 08:00–12:00. |
| P1-3: completed recovery loses pending attachment activation | S5 | OPEN. Add durable/fenced resumable activation after completion, including crash/unknown-reply/ack failure between completion and runtime acknowledgement. Terminal receipt lookup alone cannot finish native recovery. Reattach only the original retained process, with no input/settings replay. Test restart through the public service boundary and assert eventual exact acknowledgement. S2 wires only the resulting real constructors. | Service checkpoint Thu 06:00–10:00 estimate; pinned-runtime evidence 12:00–16:00. |
| P1-4: segmented response/transcript still fall back to lifetime data | S2 response; S3 transcript | OPEN. Replace long-turn/segmented source fallback with indexed bounded page reads; stream pages without materializing the complete response/event sequence. Test >10,128 entries and >4 MiB histories, one turn across several segments, cursor stability/revocation, page/source byte budgets, and unchanged-head polls. Legacy fast-path fallback cannot authorize segmented activation. | S2/S3 source Thu 10:00–12:00 estimate; scale/parity and API tests by18:00. |
| P2-5: restart replans unresolved keyed attempt | S4 | OPEN. Preserve the original attempted publication across a call/restart or establish positive KnownAbort before replanning. New `after_unknown` source at `a2e29f32a` must also cover the next public call with absent receipt and a later head; do not close based only on low-level reconciliation. Test root-level lost acknowledgement, absent receipt, restart and later-head race without a second attempted result. | Next S4 checkpoint; exact root regression by Thu14:00 estimate. |
| P2-6: paged child query accumulates unbounded memory | S5 | OPEN. Bound total child count and decoded/encoded bytes, or use a bounded visitor. Refuse before any mutation and preserve all fences. Test oversized Agent/Workspace child sets and verify unchanged durable state. | With S5 accepted-work checkpoint Thu06:00–10:00 estimate. |

S2 tracks closure links, source pins and actual test names/results in this ledger. Dedicated reviewer rechecks the exact combined head after fixes. No issue or finding is closed solely by this plan.

## S5 accepted-work constructor checkpoint — Wed 23:13 SGT

Published **`2ff67f6cbaddcb4ec5bea5b89e540ecf3c5403b6`** on
`tl/cma-log-segments/capacity-recovery`, with current develop
`4411f9b699f7dd632c0868ca92d206a0c9af6da5` normally merged.
Concrete constructors and exact shared registry/authority gaps are in
`notes/2026-11-11-1351-service-worker-handoff.md`. No shared root was edited.
S2 can consume `RecoveryService::new` and
`AgentWorker::run_capacity_recovery`. Native proof remains Unsupported.

P2-6 source budget fix is `a8b18866f`; P1-3 completed-activation protocol
is `bc130027c` plus production wiring `709c302b5`. Both findings remain
OPEN until executable qualification and review. P1-3 additionally needs
completed original-request scheduling, exact already-published registry detection
and restricted loss routing. No claim that the receipt-only model establishes
native recovery. Corrected CMA gate is queued with a blocking heavy-slot
allowance of 3600 seconds; actual Mongo and core restart tests remain pending.
Formatting/file-size checks and the full-history secret scan passed before
this note; no Rust/Mongo current-head pass is claimed.


## First complete combined Mongo batch — source 2cf7d3fe6

**182 passed, 5 failed, 187 selected contracts**, all five serial partitions attempted, final exit1. Partition results:37/1,36/2,36/1,36/1,37/0 (passed/failed). The other2,304 tests in the2,491-test binary were outside the union of this selection. Each owned container/volume cleanup completed before its successor; no cleanup125 occurred. Strict CMA remains39 dead_code/zero type errors on this source; affected adapter22pass/657skip. This is failure evidence, not review readiness.

| Owner | Failed contract | Exact observed failure |
| --- | --- | --- |
| S4 | writer_commit::persistence::tests::bootstrap::bootstrap_stages_multiple_fenced_batches_without_exposing_or_rewriting_source | writer_persist_tests.rs:32; publication domain error: record revision changed |
| S4 | writer_commit::persistence::tests::bootstrap::staged_bootstrap_is_published_only_against_the_same_recaptured_source | writer_persist_tests.rs:32; publication domain error: record revision changed |
| S4 | writer_commit::persistence::tests::keyed_caller::keyed_trait_adopts_legacy_only_after_durable_gate_admission | writer_keyed_mongo_tests.rs:133; Conflict |
| S4 | writer_commit::persistence::tests::uncertain_commit::unknown_commit_is_reconciled_from_original_receipt_after_later_head_advance | writer_unknown_mongo_tests.rs:96; expected Outcome::Recorded |
| S5 | agent_capacity_recovery_tests::records::capacity_carrier_cannot_change_original_request_or_native_target | records.rs:109; expected Conflict; actual variant was not printed, so accepting the write versus another refusal remains unproven |

Full log: `2026-11-11-segments-mongo-2cf7d3fe6.log`; owner handoff: `2026-11-11-segments-mongo-failures-2cf7d3fe6.md`, both0600. Passing subsets include bounded reader/capture/audit/retention, real additive migration restart/drift and existing response/approval windows. No timeout/assertion/cache bound was weakened. Next combined batch consumes S1 `f205616a4`, S3 `09715a0fa`, S4 `a2e29f32a`, S5 `2ff67f6cb` and develop `4411f9b69`; newer code must be measured before any failure is closed.


## Second combined candidate — 631d92042

Normally merged S1 `f205616a4`, S3 `09715a0fa`, S4 `a2e29f32a`, S5 `2ff67f6cb` and develop `4411f9b69`. Resolved only the expected shared migration checksum conflict (retain all new schema/index bytes) and transcript startup-config conflict (retain the existing strict process parser and production setter). S2 runtime authority/test checkpoint **`631d92042c76bf112e5909f4cf97efdafedf0f2a`** adds the missing gate, admission and format-2 carrier collections, three pure security regressions and an authenticated canonical-migrator/runtime-role regression. Backup/restore inventory is in `2026-11-11-segments-authority-backup.md` and its repository mirror. No live grants, schema, flags or runtime state changed.

The authenticated CI selector now includes the new migration/runtime contract; all **18 mocked authenticated-runner tests passed**. The candidate's strict CMA gate is queued under a fresh heavy-1 acquisition with3600s wait allowance. Current-head adapter, domain, unit, real-Mongo and authenticated-Mongo results remain pending. Do not carry the 182/5 result from the earlier candidate forward.

S1 has supplied canonical format-2 registration/nonunique-index comparison and retention source. S4 has supplied exact keyed/lifecycle receipt reconciliation. S5 has supplied accepted-work service/worker source, bounded child enumeration and post-completion activation retries. These source changes move their findings to awaiting combined executable evidence, not closed. S3's new transcript regression fixes still do not supply indexed segmented transcript pages; P1-4 remains open for both consumer owners.

S5 constructor consumption still needs an actual Layer2-authorized command/worker boundary in shared composition. Its new Cma RecoveryService takes a fresh Caller for every operation and exposes status/admit/run_one/discover, while the existing Layer1 route is read-only preflight. Discovery metadata must never be converted into a fabricated owner Caller. S2 will wire a protocol-neutral endpoint/worker interface; S5 retains domain authorization/accepted-work semantics and supplies its neutral contract. A provider-native confirmation producer is still absent; Unsupported must remain a refusal, not ordinary reconnect. No fake call or unused constructor registration is accepted as production wiring.


### Compile correction at 245f8e821

Strict CMA at631d92042 exited101 with **seven compile diagnostics in three causes**: duplicated `bounded_transcript_reads` field/initializer after the S3 merge, S5 worker test passing Arc<Fixture> instead of its existing Runtime wrapper, and S3's u64-versus-usize telemetry assertion. Shared integration correction **`245f8e821378d7038d74bee80877c3962edc2e3b`** fixes those three without changing behavior or weakening assertions. Formatting and source size pass at this source (**6,043 files**). Strict CMA rerun is queued; a zero-length/in-progress diagnostic file is not a pass. No adapter/Mongo counts are claimed for the unbuildable intermediate head.

The full pure recovery and authority filters now include the actual S5 worker/discovery/lineage tests in addition to codec, migration and operator tests; Layer2 recovery and Layer3 adapter gates remain separate lock acquisitions. The authenticated Mongo gate is separately owned and selected from the canonical authenticated runner, with no externally supplied URI accepted and no credential output. All local evidence files remain0600.


Correction after reading `layer2-core/src/clock.rs:68`: the previous claim that `seconds_from_rfc3339` accepts noncanonical forms was incorrect. This shared parser already requires exactly20-character UTC seconds, ASCII numeric components and valid calendar round-trip at245f8e821. S5 correctly reuses it. No parser implementation blocker remains. Recovery-specific offset/fraction/invalid-calendar and lexical-order negative tests are supplied at8514f4d38 and await execution with the next combined source.


S3 follow-up on09715a0fa: the new default `AgentLogStore::agent_log_transcript` contains a cfg(test)-only full-read/filter return while production returns None, followed by an unconditional Ok(None). Move the parity fixture behavior into the actual in-memory test-store implementation so the trait does not change semantics under cfg(test) or introduce unreachable test code. S3 owns that trait/test implementation. The real Mongo transcript regression must remain; this source review is not an executed strict Layer2 result.


## Combined strict-lint update — Wed 23:43 SGT

At immutable source `245f8e821378d7038d74bee80877c3962edc2e3b`, the strict CMA rerun has emitted **65 distinct dead_code diagnostics, zero type errors**. It is still waiting for the second compiler job to finish, so this is the observed diagnostic count, not a completed gate result. All 65 locations are the uncomposed S5 service/worker and its dependency graph (including TransactionScope::insert_one); S4's two reconciliation warnings are gone. This count increased because the concrete accepted-work implementation was added, not because a warning gate was relaxed. S2 owns shared composition; S5 owns the neutral authorized command/accepted-work interface. No dummy reachability, owner Caller fabricated from discovery metadata, or dead-code suppression is permitted.

**Registry interface accepted for S5:** `LiveAgentRegistry::recovery_attachment_is_active(&self, target: &LiveAgentTarget, authority_fence: u64) -> bool`. S2 will put it in a focused child module, checking published entry, exact Agent/conversation/generation/fence, current unfenced watermark, and live update task under one registry lock. It does not read or promote staged authority, publish, reconnect, or send native acknowledgements. S5 may code its AlreadyActivated branch against this signature now. Pin follows after the current immutable gate batch; tests cover exact live publication, staged-only, every identity/fence mismatch, fenced watermark, loss and finished task. The boolean is current process-local attachment presence, never authority to complete a database transition or proof after process restart. S5 must retain before/after fresh authorization and exact recovery receipt checks.

The complete P1-3 contract still requires safe completed-request lookup/dispatch after process restart and restricted connection-loss routing. This helper by itself does not close P1-3 or advertise native recovery support.


### Narrow S2 registry dependency published

S5 may normally merge **`origin/tl/cma-log-segments/recovery-registry` at `ad5641a7122e1b99417e2be7ce421b7a25d64f50`**, based on current develop `4c65d80e8`. This contains only the frozen `recovery_attachment_is_active` method, its shared module registration and three focused registry tests. Exact live publication, staged refusal, wrong identities/fences, fenced watermark, finished task, missing attachment and empty restarted registry are covered in source. `rustfmt` passed; executable tests await the combined gate, so this is a dependency pin and **not** a recovery readiness claim. S2's active test source remains unchanged at245f8e821; the pin was built through an isolated temporary git index without changing the worktree or its HEAD. Temporary index removed.


## S5 authorized neutral command pin — 58ffd3b61

Published `58ffd3b61` on `tl/cma-log-segments/capacity-recovery`, with
`origin/develop` normally merged. S2 can mount
`Arc<AgentWorker>::capacity_recovery_commands(Arc<RecoveryService>)` as
`Arc<dyn layer2_core::sandbox::agent_capacity_recovery::commands::CapacityRecoveryCommands>`.
The object supplies `status(&Caller, &AgentId, Option<&RecoveryRequest>)`,
`admit(&Caller, &RecoveryRequest)` and `resume(&Caller, &RecoveryRequest)`.
Status is now a Layer2 type. The actual worker/store implementation reauthorizes
all operations; no discovered owner ID may be converted to a Caller. HTTP adapters
must expose safe status/outcome DTOs, not serialize private native facts in RecoveryWork.
Discovery remains the existing bounded CMA service method for shared scheduling.
This is an executable implementation boundary, not a fake constructor registration.

The canonical format-2 schema dependency `8ba9d5521` is normally merged. Carrier
writes, discovery and completion/restart fixtures now use its installed validator
and indexes. Explicit corruption/obsolete-format fixtures alone bypass validation.
Canonical migration-ledger/authenticated-role qualification remains S1/S2-owned.

Timestamp correction: exact combined `245f8e821` and S5 both already contain the
strict shared parser at `layer2-core/src/clock.rs:68`, introduced by `38550a154`.
It requires exactly20 bytes, UTC Z, ASCII digits, bounded components and a valid
calendar round-trip. Offset/fraction/calendar rejection is not missing source.
`8514f4d38` adds recovery-specific negative codec/discovery/cursor tests and the
fractional-second lexical-order trap; it does not duplicate the parser.

The S5 carrier identity failure had dependent-row validation before immutable CAS
checks. Moving the latter before proposed-row encoding preserves Conflict for a
changed requested_at that also invalidates ready_at. The original exact assertion
is retained with actual-variant diagnostics. No refusal is weakened.

Executed core recovery at `5fa84974f`:46 passed,0 failed. Current S5 CMA tests are
queued under the existing shared lock, two jobs, one target. New source is not yet
Mongo/strict-lint certified. P1-3/P2-6 remain awaiting execution and exact-head review.
The accepted registry signature is understood; AlreadyActivated integration and
restricted loss/restart scheduling remain pending that shared pin. Native proof
remains typed Unsupported. No PR-ready or activation claim.


### Completed current-head pure gates — 245f8e821

Strict CMA completed with exit101: **65 distinct dead_code diagnostics, zero type errors** (binary65/test6; the six test-target diagnostics overlap the same65 locations). The recovery production-composition gap remains, so strict lint is red. The separately locked pure gate completed: **41 passed, 2,482 skipped**, including all three new runtime-authority success/refusal contracts, operator authority/parse contracts, migration-unit contracts and current recovery worker/codec/discovery units. No ignored Mongo contract is counted as a pass. The authenticated real-Mongo P1-2 gate is queued under its own blocking heavy-1 acquisition; adapter/domain and full combined Mongo gates remain pending for this source.

New published checkpoints for the next normal integration: S4 `e363f788f` includes ordinary segmented worker/control/lifecycle roots (`b73af4da9`) and a durable unresolved-keyed-attempt fence (`37a4a8509`); S5 `b414a4655` adds post-first-completion authority recheck and its fixture wrapper fix; S3 `29092c940` adds bounded usage projection and its Mongo contract. They remain unqualified until merged and tested. A source commit is not closure of P1-1/P2-5/P1-3/P1-4.


### Production-composition handoff to S5 — exact remaining interface

The existing `cma::store::agent_capacity_recovery::RecoveryService` cannot be a Layer1 dependency. S5 owns a protocol-neutral, object-safe Layer2 command boundary and the matching recovery REST child; S2 owns construction/registration and the config/worker root. Freeze the following request-scoped interface (reuse the existing recovery types; move the current status value to Layer2 instead of duplicating it):

```rust
#[async_trait::async_trait]
pub trait CapacityRecoveryCommands: Send + Sync {
    async fn status(
        &self, caller: &Caller, agent: &AgentId, request: Option<&RecoveryRequest>,
    ) -> Result<RecoveryStatus, RecoveryError>;
    async fn admit(
        &self, caller: &Caller, request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
    async fn resume(
        &self, caller: &Caller, request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
}
```

`resume` uses the actual restricted runtime worker and original request; it does not call admission or require admission enabled for already-accepted work. The route supplies a currently authenticated Caller, never one synthesized from Agent ownership or discovery. All mutations retain the existing service's per-operation reauthorization and exact receipt checks. S2 will supply the small Cma composition adapter owning `Arc<RecoveryService>` and `Arc<AgentWorker>` once S5 registers this port and its authenticated route/negative tests. This signature freezes a composition dependency, not a replacement recovery state machine.

**Separate S5 blocker:** background restart/connection-loss continuation still needs an explicit accepted-work authority contract, because the current ResourceAccessService has no accepted-background authorization entry. Discovery returns hints only. S5 must supply a receipt-bound, freshly revalidated accepted-work port (including current personal ownership/policy and parent intent) before S2 starts a dispatcher. Do not construct a personal Caller, retain an expired request Caller, or execute work from the metadata-only discovery page. Request-scoped wiring alone does not satisfy restart recovery; this is still a P1-3/strict-ready blocker. No authorization policy or production flag changes are made by this handoff.


S5's requested neutral command boundary is now delivered at **58ffd3b61**. S2 will consume the existing `AgentWorker::capacity_recovery_commands` constructor instead of introducing a duplicate composition adapter. S5 retains its REST child/DTO/authorization tests; S2 retains root mounting and default-off config. Background authority and restricted connection-loss dispatch are still separate open dependencies, not solved by this request-scoped port.


S2/S5 route registration contract: S5 exports `layer1_public::rest::agent_capacity_recovery_commands_router(commands: Arc<dyn CapacityRecoveryCommands>) -> axum::Router` from its existing recovery REST child. The original dry-run router remains read-only. S2 mounts the command router under existing human authentication/rate limits, constructs `RecoveryService` with current resource access and the already assembled AgentWorker, and wires `CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED=false` through strict process config plus infra declaration. The process switch controls new admission only; both durable admission gates still apply. Status and exact accepted-work continuation remain available with admission off. S5 owns request parsing, safe status/outcome DTOs (no private RecoveryWork serialization), and positive/negative HTTP authorization tests. S2 owns config/default/refusal tests and root registration. Background discovery is not made reachable by a dummy call; its accepted-work authority port remains required.


### S5 REST and restart-discovery checkpoint — 1573f9100

Published `1573f9100` on the S5 branch. S2 can consume the existing neutral adapter
and re-export/mount `agent_capacity_recovery_commands_router` from the recovery
REST child. HTTP tests cover original identity, no input fields, bounded body,
no private-native facts, anonymous/foreign/scoped credentials, disabled admission
with accepted resume, Unknown/KnownAbort and failed activation. The route applies
independent credential expiry to the awaited command. Shared config/root ownership
is unchanged. The route gate is pending, not green.

`RecoveryService::discover_agent(&AgentId)` now implements the bounded restart
lookup proposal through the frozen by-Agent index, including Completed requests.
At most32 candidates/128KiB; limit33 detects overflow; simple collation and1000ms
server bound. It grants no execution authority and no ordinary-reconnect fallback.
A real-Mongo regression covers completed versus active streams,32/33 boundary,
foreign-Agent isolation, missing index and no authority from candidate rows.

Current CMA gate is still blocking for an existing heavy slot with3600s allowance.
Only the earlier46 core tests and light gates have passed. Explicit accepted-work
background authority and the narrow shared registry pin remain pending; native
proof remains Unsupported as instructed. Repository handoff:
`docs/notes/2026-11-11-capacity-recovery-service.md`.


S3 usage handoff29092c940: S2 takes the pending strict process registration for `CHRONO_AGENT_USAGE_BOUNDED_READS_ENABLED` (FeatureFlags, production store setter, composition and default-false infra declaration), replacing the current per-read environment fallback and test-only setter. S3 retains query/projection correctness and the missing segmented/current-state implementation. The filtered legacy query still accumulates usage rows across history, so it is not evidence that P1-4 or all bounded consumer work is complete. Its Mongo parity tests remain in the combined selection.


At Thu00:03 SGT the still-unacquired authenticated gate wait at245f8e821 was cancelled (exit143, zero log bytes, verified no child/fixture). This is a reschedule to consume the newly published S5 route/constructor and S4 writer fixes before the next gate, not a test result or a lock timeout. No lock file was changed. Current-head real-Mongo qualification remains pending; the next combined source runs fresh separately acquired gates with the full3600-second wait allowance.


### S5 accepted-background authority checkpoint — d84eb3977

Published `d84eb3977` on the S5 branch. `AgentWorker::run_accepted_capacity_recovery(&RecoveryService, &RecoveryRequest)`
now supplies the real background entry; its service method is `run_accepted`.
It never constructs or retains a Caller. Each claim/complete/retry/reconcile/activation
freshly verifies a private AcceptedAuthority against the exact carrier, immutable
Publication, captured source and current Agent result revision/digest. The latter
binds original ownership as well as native/input state; later revisions refuse
conservatively. Current ResourceAccessSnapshot scope must be Person with the same
owner. The following mutation transaction compares policy, shares, full lineage
and parent snapshots again. This is accepted-work settlement only; admission still
requires a currently authenticated Caller and both gates. Current parent intent
continues to be enforced by the state machine and completed-activation checks.

Mongo restart tests now call the background entry without a retained request Caller
and include no-receipt, changed original request, later Agent revision and current
organization-owner refusals before native IO. These are pending execution. Shared
scheduling/loss routing can consume this entry after review; no dummy discovery
call is needed to silence dead code. Please review this authority boundary explicitly.

`8a9284dda` also adds `CompletedAttachment::{AlreadyActivated, Reattached}` to the
runtime port and rechecks store authority around either outcome. The model covers
repeat same-process activation with no second native reconnect or acknowledgement.
The production runtime still needs the accepted exact registry helper pin to return
AlreadyActivated; native proof remains Unsupported. No forged confirmation is used.

The previous queued gate acquired a slot while source edits were in progress and
correctly refused to start heavy work; no pass is claimed. Clean committed source
is now queued for the CMA gate with a fresh blocking 3600-second allowance. The
gate also rejects any source change during execution. One reusable target remains;
no shared locks or other worktrees were modified.


### S5 develop integration — bcbcc43da

S5 now normally includes develop `8d59f6154` at published `bcbcc43da`.
The merge retained both feature flags and both Mongo selector sets. The retained
runtime recovery/reopen additions intersect the owned unpublished-recovery
transaction: load/load_retained/load_reopen now share inspection while scoped
unpublished proofs remain distinct. A generation maximum cannot establish the
new retained-reopen history digest/semantic proof, so that segmented path refuses
until bounded evidence is implemented. Add this new caller to the integration
coverage matrix; it is not a successful segmented consumer or activation evidence.
The new roots in sandbox_unpublished_recovery.rs still need S2's scoped dispatch
registration along with the other pending roots. No history is silently skipped.

Formatting/source-size passed at this head (6,082 files); canonical full candidate
history scan passed (3,785 commits, zero findings). Mock Mongo runner tests are
still executing; real Rust/Mongo gates remain queued. The earlier all-remote-ref
scan was broader than the canonical candidate scan and had unrelated findings;
only the canonical script result is the repository gate evidence. No allowlist,
scanner setting or test assertion was weakened.


## Thu 00:18 combined-source checkpoint and new compatibility gate

Normal merges consumed the current S1 schema/index qualification and S5 accepted-background authority source. S2 production root commits are `5a4ec596c` (strict usage configuration), `100299dcb` (authenticated recovery command composition), and `21333475c` (transaction-bound retained assessment with bounded Agent/Workspace evidence). The registry dependency remains the separately published `ad5641a7122e1b99417e2be7ce421b7a25d64f50`; the operator dependency remains `b11107979231ec0da5683d8719722c8b4dfea15e`. S1 has consumed the latter and the frozen format-2 carrier pin. No v1 carrier is registered.

The #1652 develop merge adds a **new P1-1 compatibility dependency**: retained reopen needs its original history digest, semantic proof and capacity proof. A certified maximum generation is insufficient. The combined source retains all legacy checks, uses the exclusive TransactionScope for segmented read-only retained assessment, and refuses segmented retained reopen. S2 owns root dispatch/qualification; S5 owns bounded captured recovery proof; the retained-reopen owner is cma-admission-scan. The lead should route that specific bounded-history proof dependency. This is an activation blocker, not a regression waived by a flag. New real-Mongo root tests cover actual gated adoption, read-only assessment, corrupt-envelope refusal and the 513-Workspace limit; execution pending.

S2's scoped inspection now reuses the same bounded captured Agent and Workspace sets instead of repeating unbounded queries after capture. S5's 512-child/8MiB limits therefore survive this shared root. The legacy root remains unchanged. This source contribution advances P2-6; it does not establish combined gate success.

**S5 next narrow checkpoint requested:** consume the published registry pin in `RestrictedRecoveryRuntime::reattach_completed` to return `AlreadyActivated` only for the exact current published target/fence, with the service's fresh pre/post authority checks retained. `bcbcc43da` still unconditionally reconnects. S2 takes background scheduler/shared loss routing using `run_accepted_capacity_recovery` and `discover_agent`; discovery itself never grants execution. The new private accepted authority boundary from `d84eb3977` must be explicitly reviewed. Native proof still returns Unsupported and requires its real producer/tests before activation.

PR-ready evidence additions: retained reopen success on a valid segmented source (or a reviewed compatible alternative before any writer activation), real retained GET proof under the same exclusive transaction, authenticated runtime Mongo role success/refusal, and actual post-completion attachment activation through restart/loss. PR shape remains one combined default-off PR. Earliest strict-green estimate remains conditional Thu 12:00–16:00; full PR-ready Thu 18:00–22:00. Source readiness for bounded response/transcript pages remains Thu 10:00–12:00, with scale/parity evidence by 18:00. These are forecasts, not completed gates; refresh by 08:00 using measured results.

Administrative verification: #1687 was already merged at `cd3babea89a1a1a0de00e19d7807b6c073f69c1d` on Wed19:35SGT, from unchanged reviewed head `41334d7c7c4c183110de9d2b67b774c7ed71a808`. All25 PR jobs are successful. The issue's merge comment already exists; no duplicate merge or branch-protection write was needed. #1421 is transferred to cma-admission-scan per lead relay. Segmentation integration stays the priority.


Latest finding evidence ledger at `21333475c`: P1-1 ordinary roots implemented but Mongo qualification and retained-reopen compatibility pending; P1-2 grants/schema implemented with3pure security tests passed on earlier source, actual authenticated-role gate pending; P1-3 service/commands/restart protocol implemented but native producer, registry consumption and shared scheduler/loss routing pending; P1-4 indexed response/transcript consumers still incomplete; P2-5 durable attempt fence implemented, root restart/Mongo gate pending; P2-6 S5total-child budgets plus S2captured-root reuse implemented, oversized-child real-Mongo gate pending. **None is closed by source presence.** All four P1s still block writer/recovery activation.

Light-gate results for the current combined source: formatting passed; source-size passed for6,116files; all31Mongo-runner harness tests passed, zero failed/skipped. The strict CMA job remains queued; no Rust/Mongo source gate has yet acquired the slot. The #1687 merge push run43013150034 was cancelled by a later develop push; current develop run43055008152 has no finished failures at the00:21check. Release still requires the exact final batch's fully green artifact.


S2/S5 qualification case to retain: completed recovery followed by ordinary output or command progress advances the Agent revision. The accepted-background capability currently refuses a later revision deliberately; by-Agent discovery still returns the old Completed carrier. The shared routing contract must prove which operation remains eligible without treating discovery as authority or permanently diverting a healthy progressed Agent into a stale completion. Test crash-before-first-ack separately from later loss-after-progress, including current parent/policy changes. Exact completed-result refusal is safe, but is not evidence of availability after subsequent work. S5 owns the service decision/port, S2 owns the shared routing; native confirmation remains mandatory.


### S5 exact published-attachment checkpoint — fc09a0eac

Published `fc09a0eac` on the S5 branch with current develop `8d59f6154` normally
merged. Consumed registry pin `ad5641a71`; RestrictedRecoveryRuntime now returns
AlreadyActivated only from `recovery_attachment_is_active` at the original Agent,
conversation, generation and committed writer fence. Staged/fenced/finished or
mismatched entries do not qualify. Both runtime outcomes retain fresh pre/post
store receipt/authority checks. No native confirmation or ACK is manufactured;
no ordinary reconnect fallback exists. Native proof remains Unsupported.

The CMA diagnostic build completed with21 passed/17 ignored/zero failures and no
type errors, but the registry dependency merge overlapped its start at45fc44e33;
the source guard invalidated the run, so this is NOT an exact-head pass. A fresh
clean-source capacity filter (including the three shared registry tests) is queued
under the existing heavy-slot locks. No source mutation is planned during this
acquisition/run. Source-level source/consumer gaps remain with S2/S4/the retained
reopen owner as recorded; no P1 closure or PR-ready claim.


S2 source-review correction queued for the next checkpoint: the new retained-assessment regression initially reused the generic worker fixture whose first rows are conversation-open/status. Segmented adoption correctly requires the original Created row. A corrected fixture is prepared through the actual legacy Created append plus worker publication; it will be applied after the immutable-head lint gate. Do not interpret a fixture refusal as valid-history unavailability or relax adoption integrity to admit it. No actual Mongo test has yet run on this source.
