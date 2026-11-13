# Segmentation coordination history, part 2

Historical source-specific observations, retained from the shared working note.
The current decision and ownership ledger is [the integration plan](2026-11-11-segments-strict-lint-assignments.md).
Later entries and explicit lead allocations supersede earlier assignments and estimates.


**Separate S5 blocker:** background restart/connection-loss continuation still needs an explicit accepted-work authority contract, because the current ResourceAccessService has no accepted-background authorization entry. Discovery returns hints only. S5 must supply a receipt-bound, freshly revalidated accepted-work port (including current personal ownership/policy and parent intent) before S2 starts a dispatcher. Do not construct a personal Caller, retain an expired request Caller, or execute work from the metadata-only discovery page. Request-scoped wiring alone does not satisfy restart recovery; this is still a P1-3/strict-ready blocker. No authorization policy or production flag changes are made by this handoff.

S5's requested neutral command boundary is now delivered at **58ffd3b61**. S2 will consume the existing `AgentWorker::capacity_recovery_commands` constructor instead of introducing a duplicate composition adapter. S5 retains its REST child/DTO/authorization tests; S2 retains root mounting and default-off config. Background authority and restricted connection-loss dispatch are still separate open dependencies, not solved by this request-scoped port.

S2/S5 route registration contract: S5 exports `layer1_public::rest::agent_capacity_recovery_commands_router(commands: Arc<dyn CapacityRecoveryCommands>) -> axum::Router` from its existing recovery REST child. The original dry-run router remains read-only. S2 mounts the command router under existing human authentication/rate limits, constructs `RecoveryService` with current resource access and the already assembled AgentWorker, and wires `CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED=false` through strict process config plus infra declaration. The process switch controls new admission only; both durable admission gates still apply. Status and exact accepted-work continuation remain available with admission off. S5 owns request parsing, safe status/outcome DTOs (no private RecoveryWork serialization), and positive/negative HTTP authorization tests. S2 owns config/default/refusal tests and root registration. Background discovery is not made reachable by a dummy call; its accepted-work authority port remains required.

### S5 REST and restart-discovery checkpoint — 1573f9100

Published `1573f9100` on the S5 branch. S2 can consume the existing neutral adapter and re-export/mount `agent_capacity_recovery_commands_router` from the recovery REST child. HTTP tests cover original identity, no input fields, bounded body, no private-native facts, anonymous/foreign/scoped credentials, disabled admission with accepted resume, Unknown/KnownAbort and failed activation. The route applies independent credential expiry to the awaited command. Shared config/root ownership is unchanged. The route gate is pending, not green.

`RecoveryService::discover_agent(&AgentId)` now implements the bounded restart lookup proposal through the frozen by-Agent index, including Completed requests. At most32 candidates/128KiB; limit33 detects overflow; simple collation and1000ms server bound. It grants no execution authority and no ordinary-reconnect fallback. A real-Mongo regression covers completed versus active streams,32/33 boundary, foreign-Agent isolation, missing index and no authority from candidate rows.

Current CMA gate is still blocking for an existing heavy slot with3600s allowance. Only the earlier46 core tests and light gates have passed. Explicit accepted-work background authority and the narrow shared registry pin remain pending; native proof remains Unsupported as instructed. Repository handoff:  `docs/notes/2026-11-11-capacity-recovery-service.md`.

S3 usage handoff29092c940: S2 takes the pending strict process registration for `CHRONO_AGENT_USAGE_BOUNDED_READS_ENABLED` (FeatureFlags, production store setter, composition and default-false infra declaration), replacing the current per-read environment fallback and test-only setter. S3 retains query/projection correctness and the missing segmented/current-state implementation. The filtered legacy query still accumulates usage rows across history, so it is not evidence that P1-4 or all bounded consumer work is complete. Its Mongo parity tests remain in the combined selection.

At Thu00:03 SGT the still-unacquired authenticated gate wait at245f8e821 was cancelled (exit143, zero log bytes, verified no child/fixture). This is a reschedule to consume the newly published S5 route/constructor and S4 writer fixes before the next gate, not a test result or a lock timeout. No lock file was changed. Current-head real-Mongo qualification remains pending; the next combined source runs fresh separately acquired gates with the full3600-second wait allowance.

### S5 accepted-background authority checkpoint — d84eb3977

Published `d84eb3977` on the S5 branch. `AgentWorker::run_accepted_capacity_recovery(&RecoveryService, &RecoveryRequest)` now supplies the real background entry; its service method is `run_accepted`. It never constructs or retains a Caller. Each claim/complete/retry/reconcile/activation freshly verifies a private AcceptedAuthority against the exact carrier, immutable Publication, captured source and current Agent result revision/digest. The latter binds original ownership as well as native/input state; later revisions refuse conservatively. Current ResourceAccessSnapshot scope must be Person with the same owner. The following mutation transaction compares policy, shares, full lineage and parent snapshots again. This is accepted-work settlement only; admission still requires a currently authenticated Caller and both gates. Current parent intent continues to be enforced by the state machine and completed-activation checks.

