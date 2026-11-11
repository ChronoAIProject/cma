# Combined CMA strict-lint wiring assignments

**Current checkpoint, Thu 00:18 SGT:** combined committed and pushed source `21333475c`. Normal merges now include develop `8d59f6154`, S1 `fa67a1409`, S4 `e363f788f`, S5 `bcbcc43da`, and S3 `c19e717d1`. Strict CMA is queued under the existing heavy-1 lock with the full 3600-second acquisition allowance; no gate has run on this head yet. Earlier exact-source counts remain below, not current-head passes. Real production recovery commands and strict default-off configuration are composed. Background accepted-work scheduling, restricted connection-loss routing, native proof, and the bounded foreground consumers remain incomplete. All four P1 activation gates remain open. One combined default-off PR remains the plan; no integration PR is ready.


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


## Historical combined gates at 2026-11-11 22:51 SGT; current PR plan below

Source candidate **`2cf7d3fe6`** includes the accepted checkpoint, narrow S2 operator/migration dependency `b11107979`, S5 format-2 contract `daf605ab4`, and S1 `41ef81cab`, via normal merges. The only integration correction removes a duplicated production module declaration. Latest fetched S3 is `33d878350`, S4 is `191afaa86`, and S5 implementation is `1481b1d7f`. Those implementation checkpoints are already in the candidate; the later S5 contract is additive data, not the completed codec or worker. This is historical source; the merge freeze was subsequently lifted at22:58SGT. Current combined source/develop are named at the top.

| Gate at 2cf7d3fe6 | Exact result / evidence |
| --- | --- |
| Formatting / source size / runner harness | `cargo fmt --all --check` passed; source size passed for **6,014 files**; all **31 mocked Mongo runner infrastructure tests passed**, zero failures/skips. Mocked tests are not actual Mongo evidence. |
| Strict CMA Clippy, all targets/features, locked, warnings denied | Exit 101; **39 distinct dead_code locations, zero type errors**. S5 owns 37 (35 service/record/receipt locations, TransactionScope::insert_one, obsolete maintenance live_activities wrapper); S4 owns 2 (writer_reconcile Outcome and reconcile). This replaces the historical 33 count; the increase reflects added unconsumed receipt/witness code. |
| Layer 3 affected adapter contracts | **22 passed, 657 skipped**, exit 0, at this exact head. Includes all seven restricted recovery tests. Model/adapter evidence does not establish a production native producer. |
| Actual Mongo contracts | **187 selected:182passed,5failed** at `2cf7d3fe6`, all5 serial partitions attempted; all owned fixtures removed. Four writer/bootstrap failures and one recovery immutable-carrier refusal failure are recorded later. Newer source awaits rerun. |

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
8. Repository structure/source size, secret scan, changed manifest/config parity and canonical Mongo-runner registration tests. Prepare stage/kill-switch/evidence instructions for the release owner. Review at the exact head plus lead merge relay are mandatory; the develop freeze is currently lifted. After merge, monitor exact develop push CI and fix forward/revert a real failure within one hour; release requires its green exact-batch artifact.

### Remaining blockers and owner checkpoints

These ETAs are **integration planning estimates**, except S4's lead-confirmed 01:30 handoff. They are not claimed engineer commitments or test results. Reforecast after each pinned delivery/gate.

