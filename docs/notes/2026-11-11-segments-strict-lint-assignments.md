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

Ledger track B has already generalized the same migration files on `tl/cma-ledger-collector/bounded-collection`: registry introduction `a1dac595d`, test harness `ea171fe6a`, invocation type `cc4e51a5c`. Reuse that registry; do not independently rewrite `ledger.rs` or create a second ledger. Requested relay to track B: publish a narrow reusable registry checkpoint, or identify the smallest mergeable checkpoint, and reserve the following additive segment dispatch extension for S2 after that pin. S2 retains `migrations/agent_log_segments.rs`; S1 retains schema internals.

The existing Step API only describes nonunique index creation. Segments require exact collection validators plus unique indexes. The shared runner extension must preserve old Activity and LedgerCollection IDs/checksums/receipts, the same global fenced lease and unknown-history refusal; permit explicit schema units from `agent_log_segments::{inspect,apply_next}`; renew before/during every effect; and publish completion only after pending_count is zero. No startup DDL. The displayed effect count must be named schema steps for this migration, not created indexes. Plans and status remain read-only. S1 must include every schema child source in the immutable checksum once the validator/index inventory is finalized.

### S5 native confirmation ownership

Allocate new focused Layer 3 capacity-recovery contract/hosted implementation/test children to S5, together with its existing restricted reconnect and registry children. S2 owns shared trait/export/module registration; S5 supplies the minimal patch against those shared files for integration. Preserve Layer 3 neutrality: no Layer 2 RecoveryClaim/RecoveryConfirmation type may enter a Layer 3 API. The lower-layer request/result must carry the exact original native/home/runtime identity, writer fence, current independently observed native state and active input identity. S5 maps that to its existing Layer 2 confirmation.

S5 must identify any required Layer 4/provider route extension before editing a shared runtime dispatcher. New focused provider children can be assigned to S5; shared dispatcher changes must pass through the runtime integration owner. Unsupported native proof is a typed refusal. The ordinary settings/context replay or PublishConnection path is never recovery fallback. No inferred Idle/Working/WaitingApproval from an Agent aggregate, empty channel or original open receipt alone. A real pinned-runtime test is required before production recovery is advertised.

S5 also owns success/foreign/delegated/refused/no-store HTTP tests and API documentation for its dry-run REST child now mounted by S2. S2 retains only the shared composition registration.