Mongo restart tests now call the background entry without a retained request Caller and include no-receipt, changed original request, later Agent revision and current organization-owner refusals before native IO. These are pending execution. Shared scheduling/loss routing can consume this entry after review; no dummy discovery call is needed to silence dead code. Please review this authority boundary explicitly.

`8a9284dda` also adds `CompletedAttachment::{AlreadyActivated, Reattached}` to the runtime port and rechecks store authority around either outcome. The model covers repeat same-process activation with no second native reconnect or acknowledgement. The production runtime still needs the accepted exact registry helper pin to return AlreadyActivated; native proof remains Unsupported. No forged confirmation is used.

The previous queued gate acquired a slot while source edits were in progress and correctly refused to start heavy work; no pass is claimed. Clean committed source is now queued for the CMA gate with a fresh blocking 3600-second allowance. The gate also rejects any source change during execution. One reusable target remains; no shared locks or other worktrees were modified.

### S5 develop integration — bcbcc43da

S5 now normally includes develop `8d59f6154` at published `bcbcc43da`. The merge retained both feature flags and both Mongo selector sets. The retained runtime recovery/reopen additions intersect the owned unpublished-recovery transaction: load/load_retained/load_reopen now share inspection while scoped unpublished proofs remain distinct. A generation maximum cannot establish the new retained-reopen history digest/semantic proof, so that segmented path refuses until bounded evidence is implemented. Add this new caller to the integration coverage matrix; it is not a successful segmented consumer or activation evidence. The new roots in sandbox_unpublished_recovery.rs still need S2's scoped dispatch registration along with the other pending roots. No history is silently skipped.

Formatting/source-size passed at this head (6,082 files); canonical full candidate history scan passed (3,785 commits, zero findings). Mock Mongo runner tests are still executing; real Rust/Mongo gates remain queued. The earlier all-remote-ref scan was broader than the canonical candidate scan and had unrelated findings; only the canonical script result is the repository gate evidence. No allowlist, scanner setting or test assertion was weakened.

## Thu 00:18 combined-source checkpoint and new compatibility gate

Normal merges consumed the current S1 schema/index qualification and S5 accepted-background authority source. S2 production root commits are `5a4ec596c` (strict usage configuration), `100299dcb` (authenticated recovery command composition), and `21333475c` (transaction-bound retained assessment with bounded Agent/Workspace evidence). The registry dependency remains the separately published `ad5641a7122e1b99417e2be7ce421b7a25d64f50`; the operator dependency remains `b11107979231ec0da5683d8719722c8b4dfea15e`. S1 has consumed the latter and the frozen format-2 carrier pin. No v1 carrier is registered.

The #1652 develop merge adds a **new P1-1 compatibility dependency**: retained reopen needs its original history digest, semantic proof and capacity proof. A certified maximum generation is insufficient. The combined source retains all legacy checks, uses the exclusive TransactionScope for segmented read-only retained assessment, and refuses segmented retained reopen. S2 owns root dispatch/qualification; S5 owns bounded captured recovery proof; the retained-reopen owner is cma-admission-scan. The lead should route that specific bounded-history proof dependency. This is an activation blocker, not a regression waived by a flag. New real-Mongo root tests cover actual gated adoption, read-only assessment, corrupt-envelope refusal and the 513-Workspace limit; execution pending.

S2's scoped inspection now reuses the same bounded captured Agent and Workspace sets instead of repeating unbounded queries after capture. S5's 512-child/8MiB limits therefore survive this shared root. The legacy root remains unchanged. This source contribution advances P2-6; it does not establish combined gate success.

**S5 registry consumer now published at `fc09a0eac16a2bed0a478668d1dc4c1bb19f573c` (next normal integration after the fixed-head gate):** consumes the published registry pin in `RestrictedRecoveryRuntime::reattach_completed` to return `AlreadyActivated` only for the exact current published target/fence, with the service's fresh pre/post authority checks retained. The fixed gate head still contains `bcbcc43da`, which unconditionally reconnects; no current gate covers the later pin. S2 takes background scheduler/shared loss routing using `run_accepted_capacity_recovery` and `discover_agent`; discovery itself never grants execution. The new private accepted authority boundary from `d84eb3977` must be explicitly reviewed. Native proof still returns Unsupported and requires its real producer/tests before activation.

PR-ready evidence additions: retained reopen success on a valid segmented source (or a reviewed compatible alternative before any writer activation), real retained GET proof under the same exclusive transaction, authenticated runtime Mongo role success/refusal, and actual post-completion attachment activation through restart/loss. PR shape remains one combined default-off PR. Earliest strict-green estimate remains conditional Thu 12:00–16:00; full PR-ready Thu 18:00–22:00. Source readiness for bounded response/transcript pages remains Thu 10:00–12:00, with scale/parity evidence by 18:00. These are forecasts, not completed gates; refresh by 08:00 using measured results.

Administrative verification: #1687 was already merged at `cd3babea89a1a1a0de00e19d7807b6c073f69c1d` on Wed19:35SGT, from unchanged reviewed head `41334d7c7c4c183110de9d2b67b774c7ed71a808`. All25 PR jobs are successful. The issue's merge comment already exists; no duplicate merge or branch-protection write was needed. #1421 is transferred to cma-admission-scan per lead relay. Segmentation integration stays the priority.

