# Home restore compatibility reader

状态：**未交付**。本文记录 #1473 的 PR-2 reader 扩展及尚待确认的接口。
代码、测试或合并均不证明已经具备备份、恢复或灾备能力。本 PR 不包含恢复
准入、恢复 writer、配置开关或部署动作。

## Scope and durable interfaces

The baseline is `develop@23051b4c01fec7edf41236738717482ac6bf7bec`.
The approved design input is R1-DESIGN §9 and R2-DESIGN §3–§4 in
`feat/1473-backup-capture@a96cec3da3a26326dbed5fcf2d7f45c8dea1a9ea`.
The task's merge-only and `Refs #1473` rules supersede the plan's historical
rebase and issue-closing language.

Only three additive durable shapes are introduced:

- `SandboxLifecycleState.home_restore`: a pin with restore, backup and lifecycle
  operation identities, admission time/operator, idempotency-key digest, original
  home evidence classification/observation time/optional observed storage
  generation, optional replacement attestation, and optional completed receipt.
  Evidence classifications are provider-neutral: `gone`, `deleting`, `unbound`,
  `present`, `unreadable`. Reading a classification does not authorize admission.
  The completion receipt retains the inventory digest, restored-to time,
  completion time, resulting home identity/source generation, covered/uncovered
  Agents, missing Workspaces/Worktrees and re-deleted child resources.
- `SandboxLifecycleState.superseded_homes`: restore identity, complete original
  `SandboxHome`, original home identity/source generation and supersession time.
  Keeping the complete allocation preserves its recorded cleanup authority.
  Validation rejects duplicate identities and reuse as the current home.
- `SandboxEventKind::HomeRestored { restore_id, backup_id, restored_to }`.

All new objects retain `deny_unknown_fields`. Missing optional lifecycle fields
read as absent/empty and are omitted on serialization. No old field, enum value,
request digest or runtime binding changes. A completed receipt is retained as
history; its presence alone does not authorize a new restoration.

An incomplete pin encountered by the ordinary lifecycle worker must wait before
funding, credential, runtime-adoption or ordinary revive effects. The intended
typed failure is `home_restore_wait_for_compatible_worker`, using existing fenced
retry publication and preserving the lifecycle operation and pin. Completing a
restore is reserved for a later compatible worker.

## Source propagation decision still required

The existing native storage contract rejects any `dataSource` or
`dataSourceRef` in `pool/native_home.rs::validate_native_volume`. The same
origin also reconstructs the immutable member template via
`pool/home.rs::member_spec`; adding a separate volume validator alone cannot
make snapshot-derived homes pass that proof. Both checks remain unchanged.

These are the observed call chains, relative to
`api/crates/layer3-adapter/src/agent_sandbox/` unless stated otherwise:

| Consumer | Existing path and available authority |
| --- | --- |
| Ordinary pause | CMA `worker/lifecycle.rs::run_lifecycle` adopts the bound runtime first; `runtime_attachment.rs::inspect_bound_runtime_after` builds `RuntimeAdoptionRequest` from `runtime_mapping.rs::sandbox_spec`. The adapter receives `SandboxSpec.home_volume`, a `HomeVolumeAttachment`, without lifecycle history. Native proof ends in `pool/native_proof.rs::prove_native_home` → `member_spec` and `native_attachment` → `validate_native_volume`. |
| Ordinary revive verification | CMA `worker/lifecycle/home_recovery.rs::verify_retained_home_and_absence` calls `HomeVolumeStore::verify(&attachment)`. `pool/native_home_store.rs::verify` → `recover_known_native_vct` → `recovered_native_attachment` → `member_spec`/`native_attachment`. The port receives only the attachment. |
| Ordinary resume | `suspension.rs::native_resume_desired` receives a `HomeVolumeAttachment`, then calls `prove_native_home` and template policy checks. It has no Sandbox lifecycle. |
| Runtime adoption/status | `hosted/adapter_context.rs::inspect_exact_after` and `hosted/agent_sandbox_context.rs::inspect_native_context_after` pass the attachment into `native_runtime_proof.rs::native_runtime_proof` → `prove_native_home`. No restore receipt reaches these readers. |
| Provision from recorded home | `pool_runtime.rs::provision_pool_runtime_with_home`/`admit_pool_runtime` receive the attachment; `prove_native_home` and template policy checks remain required. The alternative discovery path uses `volume_origin` and `native_attachment` from provider resources alone. |
| Home inventory | `pool/home_inventory.rs::Cursor` enumerates provider Claims and reads their `NativePoolHomeOrigin` annotations, then calls `recovered_native_attachment`. Neither the cursor nor its port receives Sandbox documents or restore receipts. |
| Pool retirement | `pool/retirement_inventory.rs::prove_retirement_homes` receives attachments and calls `prove_native_home`. Its later complete-volume pass calls `validate_native_volume(volume, expected_origin)` directly. |
| Cleanup inventory and draining | `pool/cleanup_inventory.rs::cleanup_members` constructs an origin from a Claim/member/volume; `pool/cleanup.rs` and `pool/drain.rs` also call the validator directly. These paths can run with provider resource authority alone. |
| Runtime deletion after object loss | `pool_runtime.rs::retirement_pool_origin` can reconstruct authority from the retained volume annotation after the member disappears. A lifecycle-only proof is unavailable there. |

