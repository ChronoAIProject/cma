# Combined CMA strict-lint wiring assignments

**Current checkpoint, Wed 23:58 SGT:** integration source245f8e821 (last pushed2cf7d3fe6). Strict CMA65 dead-code diagnostics /0 type errors; pure contracts41pass/2482skip. Authenticated Mongo is queued under heavy-1; no current-head Mongo pass claimed. Narrow S1 operator pinb11107979 and format-2 pindaf605ab4 are consumed by S1f205616a4. New S2 registry pinad5641a71 is published for S5. Latest neutral S5 commands58ffd3b61 are available for the next merge. All four P1 activation gates remain open until executable qualification and review; the timestamp-parser source concern is withdrawn after inspection. The proposed PR remains one combined default-off PR, not yet ready. Detailed ownership, exact pins, six-finding closure ledger, gate list and Thu/Sun schedule follow below.


Baseline: integration `5ea82dbc9`, 184 binary warnings and 35 test warnings (31 duplicates), plus 187 passing selected core/API tests. The 184 distinct binary diagnostics are inventoried in `2026-11-11-segments-lint-inventory.tsv`. Counts are source locations, not independent defects: an unwired entry point makes its whole implementation appear unused. Strict CMA Clippy has not passed.

| Diagnostic group | Count | Missing production entry / assignment |
| --- | ---: | --- |
| Writer/publication/control | 94 | S4 owns ordinary command/worker/lifecycle append roots, adoption/rollover/bootstrap, exact receipt reconciliation and output-index publication. S2 owns the shared gate-control CLI dispatch. |
| Audit/rebuild | 34 | S1 owns executable one-unit audit/rebuild invariants and its parser/data validation. S2 owns the shared operator command/authority and migration-ledger registration that call the S1 API. |
| Capacity recovery | 19 | S5 owns accepted-work Mongo/service/runtime adapters and durable completion verification. S2 owns shared route/worker composition and carrier migration registration after S5 exports the adapters. |
| Captured source | 13 | S1 owns implementation; S2/S3 consume snapshot capture and point proofs in their bounded page readers; S4/S5 consume transaction capture in actual publication roots. S4 already merged S1's executable checkpoint. |
| Read-port delegation | 6 | S4 owns `store/agent_log.rs`: forward the existing AgentLogStore point/visitor methods to S1's `store/agent_log_store.rs::{point,visit_page}` without changing append semantics. S1 fixes any reader contract/test issue surfaced by that dispatch. |
| Other/scoped roots | 18 | S2 takes the unassigned runtime-loss and unpublished-recovery roots. S4 takes its lifecycle checkpoint root. S1 retains the separately assigned purge root and schema. |

## Exact ownership and next pinned deliveries

- S1 / cma-cost: retain `agent_log_segments/{capture*,reads*,audit*,schema*}`, `agent_log_reads*`, and `sandbox_worker_store.rs` (purge). Supply additive schema for output-change seek and gate/admission collections in coordination with S4's typed records; maintain exact read-only visitor/point semantics. Audit entry remains `run_unit(client,database,agent,audit,cancel)` plus `install(client,database,agent,audit)`. Any missing command parser belongs in a new S1 `audit_command.rs`; do not edit the S2 shared main/migration/authority files. Publish one small wiring checkpoint next and its focused gates.
- S2 / cma-db-contention: own `main.rs`, `store/migrations/{mod,authority,ledger,agent_log_segments}.rs` and new shared operator modules, `store/isolation.rs`, root composition/config/router registrations, and **newly assigned** `store/sandbox_reconciliation.rs` and `store/sandbox_unpublished_recovery.rs`. Wire S5's scoped helpers at transaction start, preserving root commit/abort/unknown handling; mount the authorized dry-run service. Supply new migration/control plumbing with explicit operator authority and no startup writes. Continue the response page consumer after the shared roots.
- S3 / cma-seg-2: publish the registered `transcript-consumers` checkpoint and actual bounded transcript/current-state/title/usage/approval paths against S1 capture/point APIs and S4 facts. Supply the concrete service/source constructor required by shared composition. No complete lifetime fallback for a segmented source; no unused adapter registration used to imply delivery.
- S4 / cma-seg-lead: own `store/agent_log.rs` and the publication caller families in the plan, including `lifecycle_transaction.rs` and `checkpoint_capture::validate_and_fence_begin_scoped`. Wire `writer_commit::{prepare,persist}` in actual same-scope transactions; keep unknown outcome reconciliation exact. Implement output-index/bootstrap evidence from `9327ec32f`; support the independently admitted CapacityRecovery empty-batch successor. Export gate/schema requirements to S1/S2. Do not edit the S2 reconciliation/unpublished roots.
- S5 / cma-seg-3: retain recovery child modules, checkpoint/maintenance roots already assigned, restricted runtime reconnect/completion and accepted-work adapter. New scoped root helpers are consumed by S2. Publish a concrete production service/worker constructor and any schema discovery fields for the work carrier; S2 wires only those public constructors. Complete receipt verification before status can report Completed. No ordinary PublishConnection fallback after capacity recovery.