Latest finding evidence ledger at `21333475c`: P1-1 ordinary roots implemented but Mongo qualification and retained-reopen compatibility pending; P1-2 grants/schema implemented with3pure security tests passed on earlier source, actual authenticated-role gate pending; P1-3 service/commands/restart protocol implemented but native producer, registry consumption and shared scheduler/loss routing pending; P1-4 indexed response/transcript consumers still incomplete; P2-5 durable attempt fence implemented, root restart/Mongo gate pending; P2-6 S5total-child budgets plus S2captured-root reuse implemented, oversized-child real-Mongo gate pending. **None is closed by source presence.** All four P1s still block writer/recovery activation.

Light-gate results for the current combined source: formatting passed; source-size passed for6,116files; all31Mongo-runner harness tests passed, zero failed/skipped. The strict CMA job remains queued; no Rust/Mongo source gate has yet acquired the slot. The #1687 merge push run43013150034 was cancelled by a later develop push; current develop run43055008152 has no finished failures at the00:21check. Release still requires the exact final batch's fully green artifact.

S2/S5 qualification case to retain: completed recovery followed by ordinary output or command progress advances the Agent revision. The accepted-background capability currently refuses a later revision deliberately; by-Agent discovery still returns the old Completed carrier. The shared routing contract must prove which operation remains eligible without treating discovery as authority or permanently diverting a healthy progressed Agent into a stale completion. Test crash-before-first-ack separately from later loss-after-progress, including current parent/policy changes. Exact completed-result refusal is safe, but is not evidence of availability after subsequent work. S5 owns the service decision/port, S2 owns the shared routing; native confirmation remains mandatory.

### S5 exact published-attachment checkpoint — fc09a0eac

Published `fc09a0eac` on the S5 branch with current develop `8d59f6154` normally merged. Consumed registry pin `ad5641a71`; RestrictedRecoveryRuntime now returns AlreadyActivated only from `recovery_attachment_is_active` at the original Agent, conversation, generation and committed writer fence. Staged/fenced/finished or mismatched entries do not qualify. Both runtime outcomes retain fresh pre/post store receipt/authority checks. No native confirmation or ACK is manufactured; no ordinary reconnect fallback exists. Native proof remains Unsupported.

The CMA diagnostic build completed with21 passed/17 ignored/zero failures and no type errors, but the registry dependency merge overlapped its start at45fc44e33; the source guard invalidated the run, so this is NOT an exact-head pass. A fresh clean-source capacity filter (including the three shared registry tests) is queued under the existing heavy-slot locks. No source mutation is planned during this acquisition/run. Source-level source/consumer gaps remain with S2/S4/the retained reopen owner as recorded; no P1 closure or PR-ready claim.

S2 source-review correction queued for the next checkpoint: the new retained-assessment regression initially reused the generic worker fixture whose first rows are conversation-open/status. Segmented adoption correctly requires the original Created row. A corrected fixture is prepared through the actual legacy Created append plus worker publication; it will be applied after the immutable-head lint gate. Do not interpret a fixture refusal as valid-history unavailability or relax adoption integrity to admit it. No actual Mongo test has yet run on this source.

### P1-4 S2 indexed-reader ownership boundary

S2 owns new `store/segment_response/indexed*` children, neutral response-page ports under `layer2-core/src/sandbox/store/agent_response_log*`, their authorized core read-service children, and public Responses projection/page children. S2 will reuse S1's captured snapshot and point proofs plus S4's immutable numeric output-change index; it will not reconstruct a whole prefix or invent compacted log entries. S1 retains the shared capture implementation; S4 retains `projection.rs` and its validator, as in the ownership manifest. The minimal shared exposure needed is `CapturedLog::validate_snapshot` and `projection::{evidence,read_evidence,EvidenceRecord}` at `pub(in crate::store)` visibility (same signatures, no semantic change); S2 can integrate that visibility-only delta alongside the actual caller, avoiding an unused standalone stub. No new writer format or index shape is required by this exposure.

Required response parity includes empty/nonempty messages, first-terminal tool output versus later current tool output, historical models, queued/deferred command failures, gaps and usage after turn completion, cross-segment long turns, exact replay sequence numbers and current authority on every emitted page. Public terminal frames necessarily contain their documented response output; bounded source work must not be implemented by silently truncating that output. Current source has deferred headers and unchanged-head suppression, but those alone do not close P1-4.

**S5 measured discovery delta still required:** S1 at `853b6cca7` measured64keys/documents for page2of32 with the factored predicate still present in integrated `agent_capacity_recovery/discovery.rs::filter`. The passing S1 contract repeats format/phase equality and the time cutoff in both `$or` arms; at1/100/1000terminal/future-history rows it examines only returned keys and zero documents. S5 owns the small production-predicate correction and actual-consumer regression; S2 will consume the pin normally. This is measured missing production wiring within P1-2, not an inference from an explain plan alone. Do not substitute the synthetic index test for the real consumer gate.