| Slice / owner | Concrete remaining blocker | Next checkpoint / ETA (SGT) |
| --- | --- | --- |
| S1 / cma-cost | Operator/format-2 pins consumed; canonical schema/checksum/nonunique-index comparison and query-plan tests are integrated through `fa67a1409`. Remaining: combined real-Mongo qualification, any reader/audit failures, and actual rollback artifact qualification. | Source delivered. Mongo/reader evidence Thu08:00–12:00 estimate; final compatibility fixes by18:00. |
| S2 / cma-db-contention | Runtime authority, strict flags, authenticated command composition and scoped root dispatch are implemented. Remaining: authenticated Mongo security gate; accepted-work scheduler/loss wiring; indexed long-turn/segmented response paging; combined integration qualification. New retained-reopen proof gap requires S5/#1652 coordination. | Fixed-head gates queued now. Background wiring Thu06:00 estimate; response source Thu10:00–12:00; qualification by18:00. |
| S3 / cma-seg-2 | Legacy transcript and usage source/config are integrated. Segmented transcript/current-state/title/usage/approval consumers remain incomplete; no lifetime fallback may be used for segmented history. | Indexed consumer source Thu10:00–12:00 estimate; parity/scale and API gates by18:00. |
| S4 / cma-seg-lead | Ordinary publication roots, reconciliation and durable keyed-attempt fence are integrated through `e363f788f`. Remaining: reproduce/fix the four earlier bootstrap/keyed/unknown Mongo failures, qualify all roots and admitted capacity successor, and cover newly added retained-reopen consumer before activation. | Lead-confirmed01:30checkpoint; combined root/unknown/concurrency gates Thu10:00–14:00 estimate, final fixes by18:00. |
| S5 / cma-seg-3 | Format2codec, bounded discovery, commands, receipt-bound background port and completion retry are integrated through `bcbcc43da`. Remaining: consume exact live registry pin, native proof producer, real crash/ack/restart/security tests, bounded retained-reopen proof interface with #1652 owner. S2 owns scheduler/shared loss routing. | Registry/native producer source Thu06:00–10:00 estimate; pinned-runtime evidence12:00–16:00; final integration fixes by18:00. Native proof is still the largest schedule risk. |

**Earliest realistic combined strict-green estimate: Thu 11-12 12:00–16:00 SGT**, conditional on executable S5 constructors by 10:00, S4 reconciliation by 01:30, S1 schema completion, and a usable shared build slot. Removing warning locations alone may happen earlier; it is not PR-ready. Earliest complete PR-ready estimate is **Thu 18:00–22:00**, after real Mongo, bounded-consumer and runtime/compatibility gates. At 08:00 publish actual pins, counts and revised remaining times; do not carry these provisional estimates forward as facts. Any new cross-slice type/schema/semantic/test failure is relayed immediately with its owner.

### Delivery and stabilization through Sun 11-15

Target review Thu evening; source merge Fri 11-13 by 06:00 only after exact-head review/lead authorization and freeze lift. Fri deployment depends on a fully green batch artifact; if artifact/review timing misses that train, use the approved Sat train and state the reduced stabilization margin. No production action is performed by this integration owner.


Later lead chronology is preserved in [the companion snapshot](2026-11-11-segments-strict-lint-history.md). This split keeps each handoff file below 500 lines.

## S4 receipt checkpoint — Thu 00:29 SGT source handoff

Published source: `5fdfae28133246ffbeb1d6bc5689513139d9cf95` on
`tl/cma-log-segments/writer-publication`, including normal develop merge `8d59f6154`.
Actual original worker/control/lifecycle and checkpoint callers now compare exact
original publication/domain receipts through `writer_reconcile::{Outcome,reconcile}`.
Durable unresolved keyed claims survive restart/later head and clear only after
local KnownAbort or atomic successful publication. No activation is enabled.

Measured previous pin `e363f788f`: writer Mongo 28 pass/5 fail; legacy compatibility
23 pass/0 fail. All new original worker/control/lifecycle-child/checkpoint receipt
contracts passed. Three bootstrap clock conflicts and two test-context defects
were corrected in `27efe61cd` / `695ca2617`; current-pin rerun acquired the existing release-only lock at00:54 after explicit
lead authorization. Shared waiters were cancelled before any child started.
Rust1.96.1, two jobs, one target; pause below10% available RAM, resume at12%.
The earlier Rust1.95 strict/unit run is diagnostic only (22 wiring diagnostics,
40 unit passes); shell bookkeeping was contaminated and is being rerun unchanged.
Canonical secret-history gate passed: 3769 commits, zero findings.