Correction: runtime-loss recovery starts in `sandbox_reconciliation.rs::recover`, not `sandbox_worker_store.rs`; the latter transaction is purge. S2 owns the former now. S5 owns the helper beneath it; this preserves non-overlapping edits.

Frozen dependencies already published: scope `9e5c47a72`, S5 registrations `34c9a3228`, output-index contract `9327ec32f`, executable S1 capture checkpoint `5b8a823fc`. Normally merge the narrow dependencies; do not import the evolving mixed integration tip into a slice PR.

## Validation and schedule

Next owner checkpoint requested tonight Wed 2026-11-11 21:30 SGT; report an exact SHA and actual gates or the concrete missing interface. S2 integrates each checkpoint normally and reports cross-slice failures immediately. Each heavy step acquires/releases its existing shared lock separately, at most two Cargo jobs and one target. No suppression, blanket allow(dead_code), fake call or test-only registration may replace missing production wiring. Target is strict `cargo clippy --locked -p cma --all-targets --all-features -- -D warnings`, then affected Mongo success/failure and full release qualification. Source availability and default-off do not constitute readiness.

## Follow-up integration findings (20:45 SGT)

The scoped-root checkpoint `c13eff3b5` is published. The combined candidate now also includes S4's output publication and S5's exact settlement/receipt source, via normal merges. Strict CMA Clippy is running on the combined candidate under heavy-1, two jobs; source presence is not gate or production acceptance.

### Shared migration runner overlap

Ledger track B has already generalized the same migration files on `tl/cma-ledger-collector/bounded-collection`: registry introduction `a1dac595d`, test harness `ea171fe6a`, invocation type `cc4e51a5c`. Reuse that registry; do not independently rewrite `ledger.rs` or create a second ledger. S2 extracted those migration-only changes at `8bea360e6` on `tl/cma-log-segments/migration-registry`; the migration subtree exactly matches the current track B implementation. This dependency adds no unrelated runtime work. S2 extends segment dispatch after consuming this pin, preserving B’s original step bytes. S2 retains `migrations/agent_log_segments.rs`; S1 retains schema internals.

The existing Step API only describes nonunique index creation. Segments require exact collection validators plus unique indexes. The shared runner extension must preserve old Activity and LedgerCollection IDs/checksums/receipts, the same global fenced lease and unknown-history refusal; permit explicit schema units from `agent_log_segments::{inspect,apply_next}`; renew before/during every effect; and publish completion only after pending_count is zero. No startup DDL. The displayed effect count must be named schema steps for this migration, not created indexes. Plans and status remain read-only. S1 must include every schema child source in the immutable checksum once the validator/index inventory is finalized.

### S5 native confirmation ownership — assigned

Allocate new focused Layer 3 capacity-recovery contract/hosted implementation/test children to S5, together with its existing restricted reconnect and registry children. S2 owns shared trait/export/module registration; S5 supplies the minimal patch against those shared files for integration. Preserve Layer 3 neutrality: no Layer 2 RecoveryClaim/RecoveryConfirmation type may enter a Layer 3 API. The lower-layer request/result must carry the exact original native/home/runtime identity, writer fence, current independently observed native state and active input identity. S5 maps that to its existing Layer 2 confirmation.

S5 must identify any required Layer 4/provider route extension before editing a shared runtime dispatcher. New focused provider children can be assigned to S5; shared dispatcher changes must pass through the runtime integration owner. Unsupported native proof is a typed refusal. The ordinary settings/context replay or PublishConnection path is never recovery fallback. No inferred Idle/Working/WaitingApproval from an Agent aggregate, empty channel or original open receipt alone. A real pinned-runtime test is required before production recovery is advertised.

S5 also owns success/foreign/delegated/refused/no-store HTTP tests and API documentation for its dry-run REST child now mounted by S2. S2 retains only the shared composition registration.