**Reader-floor configuration gate (S2/S3):** after any Agent is adopted, closing writer/recovery admission must leave the v2 readers available. Current response-reader composition is optional behind the legacy optimization flag; its absence still leads to the lifetime-prefix path. Final indexed consumers must either always dispatch v2 while leaving legacy optimization default-off, or declare and qualify the minimum reader configuration retained during rollback. Disabling a required v2 reader is not an admissible kill switch. Test flag closure on retained adopted data, with current source/API output parity; this is part of P1-4/#1327, not satisfied by default-off deployment alone.


### S5 measured-discovery consumer correction — 706f5a7d7

Published `706f5a7d757eefc5c4a760ae997584471c48dd01` on the S5 branch, with current develop normally included. Commit `64a4d496d` consumes S1's qualified distributed keyset predicate in the actual discovery service. Each OR arm includes the format/phase index prefix and due-time cutoff. A real Mongo profiler regression calls both service streams across three pages at 1/100/1000 terminal/future rows; it requires bounded examined keys/documents, response bytes, explicit limit/batch/maxTimeMS, no COLLSCAN or blocking sort, and complete ordered results. Execution is pending; S1's synthetic explain result is not being claimed as the production consumer result.

The discovery fixture now uses the carrier clock explicitly. The restart fixture now appends the original Created event through the legacy store before worker publication, so adoption receives a valid original history. Assertions and admission validation were not weakened. The previous queued gate was cancelled before acquisition; no test ran at fc09a0eac. A clean-source CMA capacity gate is queued at 706f5a7d7. Source remains fixed during that run. Formatting and canonical secret scan passed on this head (3790 commits, zero findings); file-size and mocked-runner checks are still executing. No P1/P2 closure, native recovery qualification or PR-ready claim.

P1-3 later-progress boundary remains explicit: original completion authority requires the exact result Agent revision/digest, while ordinary progress changes that revision and can change active-turn/input facts. A later revision or absence of a discovery candidate cannot authorize ordinary reconnect. S2's shared loss routing needs a positive, bounded ordinary-publication/activation proof plus independently current parent/policy authority. S5 can expose a typed supersession decision when that proof is specified; no relaxed old-receipt permission or invented Caller is used. Native confirmation remains typed Unsupported per the lead's instruction; the deferred native producer is not part of this checkpoint.


The next full Mongo batch will use `2026-11-12-segments-mongo-separate-slots.py`: compile/list in one heavy-1 acquisition, then acquire/release separately for every serial partition of at most40selected contracts. The existing one-owned-fixture cleanup and full-selection policy are preserved. Cleanup uncertainty stops further allocation. Every acquisition retains the full3600-second blocking allowance, at most2Cargo jobs and one target. This replaces the earlier coordinator's single lock across all partitions; no actual batch has run through the new coordinator yet.

The canonical configuration-doc gate currently fails for four newly registered switches (capacity recovery, parent pacing, transcript and usage); its4parser tests pass. S2 has prepared the focused operations reference and canonical checker registration for the next source checkpoint. No configuration gate pass is claimed until that delta is applied and checked.


**P1-1 actual command roots still require conversion (S4):** at `21333475c`, `store/agent_commands/reserve.rs:154` applies `document_schema_version:1` to the Agent admission fence; adopted v2 Agents cannot match. Its contextual-answer branch at190–246 also remains on the legacy trusted-summary/full-audit path. `agent_commands/targeted_control/fences.rs:85` and `queued_cancellation/fences.rs:77` apply the same v1 Agent fence; targeted-control apply and command reconciliation still contain complete-log reads. Convert the Agent predicate with exact captured-envelope authority while retaining v1 for unaffected parent/command records; do not mechanically replace every schema constant. Exercise real reserve, targeted control, queued cancellation and restart/reconciliation through public store roots on adopted data, with foreign/stale refusal and unknown outcomes. This extends the existing P1-1 caller coverage matrix, not its severity. The initial source map missed these ordinary roots; the writer/control/lifecycle checkpoint is not yet exhaustive.


### S2 response evidence boundary — Thu 00:53 SGT

Source review at `21333475c` identifies two missing compatibility facts for P1-4. `responses/model.rs::for_turn` returns the original profile model for model-less old turns only if no earlier UpdateSettings operation occurred; otherwise it returns `unknown`. Current per-turn facts retain only `model: Option<String>` and cannot distinguish those cases. `responses/v2/projection.rs` also observes legacy Usage rows through the next TurnStarted, including after completion; current per-turn facts record only UsageUpdated. Neither today's aggregate model nor lifetime totals is a valid substitute. Existing response output-change totals already distinguish empty-message event counts and immutable first-terminal tool output from later current output.

S2 requests a narrow S4 projection addition, consumed by the S1 audit/bootstrap: keep a scalar `settings_changed` source fact, captured at each turn as `legacy_model_known`; keep the latest response-interval usage reference/tokens through the next turn (both Usage and matching UsageUpdated, including after terminal); preserve the original terminal and adjacent pre-write-failure source references. These must be folded by the existing shared publication/bootstrap paths, with exact source sequences, rather than inferred in the reader. S4 owns `agent_log_projection*` and projection persistence; S1 owns audit parity. S2 will own the neutral indexed response page port, Mongo evidence/point reader and public projection, and will not edit the S4 fold while S4 is wiring ordinary writers. The lead should relay this dependency with the already recorded command-root omissions.

