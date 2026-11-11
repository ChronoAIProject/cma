# Combined CMA strict-lint wiring assignments

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