S2 explicitly takes the shared `layer3-adapter/src/adapter.rs`, `layer3-adapter/src/lib.rs`, `layer3-adapter/src/hosted/mod.rs` (module registration) and `layer3-adapter/src/hosted/adapter_sandbox.rs` dispatch changes required by restricted reconnect. S5 owns new focused `layer3-adapter/src/agent_capacity_recovery.rs`, `layer3-adapter/src/hosted/capacity_recovery.rs` and corresponding focused tests. S5 supplies the smallest shared-file patch with its native-proof producer; S2 integrates and tests it. If the producer needs Layer 4 shared dispatcher changes, S2 coordinates that exact patch with the runtime integration owner before overlap. This assignment does not authorize or activate recovery and does not claim the native handshake exists yet.

The canonical Mongo runner registration is separately published at `9011a3796` on `tl/cma-log-segments/mongo-registrations`, based on shared recovery registration `34c9a3228` and current develop. Slice branches can normally merge this dependency without importing the mixed integration branch. Integration compile corrections are isolated in `6123c0ecf`: a narrow incarnation export, typed publication collection, maintenance entry alias, explicit legacy race-test call, and valid AgentId construction. The first strict gate found these five source compile failures; its two additional diagnostics identify unwired audit/lifecycle entry points. No claim of strict green.

Shared operator source is implemented at `23a017d91`: explicit bounded audit unit/cancel/install and gate status/replace/close dispatch, exact operator authority, topology and complete-schema verification, original retry identities and aggregate output. Gate actions refuse until gate/admission collections are included in the verified S1 inventory. No startup action, DDL, live grant or activation is introduced. Six success/refusal/security tests are registered; execution remains pending. The shared parser is now S2-owned and implemented, so S1 need not add audit_command.rs. See `docs/notes/2026-11-11-segment-operator-entry.md` for the precise command contract.

## Latest measured wiring inventory

Strict CMA run at `b813b8ecf`/operator source `23a017d91` finished with 39 distinct diagnostics: 38 dead-code locations and one unused import, no type errors. This replaces the original 184-warning inventory for the combined source containing S4 keyed append and S5 settlement. S5 recovery/maintenance owns 31, S4 lifecycle/receipt reconciliation four, S1 raw-transaction capture variant one, and shared schema migration dispatch three. The latter is implemented at `6081cf2f6` and awaits the next strict run.

- S5: construct the real admission/status/accepted-work adapter and worker, consume settlement under exact commit/abort/unknown reconciliation, and remove the superseded live_activities wrapper if no production consumer needs it. Scope insert_one is waiting on this production admission path. Restricted native confirmation still requires the assigned S2 shared trait/dispatch patch and S5 producer.
- S4: wire the lifecycle checkpoint scoped helper and exact receipt reconciliation in the remaining original caller transactions. The keyed append and point/visitor checkpoint is already consumed. No broad caller-conversion claim follows from that one entry.
- S1: remove/tighten the obsolete raw-session CaptureMode::Transaction entry while retaining refusal of transaction/snapshot proof misuse; production transactional capture uses TransactionScope. Complete output/gate/admission/work schema and rebuilt output evidence before operator/migration qualification.
- S2: explicit operator dispatch and shared schema migration registration are implemented, with six operator security/refusal tests, two migration authority/selection tests and two real-Mongo migration contracts. Gates are in progress, not yet a pass. Shared restricted reconnect integration ownership is accepted. Response paging still requires indexed/segmented consumer completion.
- S3: publish the actual bounded consumers and constructors. Their absence need not produce a dead-code warning, so a zero-warning result alone would not prove their delivery.

Each heavy step remains a separate existing heavy-1 acquisition with two build jobs and one retained api/target. No warning allowance, fake call or weakening of the strict lint command was introduced.

## Restricted native recovery ownership checkpoint — 21:38 SGT

S2 (cma-db-contention) owns shared Layer 3 trait/export/Hosted/Codex dispatch and any shared Layer 4 protocol/dispatcher registration. S5 (cma-seg-3) owns the focused native producer, restricted transport, model-to-adapter mapping, staged registry activation and success/refusal/crash tests. This assignment resolves the shared-adapter ownership request.

The consumed S5 source is d9a0335d4dec82dacdaf73326c946bcd1da30722. Its shared patch is applied to executable source and removed from docs (Markdown-only). The stable entry is:

```rust
async fn reconnect_agent_for_capacity_recovery(
    &self,
    request: &layer3_adapter::agent_capacity_recovery::CapacityRecoveryRequest,
    updates: tokio::sync::mpsc::Sender<layer3_adapter::ConversationUpdate>,
) -> layer3_adapter::agent_capacity_recovery::CapacityRecoveryResult<
    layer3_adapter::agent_capacity_recovery::CapacityRecoveryConnection,
>;
```

The existing `HostedBackend::capacity_recovery_transport(&self) -> Option<&dyn CapacityRecoveryNativeTransport>` defaults to None. S5 implements `CapacityRecoveryNativeTransport::reattach_existing` in focused children, returning the channel plus independently produced `CapacityRecoveryConfirmation`. No provider currently supplies the production producer. Until it does, the route returns typed Unsupported without ordinary reconnect, prompt/settings replay, process launch or replacement.

S5 may add focused `layer4-runtime/src/agent_capacity_recovery.rs` and its test children for retained-process-only fencing and atomic native-state/output confirmation. S2 owns registration and shared changes to supervisor/bridge/protocol/dispatch roots; S5 must supply the minimal exact patch with its focused implementation. Existing ledger tracks retain ownership of ledger state and retirement logic. S5 must reuse their receipt/fence authority and expose any new ledger dependency before changing it. No new inferred native state from the Agent aggregate or the original open receipt is acceptable.

Required producer evidence: exact original home/runtime boot and generation/conversation/open digest; current independently observed Idle/Working/WaitingApproval and prompt identity; current writer fence and fresh challenge; contiguous retained-output interval; no input or acknowledgement before the same durable recovery completes. Tests must include absent/dead retained process, mismatched identity/fence/challenge, approval wait, pending terminal output, output gap, restart before/after durable completion, and zero new input/launch. A real pinned-runtime handshake remains an activation gate.

S2 has also merged S1 9f2fe7ac0, S4 191afaa86 and S3 33d878350 into the combined branch. The next strict CMA result, adapter tests and updated remaining-wiring inventory will be published tonight; these source integrations are not executable recovery or activation evidence. The schema for the bounded accepted-work carrier remains S1/S5 dependent: S5 owns exact serialized record/discovery predicates, S1 owns its validator/index inventory, S2 owns the shared migration/authority/backup registration. No production command or grant was changed.

Shared restricted reconnect is separately published at `ccfce0137ec2d9df4e85bd8d74dfebf0d9e0093a` on `tl/cma-log-segments/restricted-reconnect`, based on S5’s contract plus a normal develop merge. S5 can consume this dependency without importing the mixed integration tip. Adapter integration is `851e69533`; audit parser/checksum integration is `4748868a2`; strict default-off transcript config is `ddc1ec39c`. The canonical runner’s 31 mocked infrastructure tests pass on this combined tree, and the source-size gate passes 6,008 files. No actual Mongo test or current strict-lint pass is implied.

## Exact combined strict result at d5ccfefe4 — 21:54 SGT

`cargo clippy --locked -p cma --all-targets --all-features -j 2 -- -D warnings` completed under heavy-1 with exit 101. It reports **33 distinct dead_code diagnostics, zero type errors**. The earlier 39/184 counts are superseded. No warning suppression or fake production call was added.

| Owner | Count | Required next production change |
| --- | ---: | --- |
| S5 | 30 | Instantiate the accepted-work admission/status/claim/retry/completion service and worker around `agent_capacity_recovery/{admission,authority,records,settlement,status}.rs` (28 locations). Its insert path consumes `TransactionScope::insert_one` (1). Remove the superseded `sandbox_maintenance::evidence::live_activities` wrapper and update test callers, or supply its genuine production use (1); no cfg(test) mask of missing production behavior. Supply public constructors to S2 for composition. |
| S4 | 2 | Wire `writer_reconcile::{Outcome,reconcile}` into original caller commit-uncertainty handling and compare the exact original publication/domain receipt. The distinct keyed-replay entry does not discharge this remaining caller obligation. |
| S1 | 1 | Retire `CaptureMode::Transaction` from the unscoped snapshot API. Preserve ordinary-session/transaction-session and cross-transaction proof-reuse refusal tests through actual snapshot validation and `TransactionScope`; no public raw-session transaction shortcut. |
| S2 | 0 diagnostic locations | Current shared audit/gate CLI, schema migration, lifecycle imports and restricted adapter registration compile. Complete authority/backup registration after S1/S5 freeze the work-carrier schema, and consume the real S5 service constructors. Indexed/segmented response paging remains a functional gap beyond lint. |
| S3 | 0 diagnostic locations | The published legacy transcript filter is linked with strict default-off config; it is not indexed/segmented transcript completion. Continue bounded consumer implementation and Mongo scan/checkout evidence. |