The response page contract remains a coherent captured Agent/turn/output-index observation, immutable ordinal cursor and bounded evidence/source points, with fresh authority before every release. The existing 4MiB result contract must distinguish result-size refusal from history-size refusal; pagination cannot silently truncate a terminal response. Empty pages, missing indexed rows, stale proof, unknown formats and corruption cannot authorize legacy lifetime fallback on v2. Actual long-turn and multi-segment golden replay, current tool output versus first-terminal replay output, settings history, post-terminal usage and revocation/backpressure are qualification gates.

The shared build wait has not yet acquired heavy-1; the fixed source and HEAD remain unchanged. No additional Rust/Mongo pass is claimed. The #1687 GitHub state was independently re-read at Thu 00:49 SGT: merged from the reviewed head, all 25 PR checks successful, merge commit `cd3babea89a1a1a0de00e19d7807b6c073f69c1d`. No merge or protection mutation was needed.


Next normal integration pins, fetched Thu 00:55 SGT: S1 `b680cf40f` adds reader/audit CI selection; S3 `f4494faaa` includes `f1070bee1` removing the unproven in-memory usage default; S4 `5fdfae281` includes `695ca2617` correcting original-transaction retry fixtures without releasing the caller's unresolved claim; S5 `706f5a7d7` adds exact registry consumption, distributed keyset predicates and the original-Created restart fixture. Current develop is still `8d59f6154`, already included. These pins are fetched, not yet merged into the fixed lint source.

The operations-reference correction now passes the canonical configuration documentation check and all four parser tests (zero failures/skips). It documents the four registered switches and corrects stale format-2 registration/startup-DDL wording. This is a documentation/script working-tree delta only; Rust source under api remains byte-identical to `21333475c` and its queued lint result will name that Rust source. The documentation commit and updated plan mirror follow the gate; no source head has been moved.

S5 exact-head CMA gate completed at `706f5a7d757eefc5c4a760ae997584471c48dd01`: cargo test --locked -p cma --bin cma capacity, 59passed/0failed/62ignored, compiled in3m49s and executed in0.08s under one existing shared heavy lock with two jobs. All three exact live recovery registry tests passed. The source guard confirmed unchanged clean HEAD; this replaces the earlier invalidated diagnostic as CMA unit evidence. Targeted actual Mongo is queued next; ignored tests are not counted as passed. One reusable target is retained; no owned Mongo resources exist before acquisition.


Develop run `43055008152` completed successfully at source `8d59f6154f38540ab7e57f5496a1c81f49ef5b51`: all 25 jobs green, no failed/cancelled jobs (read Thu01:01SGT). This is develop evidence for the already merged work, not a segmentation candidate gate. No branch protection was changed.


### Additional v2 discovery and aggregate-save roots — Thu 01:10 SGT

P1-1 source review found `agent_worker_store.rs:86` still filters live-connection restart discovery to v1, and `:24` does the same for the command-admission generation hint. S5 `agent_failure_pacing/discovery.rs:29` likewise filters both due-work arms to v1. S1 `agents.rs:359` still requires v1 in the aggregate-only `save_agent` CAS. Thus even converted publication transactions cannot make adopted Agents discoverable or make ordinary aggregate-only recovery saves work. The existing constructor at `agents.rs:135` is a legitimate v1-new-record boundary; do not blanket-replace schema constants or change parent collection predicates.

Ownership: S2 takes `api/crates/cma/src/store/agent_worker_store.rs` (absent from the exact manifest), including compatible live discovery and admission-generation observation plus tests in a focused shared-root module. S5 retains `agent_failure_pacing/discovery.rs` and its two bounded discovery arms. S1 retains `agents.rs` and must preserve the exact captured v2 envelope/projection identity in aggregate-only saves, including all existing CAS/cleanup/title/profile/worktree fences. S4 retains transactional publication and the command-reserve/control omissions reported earlier. Require actual adopted-fixture discovery, work claim, aggregate save, restart routing, malformed-v2 refusal and no version downgrade, alongside unchanged v1 behavior. Source conversion is not activation evidence.

P1-3 shared routing must intercept completed recovery before the ordinary `schedule_agent_connection_recovery` route. Admitted/expired global discovery alone does not rediscover Completed carriers; by-Agent discovery supplies only candidates. Use the restart live-Agent enumeration after its v2 fix, plus exact original completed receipt/current revision checks. Later progress still requires an independently proven route and cannot silently reuse the old target or grant ordinary reconnect from absence. S2 owns this routing; S5 owns the typed receipt/supersession decision. Native proof remains a separate activation dependency.

### S5 first actual Mongo result and fixture correction — c15bb553f