P1-1 and activation remain OPEN. Remaining S4 families and exact shared ownership
questions are recorded in `2026-11-11-s4-next-checkpoint.md`; the CapacityRecovery
segmented empty-batch successor remains additional S4 work. No missing interface
blocks the already-implemented original caller receipts. S2/S5 composition is
still required for whole-crate strict green. No PR; S2 owns the combined PR.
The actual release-slot attempt at5fdfae281 exited101 for both strict and unit
builds: two E0308 errors in S2-owned `sandbox_unpublished_recovery::transaction::load_for_inspection`
compare u64 maxima with the new Option<u64> generation. No unit/Mongo ran in that
attempt. The unused scoped export also remains. RAM stayed at least29%.
A matching 22-line correction is prepared; narrow S2 pin/ownership handoff requested.
Pushed documentation pin: `34a059faf599332c791381376c9e17abf9c25618`;
implementation pin remains `5fdfae28133246ffbeb1d6bc5689513139d9cf95`.
Final evidence is in `2026-11-12-s4-gate-evidence.md`; checkpoint status is BLOCKED
on the named S2 handoff. Canonical secret scan passed3770commits; file-size6055passed.
Cleanup removed4.847GB of owned target/tools/temp files;63.12GiB available.
All requested notes, the prepared patch, worktree and lock inodes are retained.
The final documentation head is reported in the aggregate #1350 comment.

## Post-review correction and 01:30 checkpoint

The lead handed off the minimal shared-root correction. It is applied and pushed
at `5b49c51d6e44f509d937759a4e67c8dfca9ccf8b`. This supersedes the ownership blocker
above. Optional generation is tested only for unpublished recovery; segmented
retained reopen explicitly refuses until original bounded history proof exists.
No mixed integration tip was imported.

The fresh Rust1.96.1 batch acquired the existing release-only lock at01:22SGT,
two Cargo jobs and one target, with pause below10% available RAM. Focused units,
writer/lease Mongo, legacy caller Mongo, retained contracts and strict lint are
running against this exact source. Results are recorded in the post-review gate
evidence and aggregate #1350 comment; no pending gate is counted as passing.

P1-1 remains open: v2 adoption is still incompatible with ordinary due discovery,
worker claims, command reservation and targeted approval history validation.
The real publication tests bypass discovery/claim; the required regression must
start at adoption and traverse discovery, claim, command admission and output.
P2-5 is source-corrected and awaits its corrected real-Mongo rerun. Activation
remains closed. `sandbox_reconciliation::transaction::apply_scoped` and recovery
service composition remain shared strict-lint dependencies, separate from the
now-applied type correction. Project9 remains In Progress; no slice PR.

## Qualified post-review writer evidence

At `5b49c51d6e44f509d937759a4e67c8dfca9ccf8b`, Rust1.96.1 focused units
passed40/40. Real Mongo passed36/36 writer+lease,23/23 legacy callers,9/9
retained recovery and8/8 original unpublished recovery:116 selected passes,zero
failures. The slow lost-admission-response test passed81.191s without reducing
its timeout. All four owned fixtures/volumes were removed.

Strict CMA exited101 with22distinct shared reachability diagnostics:19S5
recovery/scope and3S2 reconciliation. The optional-generation type errors and
both original S4 writer_reconcile diagnostics are gone. No allowances added.
Formatting,6055-file source size and3772-commit secret history passed. P2-5
has corrected passing Mongo evidence for closure review; P1-1 remains open.
Detailed counts, commands and hashes: `2026-11-12-s4-postreview-gates.md`.

The required fresh develop merge then consumed `92d18a909` normally at
`e094ffeea60b1aaf33510714e08795325acc8bff`, preserving both independent
feature flags in the only conflict. New source qualification runs under the
same release-only lock and memory floor; earlier results stay bound to5b49c51d6.

Merged source `e094ffeea` requalified:47distinct unit/configuration tests and76real-Mongo tests passed; strict101with22shared diagnostics andzero type errors. Evidence and source-bound counts are in `2026-11-12-s4-postreview-gates.md`.

## S4 response facts dependency