S5 should first merge the shared adapter dependency `ccfce0137ec2d9df4e85bd8d74dfebf0d9e0093a`; this resolves the shared-file ownership request. A native producer still must prove retained-process state/output independently. Any shared Layer 4 dispatcher patch remains S2-owned. S5 supplies focused producer code and the minimal dispatcher patch; no provider-specific protocol parser belongs in the kernel-neutral supervisor.

The exact diagnostic TSV and complete redacted compiler log are in the lead notes at `2026-11-11-segments-current-lint.tsv` and `2026-11-11-segments-combined-lint.jsonl` (0600). The adapter/contract tests are a separate next gate; none of this claims strict green, actual Mongo qualification or production recovery. Next source checkpoints are required tonight, Wed 2026-11-11, ahead of Thu review.

Executable adapter evidence: at combined source `60790c80dfcdac4f336b596bd245a69fd2eb6554`, 22 selected Layer 3 tests passed (657 skipped), including all seven restricted native-recovery contract/transport tests and the affected filesystem, retained-home and runtime-recovery tests. The Layer 3 subtree is identical to dependency pin `ccfce0137ec2d9df4e85bd8d74dfebf0d9e0093a`. Gate used heavy-1 with two build/test jobs and released it on completion. These are model/adapter tests; they do not demonstrate a production native producer. Log: `2026-11-11-segments-adapter-tests.log` (0600).

Subsequent integration at `f4f2266e7` consumes S1 `e85e5de1f` and S5 `1481b1d7f`. S1 has removed the unscoped transaction token from production; only its negative qualification fixture retains that invalid token. S5 has strengthened exact receipt reconciliation after later Agent revisions. The accepted-work carrier now requires BSON format_version **2**, including the strict result binding; do not migrate the earlier draft v1 row. RecoveryWork's internal domain format remains 1. S1 owns the exact schema validator/index child after S5's frozen serialized shape and discovery predicates; S2 owns shared runner, role and backup inventory. The 33-diagnostic result is historical to d5ccfefe4 and is not a count measured on this newer source. The current unit gate includes audit/parser/migration/transcript-config and the new recovery codec/receipt tests.

Final local unit evidence for the checkpoint: **26 selected CMA tests passed** at `f4f2266e7ae9b187b8dfe6437c4f8390e58ccc7c` (2465 skipped), covering shared operator authority/refusal, migration registry/authority, canonical audit parsing, strict transcript opt-in, bounded audit batches, recovery codec and exact receipts. The 22 adapter tests remain valid on the unchanged Layer 3 subtree. Formatting, whitespace, the 6014-file size gate and a 142-commit secret scan passed. No actual Mongo test ran in this integration checkpoint; native confirmation and production recovery remain unimplemented. The strict result remains the explicitly historical run above. Heavy steps each acquired/released the existing heavy-1 lock separately with two build jobs. The sole owned target directory was removed after the final test gate; shared lock files and other worktrees were untouched. Develop merges remain frozen.

## S1 narrow operator/migration handoff — 2026-11-11 22:36 SGT

Merge **`origin/tl/cma-log-segments/operator-migration` at `b11107979231ec0da5683d8719722c8b4dfea15e`** normally into the S1 reader branch. This branch starts at current develop `207be186c` and contains only 14 shared operator/migration files (+786/-4), extracted from the qualified combined checkpoint. It consolidates operator `23a017d91`, migration registration `6081cf2f6`, and canonical-parser/schema-child checksum correction `4748868a2`. Do not merge those old mixed integration tips to obtain the dependency.

A read-only merge-tree check against S1 `41ef81cab` found exactly one expected add/add conflict: `api/crates/cma/src/store/migrations/agent_log_segments.rs`. Keep the **incoming dependency version** of that file; it is the complete shared-ledger adapter replacing S1's earlier plan/apply-next scaffold, including the schema-child checksum. `store/mod.rs` and other shared files merge automatically. During that normal merge, `git restore --source=b11107979231ec0da5683d8719722c8b4dfea15e -- api/crates/cma/src/store/migrations/agent_log_segments.rs`, then stage it and complete the merge. This changes only S1's own checkout and retains both histories. No force push or rebase.

S1 already supplies the `agent_log_segments` module and its audit, canonical audit parser, schema and writer-control APIs. The narrow dependency intentionally does not copy those implementations from another slice. It is a dependency to consume on S1, not an independently buildable PR against bare develop. The same dispatcher/migration code passed the selected operator/migration tests within the 26-test combined run at `f4f2266e7`. Run S1's resulting-head gates after merge; that earlier evidence is not a claim that the new S1 merge passed.