At unchanged706f5a7d7, pinned disposable Mongo8.0.13 ran all18 S5 tests:16passed/2failed/0ignored in25.44s. Passing cases include both513-Agent/Workspace overflow refusals (P2-6), carrier identity/CAS/corruption, bounded two-stream and completed-by-Agent discovery, authority refusals, scoped fencing, unknown-receipt handling and pacing. The profiler setup failed canonical validation because a synthetic Completed timestamp differed from its result.published_at. Restart setup failed because simulated preceding work changed a fixture Agent without advancing its revision, leaving the Created integrity checkpoint falsely current. No production validation has been weakened. Both owned fixture failures are corrected in c15bb553f: coherent completed witness times, revision advance for simulated work, and the actual legacy append for the capacity failure so both aggregate heads remain consistent. Profiler samples no longer rely on ordering equal-millisecond timestamps; docsExamined is now bounded by that sample's returned count.

The owned Mongo container and volume were removed on failure. A single bounded300-second shared-lock run is queued to rebuild/reexecute CMA units then run actual Mongo using the same target. The current source stays fixed during that run. P1-3 remains unqualified; P2-6 now has actual Mongo evidence on706f5a7d7, pending combined-root qualification and reviewer closure.


### Integration checkpoint `2c3537ee7` — Thu 01:21 SGT

Normal merges now include S1 `b680cf40f`, S3 `f4494faaa`, S4 `5fdfae281`, S5 `706f5a7d7`; develop `8d59f6154` is included. The only merge conflict was overlapping Mongo selector registration. The resolution retained the union of reader/audit, writer, recovery and retained-reopen selectors; it did not narrow a test set. S2 commits: `ce9da2336` operations/configuration documentation, `f5f103f08` valid original-Created retained fixture, `2c3537ee7` compatible live-Agent discovery and projected admission-generation hint. Two new real-Mongo regressions cover mixed-version keyset pages, missing Agents, no Agent rewrite and malformed-v2 refusal. They are source only until the Mongo gate executes.

Light checks on this source: formatting passed; file size passed for6,118files; canonical configuration reference passed and its four parser tests passed. Mongo-runner harness execution is still pending completion. The one-hour lint acquisition at the previous source exited75 with only `already locked`; there were no Cargo children, no Rust diagnostics and no Mongo fixture. It is a scheduling timeout, not a code failure or pass. The next gate uses `2026-11-12-segments-single-gate.py`, full pinned `CMA_SEG_GATE_HEAD`, existing heavy-1 with3600s acquisition allowance, two Cargo jobs and one target. It rejects changed/dirty source before launch and during execution; both refusal guards were verified without launching Cargo. Actual authenticated-role evidence is prioritized next, then strict CMA, adapter and the separately acquired full Mongo partitions.

Finding disposition after integration: P1-1 S2 discovery corrected in source; S1 aggregate-save and S5 due-work v1 filters still need their owners, in addition to S4 command roots. P1-2 complete authority/schema and real discovery source are integrated but the authenticated-role/consumer gates remain pending. P1-3 exact current registry consumption is integrated; completed-restart routing, later-progress proof and native production qualification remain open. P1-4 indexed consumers and model/legacy-usage parity remain open. P2-5 retry fixtures now retain the original unresolved attempt; P2-6 bounded children and scoped-root reuse remain pending actual-Mongo qualification. None is closed by these source merges.