Layer 3 cannot import Layer 2 or look up Sandbox Mongo documents. Consequently,
retaining a receipt under `lifecycle.home_restore` can supply evidence at a CMA
request boundary, but **cannot currently supply all readers in this table**.
The independent inventory/cleanup paths need an explicitly approved way to
resolve or carry exact restore provenance. Looking only at a live source name,
label or annotation is not proof that this particular home was admitted.

| Option requiring design review | Compatibility and unresolved work |
| --- | --- |
| Keep provenance inside the three approved shapes and pass a neutral proof/context through adapter ports | Avoids a fourth durable shape. Requires an exact source representation inside `home_restore`/its receipt, propagation through ordinary adoption/pause/revive, and redesign of inventory/retirement/cleanup APIs so every observed origin resolves against authoritative retained context. The contexts need complete enumeration, ownership/fence rules, behavior for missing/tombstoned Sandboxes and old homes, and bounded execution. This is wider than adding one validator and is not yet designed. |
| Extend persisted `HomeVolumeReceipt`/`NativePoolHomeOrigin` with restore provenance | Naturally accompanies attachments and the provider annotations already read by inventory. This is an additional durable-contract change beyond the task's three shapes; it needs explicit approval, strict-reader expansion in every consumer, digest/identity and annotation compatibility analysis, and a deployment floor before writers. It must include exact source and original/new authority, not a boolean exemption. |
| Give restored homes a separate provider mode/authority | Requires new persisted policy/ownership variants and routing in the same consumers, together with cleanup/template contracts. It also exceeds the approved shapes and needs a larger reader migration. No such mode is introduced here. |

User/design-owner approval is required to choose the provenance contract and
its reader scope. No option has been implemented. `validate_restored_volume`,
real snapshot-derived home pause/revive tests and a final deployable reader-floor
claim remain blocked on that decision. Any additional fields inside the approved
three shapes must also be settled before declaring this reader floor deployable.

## Data migration record

| Required field | Record |
| --- | --- |
| Identity, phase, owners | `2026-11-16-am-home-restore-reader`; expand only; Agent Manager owns Sandbox documents and event logs; deployment/verification operator remains unassigned. Refs #1473. Source baseline and design commit are recorded above; exact candidate executable/image is **未交付**. |
| Stores and writer triggers | Existing Sandbox lifecycle documents and Sandbox event logs. No collection, index, validator, TTL, startup DDL/scan, importer, admission route or restore writer is added. Existing whole-record writes serialize the new fields rather than discarding them. The compatibility worker may publish an existing work retry/event shape for a retained incomplete pin. |
| Format and preservation | Three shapes listed above; envelope version unchanged. Preserve IDs, ownership, revisions, operation IDs, keys, existing histories, runtime binding, home allocation and generation fences. No home bytes change. |
| Reader floor | Candidate source is this PR's commits; exact executable/image and old/candidate/recovery artifact execution matrix are **未运行**. Fixture tests cover old missing fields, new nested shapes, malformed/unknown fields, superseded-home identity and pending-operation waiting. These tests do not execute a predecessor binary. Predecessor strict lifecycle/event readers are excluded after new shapes are retained. |
| Writer gate/cohort | No restore producer exists in PR-2; no flag is introduced. PR-4 must remain disabled until PR-2's complete compatible reader is deployed at least one deployed release earlier, incompatible processes have drained, and all additional prerequisites are proven. Closing a later flag does not remove the retained reader floor. |
| Migration/backfill | No transform, backfill, migrator, startup schema operation or database command. No execution lease is needed for a nonexistent migration. Existing per-document lifecycle lease/fence governs compatibility retry commits. |
| Backup/dry-run | No destructive transform, deletion or home effect occurs, so this expand-only change requires no data-copy operation. Protected production backup and restore rehearsal are **未运行**; their absence is not recovery evidence. |
| Rollback/forward recovery | Deployment must not roll back below this reader floor; at minimum, once a later writer retains these shapes, older strict readers cannot recover them. Keep this compatible source/image and prefer a forward fix; never erase fields, rewrite old homes or downgrade records to fit a predecessor. Actual allowed image digests are **未交付**. |
| Verification/completion | Local checks and missing evidence must be reported in the PR. Source-propagation decision, full adapter continuity tests, exact artifacts, deployment/drain receipts and production sign-off remain outstanding. Production verification is not authorized by this record. |

## Outstanding delivery and prerequisites

- Finalize source propagation before completing `validate_restored_volume` and
  claiming that a snapshot-derived home survives ordinary pause/revive.
- Complete local Rust checks, affected-crate suites and the real Mongo replica-set
  ignored suite; record actual failures and unexecuted checks in the PR.
- Obtain immutable candidate and recovery image identities and reader/drain
  receipts. **PR-4 activation must follow PR-2 by one deployed release**.
- R1/R2 admission semantics are approved for development; the external D3/D14
  decisions and R1/R2 runtime/provider demonstrations remain prerequisites of the
  later writer. This PR does not decide them or certify TC-087/TC-088.