Canonical Mongo test registration remains available at `9011a3796` on `tl/cma-log-segments/mongo-registrations`; the current S1 branch already carries the writer registrations, so check ancestry/selector union before importing it. Retain both segmentation prefixes and develop's ledger census prefix. The shared step registry itself is already in develop through the ledger collector; the older registry-only pin `8bea360e6` is unnecessary on current develop.

The operator/migration dependency makes no production change and does not activate either segmentation or capacity recovery. The develop merge freeze remains in force. Next: S1 consumes this pin and the forthcoming S5 format-2 carrier contract; integration records actual resulting-head strict, adapter and Mongo counts.

## Frozen format-2 carrier agreement — 2026-11-11 22:42 SGT

Accepted S5's narrow contract pin **`daf605ab410fc4737633aa3997e67192d4c4112b`**, branch **`tl/cma-log-segments/recovery-work-contract`**. S1 should merge it normally alongside the S2 operator/migration dependency. It carries only the exact JSON validator, three index definitions and their contract document; it is based on develop. Schema sources are under `api/crates/cma/src/store/agent_capacity_recovery/format2.{schema,indexes}.json`.

The frozen collection is `agent_capacity_recovery_work`; outer BSON Int64 format_version is **2 only**, internal work format is Int32 1. Original result witness includes two generic 32-byte binary digests, original result revision/through sequence/publication time, and the immutable S4 v2-domain binding to the exact work. No v1 registration, relabel, dual decoder, synthetic witness, TTL or deletion. All timestamps used for ordered discovery must be the exact 20-character UTC seconds format; codec refuses offsets/fractions and validates calendar dates. S5 must include that enforcement in its next production codec checkpoint, since the accepted integration base predates the contract's cited codec source `790ee7458`.

S1 installs strict/error validation with simple collation and the exact three nonunique indexes: `agent_capacity_recovery_due` on (format_version,work.phase,work.ready_at,_id), `agent_capacity_recovery_expired` on (format_version,work.phase,work.lease.expires_at,_id), and `agent_capacity_recovery_agent` on (agent_id,format_version,_id). Retain ordinary unique `_id_`; no partial/sparse/TTL/unique-per-Agent index.

S5 discovery alternates separate admitted/due and attaching/expired indexed keyset queries, each limit32/batch32/maxTimeMS1000 with the exact hint and simple collation. Both keys are (time,_id); restart exhausted sweeps, never skip/count/scan all rows. Discovery projects only bounded metadata and confers no authority: re-read/decode the full carrier, exact receipts and current parents in the claim transaction. Gate closure prevents new admissions while accepted work remains discoverable. Terminal history uses the exact Agent index and fresh authorization; completion attachment reconciliation must not use ordinary reconnect or input replay.

S1 owns validator/index installation and immutable checksum (including both JSON files), drift/v1/missing-witness/unknown-field tests, actual encoded four-phase round trips, and bounded explain plans. S2 owns shared registry, runtime grants and coherent backup inventory. S5 owns codec, discovery and accepted-work service. This is agreement on the published contract, not a Mongo/runtime qualification result or activation authorization.


## Current combined gates and PR plan — 2026-11-11 22:51 SGT

Source candidate **`2cf7d3fe6`** includes the accepted checkpoint, narrow S2 operator/migration dependency `b11107979`, S5 format-2 contract `daf605ab4`, and S1 `41ef81cab`, via normal merges. The only integration correction removes a duplicated production module declaration. Latest fetched S3 is `33d878350`, S4 is `191afaa86`, and S5 implementation is `1481b1d7f`. Those implementation checkpoints are already in the candidate; the later S5 contract is additive data, not the completed codec or worker. Develop remains `207be186c`; its merge freeze is still on.

| Gate at 2cf7d3fe6 | Exact result / evidence |
| --- | --- |
| Formatting / source size / runner harness | `cargo fmt --all --check` passed; source size passed for **6,014 files**; all **31 mocked Mongo runner infrastructure tests passed**, zero failures/skips. Mocked tests are not actual Mongo evidence. |
| Strict CMA Clippy, all targets/features, locked, warnings denied | Exit 101; **39 distinct dead_code locations, zero type errors**. S5 owns 37 (35 service/record/receipt locations, TransactionScope::insert_one, obsolete maintenance live_activities wrapper); S4 owns 2 (writer_reconcile Outcome and reconcile). This replaces the historical 33 count; the increase reflects added unconsumed receipt/witness code. |
| Layer 3 affected adapter contracts | **22 passed, 657 skipped**, exit 0, at this exact head. Includes all seven restricted recovery tests. Model/adapter evidence does not establish a production native producer. |
| Actual Mongo contracts | Queued under a fresh heavy-1 acquisition; exact selection will be enumerated by nextest and run in serial partitions of at most 40, one owned disposable replica set at a time. No current-head result yet. |