Final pre-push develop sync at Thu 01:23 SGT: normally merged `61a0c9f0c` (#1709) at `7566b56f9`. Resolved only the strict-feature-parser overlap, retaining both existing segmentation options/tests and the independent permission-state option/test; adapter and infra changes merged normally. The next pure/configuration filter includes every feature parser, and adapter selection also covers permission tests. Canonical Mongo-runner harness completed: **31 passed,0 failed,0 skipped**; these are orchestration tests, not actual Mongo. Canonical configuration documentation passes after the merge. Formatting passed; the updated source-size check is running. No current-head Rust or Mongo result is claimed.


Published checkpoint `afc60d98a8ead25c9e6693c86034621eb29ebf82`: clean own branch, current develop normally included. Project9 Status is In Progress and aggregate progress comments are posted on #1348/#1349/#1350/#1351/#1674. Updated source-size gate passed **6,125files**. Authenticated Mongo is queued with the full3600-second lock acquisition allowance and immutable-source guard. No actual Rust/Mongo test has started yet. Removed the two applied scratch source/reference copies (11394bytes); requested plans/evidence remain0600. One reusable api/target is retained for the immediate gates; no own Mongo fixture is allocated while queued.


### S3 checkpoint review: lifetime work remains — Thu 01:34 SGT

Fetched S3 `a744a5a3feab021eb3a4e5042540116ae89bd36f` adds segmented consumer dispatch, but does not close P1-4. `agent_log_segment_reads.rs::selected_until` always loops from sequence0 through the captured lifetime head; `done` prevents retained rows only and never stops the range loop. `selected` accumulates every matching row in one Vec without total count/byte bounds. Thus200-row physical pages do not bound total checkouts, bytes, memory or title latency. The transcript caller can return None after segmented validation, and still gates v2 dispatch on the legacy opt-in. Both can select the incompatible lifetime fallback on adopted data. The first wrapper also retains an unnecessary `mut keep`, which strict lint will reject. This is read-only source evidence at the fetched pin, not a measured gate failure.

S3 owns the correction: read current usage/approval facts from the certified captured projection; seek turn/message evidence for the requested transcript page; point-prove only referenced source; seek the exact first eligible title turn and its references. Never scan unrequested seals to establish an indexed absence already covered by the generation proof. Retain v2 reading when legacy optimization/admission flags close, and return a typed error rather than legacy fallback for corrupt/missing v2 evidence. Add1/10/100/1000-segment profiler/byte tests for a fixed requested page, long-turn parity and kill-switch reader-floor tests. New source may be integrated as an unfinished checkpoint after the current authenticated gate; it is not bounded-reader acceptance. S2 response implementation must follow the same indexed requirement.

Other fetched checkpoints waiting for the immutable current gate: S1 `e955d37f1` (exact control-index options), S4 `5b49c51d6` (optional generation), S5 `c15bb553f` (original recovery-witness fixtures); develop `1a7325d58`. No current gate source has changed.

### S5 admission isolation and normal develop merge — c3c8c5f59

Published c3c8c5f59157ffcb23b51a9073de57ad004138a2, normally including develop42fbf75deaae699ac2f17c6e93528daef140b9fa. The single FeatureFlags conflict retained both recovery parent pacing and the new permission bounds flag.

At fixed c15bb553f, CMA capacity units passed again; actual Mongo remained16passed/2failed. The completed witness/Created-history fixture defects are no longer the failure sites. The actual consumer returned all correct32/32/1 pages, but its profiler assertion expected wire batchSize32: pinned mongodb3.9.1/src/operation/find.rs increments equal positive limit/batch pairs, sending33 while limit stays32. Commit208532217 pins that driver behavior and retains exact returned-row/document/key/byte budgets. Restart now reaches service.admit and receives Conflict; no native call occurs. Explicit parent/Agent/Workspace/Worktree fixture validation and test-only preparation/bootstrap/final-transaction diagnostics isolate that remaining refusal in the next run. Production outcomes and authority refusals are unchanged. Both owned Mongo resources were removed.

The new merged source is fixed and queued for the bounded CMA+Mongo rerun. Pure core/HTTP/adapter tests and strict lint follow. No completed-activation or PR-ready claim. The narrow format2 discovery/index pin remains daf605ab410fc4737633aa3997e67192d4c4112b, published as previously requested.


### Immutable gate export and S2 scheduling

Authenticated Mongo finally acquired heavy-1 at approximately01:48SGT and is compiling exact committed source `afc60d98a`; no result yet. The preceding wait was22minutes, with no fixture or Cargo before acquisition. Later gates use a byte-verified archive of that same committed tree at `api/target/segmentation-gate-source`, with the sole reusable Cargo target still `api/target`. This is a disposable source archive, not another Git worktree. Guard regressions reject both a wrong candidate and modified exported bytes before Cargo. New source work may proceed after the current in-place authenticated gate finishes without invalidating archived gate evidence. All heavy steps still separately acquire existing heavy-1 with3600seconds allowance and2jobs; no lock file is changed.

S2 has prepared the accepted-work scheduler and seven focused tests outside the immutable source while it compiles. It retains at most32 discovery candidates, executes at most2 original accepted operations per sweep, alternates due/expired streams and keeps fresh receipt-bound authority in S5's real service. It observes later explicit schema installation without startup DDL and continues accepted work when new admission closes. Completion after restart/loss still needs the separately recorded positive S5 routing proof; this scheduler alone cannot close P1-3. Drafts are not yet source or test evidence. The response event cursor contract is `2026-11-12-segments-response-event-contract.md`; it preserves exact event numbering and requires bounded indexed seek, not a linear lifetime skip.


## Lead allocation 2026-11-12 01:55

Lead decision #206 supersedes the pending S2-agreement condition. S1/cma-cost
owns the response-consumer implementation, including v2 AND legacy response
retrieval/streaming. S2 stops editing these paths and retains the combined PR.
Branch: `tl/cma-log-segments/response-consumers`, based on accepted S1
`e955d37f1ede36e7303c8ddaa7ace01ce46efbcb`, with normal dependency/develop merges.
Exact existing and planned child paths:

- `api/crates/cma/src/store/segment_response/mod.rs`
- `api/crates/cma/src/store/segment_response/telemetry.rs`
- `api/crates/cma/src/store/segment_response/indexed.rs`
- `api/crates/cma/src/store/segment_response/indexed/capture.rs`
- `api/crates/cma/src/store/segment_response/indexed/turn.rs`
- `api/crates/cma/src/store/segment_response/indexed/output.rs`
- `api/crates/cma/src/store/segment_response/indexed/points.rs`
- `api/crates/cma/src/store/segment_response/indexed_tests.rs`
- `api/crates/cma/src/store/segment_response/indexed_tests/fixtures.rs`
- `api/crates/cma/src/store/segment_response/indexed_tests/bounds.rs`
- `api/crates/cma/src/store/segment_response/indexed_tests/refusal.rs`
- `api/crates/layer2-core/src/sandbox/store/agent_response_log.rs`
- `api/crates/layer2-core/src/sandbox/store/agent_response_log/indexed.rs`
- `api/crates/layer2-core/src/sandbox/agent_reads/response.rs`
- `api/crates/layer2-core/src/sandbox/agent_reads/response_indexed.rs`
- `api/crates/layer2-core/src/sandbox/agent_reads/response_indexed_tests.rs`
- `api/crates/layer1-public/src/responses/v2/projection.rs`
- `api/crates/layer1-public/src/responses/v2/output.rs`
- `api/crates/layer1-public/src/responses/v2/routes.rs`
- `api/crates/layer1-public/src/responses/v2/stream.rs`
- `api/crates/layer1-public/src/responses/v2/stream_pages.rs`
- `api/crates/layer1-public/src/responses/v2/response_window_tests.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_projection.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_stream.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests/parity.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests/authority.rs`
- `docs/CMA_Bounded_Responses.md`
- `api/crates/layer1-public/src/responses/retrieve.rs`
- `api/crates/layer1-public/src/responses/stream.rs`
- `api/crates/layer1-public/src/responses/legacy_indexed.rs`
- `api/crates/layer1-public/src/responses/legacy_indexed_tests.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests/pagination.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests/large_history.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_projection/output.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_projection/metadata.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_stream/page.rs`
- `api/crates/cma/src/store/segment_response/indexed/facts.rs`
- `api/crates/cma/src/store/segment_response/indexed/seek.rs`

The existing S1 capture/schema/audit ownership is unchanged. The previously
agreed capture/evidence visibility-only handoff is consumed with real callers.
S4 owns shared projection facts/publication/bootstrap: historical model knowledge,
post-terminal interval usage and exact deferred-failure source references. Until
its pin lands, code against its latest interface through one isolated facts seam;
missing proof remains a typed refusal, never an inference or lifetime fallback.
S2 retains shared composition/config/exports and canonical Mongo registration;
small necessary registration deltas are supplied for integration, not overlapping
semantic edits. S3 retains transcript/REST-history/frontend consumers.

Default-off admission and legacy optimization remain default-off; retained v2
reading must survive their closure. Qualification includes >10,128 rows / 4 MiB
history, long turns, bounded seek/pages, golden v1/v2 parity, current authority,
and usable result-size pagination. Source changes are not activation evidence.
The exact event-count/ordinal seek contract is
`2026-11-12-segments-response-event-contract.md`.


### Authenticated gate and next combined checkpoint

Real authenticated gate at `afc60d98a8ead25c9e6693c86034621eb29ebf82` completed with1passed/0failed/2568skipped. The canonical migrator installs format2 with its original witness; the generated isolated runtime role successfully reads/writes gates, admissions and work, rejects foreign collections and missing grants, and the schema refuses v1/missing-witness replacements without changing the original row. The cleanup trap completed with exit0. This is substantive P1-2 evidence, but the exact nonunique/index-drift delta at S1 `e955d37f1` still needs the newer combined Mongo rerun and review before closing P1-2. Log: `2026-11-12-segments-authenticated-afc60d98a.log`.

Normally merged the four current slice checkpoints. S4/develop conflicts preserved both permission and parent-pacing flags plus Sandbox snapshot/timing flags; all segmentation fields remain strict and default-off. Preserved the existing S2 scoped source reuse with S4's optional-generation semantics and explicit segmented retained-reopen refusal. Writer-reported47unit/config and76actual-Mongo passes at `e094ffeea` are separately mirrored in `docs/notes/2026-11-12-s4-postreview-evidence.md`; they are not a combined-head pass. S5 diagnostics/driver-wire budget adjustments are integrated. Removed only S3's unused mutable wrapper parameter in `dcb94ae22`; its lifetime traversal and reader-floor gaps remain S3-owned P1-4 work.

S2 `0eba12933` wires the actual accepted-work service into the existing supervised scheduler: one retained32-request page, two concurrent attempts, alternating admitted/expired streams, independent failure handling, and restart from durable original requests. It performs no admission, inferred Caller construction, ordinary reconnect or input replay. A bounded, read-only collection-existence hint avoids hinted queries before explicit schema installation; it grants no execution authority. Seven unit tests cover concurrency, refusal isolation, discovery failure, bounds, restart, cancellation and later schema installation. The authenticated role regression also invokes the presence hint. These new tests are source only until the next candidate gate. Completed-work/loss routing and the native proof producer remain separate P1-3 blockers; the scheduler does not close them.

Current integrated source has no response-path semantic edits by S2. S1 consumes the already recorded exact event-count/seek contract. S2 will consume S1's published response branch normally and apply only the requested shared exports/registrations. The08:00PR plan remains one combined default-off PR; response/transcript parity and scale, remaining ordinary writer/discovery roots, completed-recovery routing, actual combined gates and rollback qualification block readiness. Strict-green estimate remains Thu12:00–16:00conditional on those deliveries and build capacity; full recovery activation has no defensible ETA while the native producer is deferred. Preserve that distinction in review/release decisions.