Narrow implementation commit `99f9826f4b98ff3784c03492ce877f883700526b` on `tl/cma-log-segments/response-facts`;
normal develop-merged branch pin `91b487f575dfdf60758bb4d211324a7036f4f5c5`.
Shared base `11f4fcc69`; no mixed integration tip imported. Adds required
settings_changed/legacy_model_known, response_usage/response_usage_sequence
through the next TurnStarted, and exact adjacent prewrite_failure_sequence
paired with original end_sequence. The existing publication and bootstrap fold
both populate them. S1 consumes the same facts for indexed response parity.
Rust1.96.1 Layer2 projection tests15passed,zero failures,1419filtered; strict
Layer2 all-targets/all-features Clippy passed with warnings denied. Fmt,whitespace,
5998-file size passed. Every changed file<500lines. Release-only lock,2jobs,
minimum memory20%,zero pauses. Durable missing fields fail closed; activation
remains closed. Contract:docs/notes/2026-11-12-s4-response-facts.md.

## Final S4 source and test checkpoint

Source `bc075a36aa45ac8fc826d4999748a9bcc8071c6d` is pushed on
`tl/cma-log-segments/writer-publication`. It includes the authorized shared
optional-generation correction, develop through42fbf75de, the narrow response
facts dependency99f9826f4, and a durable fact commit/abort Mongo regression.
47unit/configuration,37writer+lease Mongo,23legacy caller Mongo,9retained
Mongo and8original unpublished Mongo tests passed:124passes,zero failures.
Final strict diagnostics and cleanup are appended in the post-review evidence.
P2-5 has passing corrected Mongo evidence; P1-1 remains open at discovery,
claims, command reservation and targeted approval. Activation stays closed.
No missing interface blocks the completed original receipt callers. Shared
strict composition still requires sandbox_reconciliation::transaction::apply_scoped
and agent_capacity_recovery::{admission,authority,records,status} through
TransactionScope::insert_one. No slice PR; integration owns the combined PR.

Final strict CMA at `bc075a36a` exited 101: 22 shared reachability diagnostics, zero type errors, no S4 receipt diagnostic. The 124-test batch completed at 02:21 SGT. Minimum available RAM 25%, no pause. Final source-bound gate evidence is in `2026-11-12-s4-postreview-gates.md`; the 01:30 qualification deadline was missed and reported while the build continued.

## Lead reassignment: scoped runtime reconciliation (Thu 11-12)

The lead reassigns the S2 scoped reconciliation production wiring to S4. Exact
path boundary recorded before implementation; S2 has been informed by the lead.

- S4 takes `api/crates/cma/src/store/sandbox_reconciliation.rs` production
  `recover`/`apply_attempt` dispatch, retaining S2's existing scoped dispatch.
- S4 takes `sandbox_reconciliation/transaction.rs` receipt composition,
  `transaction/scoped.rs`, `agents/scoped.rs`, and `reconciliation.rs` exact
  original publication/domain reconciliation.
- S4 adds focused tests beneath `sandbox_reconciliation/` and their local module
  registration. Canonical Mongo selector registration is limited to this root.
- S4 consumes `agent_capacity_recovery::source::capture_segmented_scoped` through
  the actual root; its existing signature and scoped proof checks are preserved.
- `TransactionScope::insert_one` has a real consumer in
  `agent_capacity_recovery/records.rs::insert`. That consumer and
  `agent_capacity_recovery::{admission,authority,records,status}` stay S5-owned.
  S4 will neither remove that API nor create a fake consumer to silence lint.
- S5 owns bounded `agent_capacity_recovery/lineage.rs`; its existing accepted
  count/byte-bound correction is a dependency for activating this root. S4 will
  reuse the published correction without changing its bounds or semantics.

S2's existing integration dispatch is reused; no mixed integration tip is merged.
Publication receipts must survive scope release and join the root's original
commit/abort/Unknown handling. Unknown or absent/foreign receipt never permits a
new plan or success. Gates use Rust1.96.1, two jobs, one target and the existing
release-only lock with memory pause below10%. No PR or activation is authorized.