Logs are 0600 in the lead notes: `2026-11-11-segments-lint-2cf7d3fe6.jsonl`, `2026-11-11-segments-adapter-2cf7d3fe6.log`, and the forthcoming `2026-11-11-segments-mongo-2cf7d3fe6.log`. Each gate separately acquires/releases the existing heavy-1 lock with a blocking acquisition allowance of 3600 seconds, two Cargo jobs, one target directory. The lock files are never removed. The actual Mongo coordinator preserves the canonical runner's fixture ownership, cleanup traps, exact selectors and test time budgets; it runs the selected ignored contracts, not mocked runner tests. Source HEAD stays fixed throughout a gate.

### PR decision

**One combined, default-off PR to develop** from `tl/cma-log-segments/integration-20261111`, with reviewer sections for S1 through S5 and separately traceable commits. It advances #1348–#1351 and links the segmentation integration issue #1674. Do not use closure syntax until the full relevant issue acceptance is met. There is no second schema-only PR planned: the current schema, transaction capture, certified evidence, publication and recovery-carrier dependencies are not an independently qualified reader-first tree. The narrow handoff branches are merge dependencies, not release candidates. Introducing a second migration manifest before the inventory and checksum are frozen would add avoidable partial-deployment states.

The PR stays unopened until the gates below pass and the functional inventory is complete. A default-off setting does not excuse unused production drafts, missing recovery behavior, an unbounded segmented reader, or unqualified rollback. If the complete candidate misses the Thu-evening target, report that miss explicitly; do not call a compatible-reader-only deployment completed segmentation. An independently green bounded consumer may still be reviewed separately under its existing issue, but it does not replace this integration deliverable.

### Required gate list before PR READY

1. Freeze and normally merge all five slice heads; fetch/merge current develop; resolve conflicts without rewriting published history. Review the exact combined diff, preserve all existing ledger and Mongo selector registrations, and record source SHAs. All source files stay below 500 lines; no dead-code suppression or fake production reachability.
2. Formatting and **strict CMA Clippy**: `cargo fmt --all --check`; `cargo clippy --locked -p cma --all-targets --all-features -j2 -- -D warnings`. Strict lint for the changed layer1/layer2/layer3/layer4 packages also passes. Record exact commands, source head, test counts, skipped counts and failure names; CI runs the full repository suite.
3. Affected pure/domain/adapter/API success and failure tests, including restricted reconnect, authorization/refusal, exact receipt/witness decoding, keyset cursors, long-turn projection, unchanged-head polling, SSE-before-expensive-work behavior, codec v1 rejection and all four format-2 work phases. No test-only fixture proves a production native producer.
4. Real disposable Mongo replica-set contracts: transaction proof scope/cross-transaction rejection; schema partial apply/resume/drift; exact validators and index option checks; bounded captured reads and audit/rebuild; legacy/segmented page parity; output-index generations/maxima; all ordinary append caller families; concurrent rollover/adoption; unknown commit/retry/abort; recovery admission/claim/lease/retry/completion after later revisions; no authority from discovery; parent/writer fences; storage-only capacity recovery and preexisting segmented source. Preserve original test timeouts/assertions. Cleanup must remove each owned container/volume before the next partition.
5. Scale evidence at 1/10/100/1000 segments, fixed active segment/requested page and 100/200-entry pages: command/checkouts count, bytes, p50/p95/max, touched seals and peak memory. Work must follow requested page plus active state rather than lifetime. Long turns and output changes across segment boundaries must be indexed and bounded. No lifetime fallback for segmented data.
6. Compatibility/security: actual supported old reader and certified rollback artifact (#1327), mixed old/new writers, default-off restart, grants limited to the exact sidecars, immutable migration checksum including format-2 schema/index sources, coherent backup/restore of source plus all sidecars, complete no-startup-DDL/refusal tests. Applying schema alone never grants writer or recovery admission. Fail closed on unknown generation/format/receipt and never repair by deleting history.
7. A real pinned-runtime restricted recovery handshake and crash tests: original runtime/home/native identity and writer fence, independently observed state/output boundary, no new process/input/settings replay/ordinary reconnect, exact durable completion before attachment/output acknowledgement, and safe restart before/after completion. Missing capability remains typed Unsupported and blocks claiming #1351 recovered.
8. Repository structure/source size, secret scan, changed manifest/config parity and canonical Mongo-runner registration tests. Prepare stage/kill-switch/evidence instructions for the release owner. Review at the exact head plus lead merge relay are mandatory; develop remains frozen until explicitly lifted. After merge, monitor exact develop push CI and fix forward/revert a real failure within one hour; release requires its green exact-batch artifact.

### Remaining blockers and owner checkpoints

These ETAs are **integration planning estimates**, except S4's lead-confirmed 01:30 handoff. They are not claimed engineer commitments or test results. Reforecast after each pinned delivery/gate.

| Slice / owner | Concrete remaining blocker | Next checkpoint / ETA (SGT) |
| --- | --- | --- |
| S1 / cma-cost | Consume `b11107979` and `daf605ab4`; install only format-2 recovery carrier validator and three discovery indexes; include exact JSON bytes in checksum. Existing `schema.rs` assumes every expected index is unique; compare against each index's own expected options so the three deliberately nonunique discovery indexes qualify without relaxing the unique segment/publication/evidence contracts. Add four-phase encoded fixtures, v1/missing-witness/unknown-field refusal, index drift and bounded explain tests. Corrected reader/audit Mongo gates and actual rollback artifact qualification remain. | Dependency unblocked now. Schema/compile checkpoint estimate Thu 02:00; executable Mongo/reader evidence Thu 08:00–12:00, final compatibility fixes by 18:00. |
| S2 / cma-db-contention | Shared runtime authority and coherent backup inventory after S1 schema registration; consume S5 production service/worker constructors and minimal shared runtime dispatcher patch. Finish indexed long-turn/segmented response paging (deferred headers and unchanged-head optimization alone are incomplete). Integrate and run combined gates. | Shared inventory within 1 hour of schema pin; constructor wiring within 1 hour of executable S5 handoff. Response checkpoint estimate Thu 10:00–12:00; combined gates continue on each combined source head. |
| S3 / cma-seg-2 | Current legacy transcript filter is linked but lifetime bounded/segmented transcript, current state/title/usage/approval consumers and required API/client integration are incomplete. Supply actual source constructors and cross-segment/scale parity evidence. | Complete indexed consumer checkpoint estimate Thu 10:00–12:00; qualified gates/fixes by 18:00. |
| S4 / cma-seg-lead | Two lint locations: real writer_reconcile caller wiring, all original transaction-family coverage, exact unknown-outcome receipts, independently admitted CapacityRecovery empty-batch successor for segmented source, publication/failpoint/concurrency contracts. | **Lead-confirmed writer_reconcile checkpoint Thu 01:30.** Remaining writer/capacity successor evidence estimate Thu 10:00–14:00; final fixes by 18:00. |
| S5 / cma-seg-3 | 37 lint locations await the actual admission/status/accepted-work service and worker. Publish canonical timestamp codec (`790ee7458` cited by contract), bounded two-stream discovery and constructors; remove genuinely superseded wrapper. Complete native retained-process proof producer and crash/restart/settlement tests. Shared dispatcher registration belongs to S2. | Service/codec/discovery checkpoint estimate Thu 06:00–10:00; native proof/runtime evidence Thu 12:00–16:00; final integration fixes by 18:00. This is the largest schedule risk. |

**Earliest realistic combined strict-green estimate: Thu 11-12 12:00–16:00 SGT**, conditional on executable S5 constructors by 10:00, S4 reconciliation by 01:30, S1 schema completion, and a usable shared build slot. Removing warning locations alone may happen earlier; it is not PR-ready. Earliest complete PR-ready estimate is **Thu 18:00–22:00**, after real Mongo, bounded-consumer and runtime/compatibility gates. At 08:00 publish actual pins, counts and revised remaining times; do not carry these provisional estimates forward as facts. Any new cross-slice type/schema/semantic/test failure is relayed immediately with its owner.

### Delivery and stabilization through Sun 11-15

Target review Thu evening; source merge Fri 11-13 by 06:00 only after exact-head review/lead authorization and freeze lift. Fri deployment depends on a fully green batch artifact; if artifact/review timing misses that train, use the approved Sat train and state the reduced stabilization margin. No production action is performed by this integration owner.

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
