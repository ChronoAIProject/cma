# Home restore compatibility reader

状态：**未交付**。本文记录 #1473 的 PR-2 reader 扩展及尚待确认的接口。
代码、测试或合并均不证明已经具备备份、恢复或灾备能力。本 PR 不包含恢复
准入、恢复 writer、配置开关或部署动作。

## Scope and durable interfaces

The initial baseline is `develop@23051b4c01fec7edf41236738717482ac6bf7bec`.
The branch also merges `develop@86629438414485a9991bade0f41dcde511c60920`
before final local verification; earlier results do not establish that merged baseline.
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
For `present` and `unbound` evidence, both the observed storage generation and
replacement attestation must be retained and match exactly, as required by
R2-DESIGN §3.3. Contradictory pins are rejected even when a completion receipt
exists. This validates persisted authority and adds no admission path.

An incomplete pin encountered by the ordinary lifecycle worker must wait before
funding, credential, runtime-adoption or ordinary revive effects. A receipt whose
operation ID still matches the current lifecycle operation also waits: publishing
that receipt does not prove the operation's final publication has finished. The
typed failure is `home_restore_wait_for_compatible_worker`, using existing fenced
retry publication and preserving the lifecycle operation and pin. Completing a
restore is reserved for a later compatible worker. A historical completed receipt
does not block a later ordinary lifecycle operation with a different operation ID.

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
| Upgrade, nonexecuting proof and metrics | `upgrade.rs`, `nonexecuting.rs` and `metric_observation.rs` also call `prove_native_home`; their shared proof must interpret the same retained origin without initiating restoration. |

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

The smallest proposed persisted extension is **only** an optional
`NativePoolHomeOrigin.restore`, carried through the existing
`HomeVolumeReceipt.native_authority`, with this candidate field set:

```text
restore: Option<Box<NativeHomeRestoreOrigin>>
NativeHomeRestoreOrigin {
  restore_id, backup_id, operation_id,
  source_snapshot: { name, uid },
  restore_template: { name, uid, spec_sha256 }
}
```

All leaf fields above would be strings. The adapter fixes the snapshot group/kind
and uses the existing namespace authority. Existing outer origin fields already
pin the new Claim/member/volume UIDs and home request; duplicating those UIDs or
adding a second restore field on the receipt would introduce redundant authority.
The template digest must cover datasource, layout, image and the approved restore
initializer. This is a proposal only; neither its model nor its writer exists.

For old origins, an absent optional field must serialize identically, including
field order, with no new null/default placeholders. Attachment identity hashes
serialized JSON, so old origin/receipt/attachment/annotation bytes and identity
digests need golden compatibility tests. The historical request digest must also
stay unchanged. New restoration creates new allocation authority and identity;
it must never retrofit provenance onto the old home.

A datasource-bearing volume without exact admitted provenance must remain
rejected. Initial create/claim must prove the source UID and template bytes;
`AlreadyExists` needs the same complete proof. Later ordinary readers must prove
the retained new volume UID and source/template authority without requiring the
backup source to survive retention forever. Missing, unknown, ambiguous or
contradictory source, template, namespace, owner, layout, generation or UID context
must refuse use and cleanup. Inventory must not skip such a home or infer absence.
The source-retention proof and template-routing rules still need explicit design
approval along with the persisted extension.

User/design-owner approval is required to choose the provenance contract and
its reader scope. No option has been implemented. `validate_restored_volume`,
real snapshot-derived home pause/revive tests and a final deployable reader-floor
claim remain blocked on that decision. Any additional fields inside the approved
three shapes must also be settled before declaring this reader floor deployable.

## Task-card review: validator and continuity remain blocked

This review binds code references to `76c5f67d36e3ac56c13aab935357c6f725101462`
and design references to `a96cec3da3a26326dbed5fcf2d7f45c8dea1a9ea` on
`feat/1473-backup-capture`. Design paths below are under
`docs/plans/home-backup/`; they are not copied into this PR.

Re-reading the approved R1/R2 does not remove the task-card requirements.
`IMPLEMENTATION.md:355` requires the restored volume's datasource to be exactly
the pinned backup, with every other native-volume check retained.
`IMPLEMENTATION.md:607` explicitly says decoding metadata alone is insufficient:
the reader must pause/revive an already restored home. The PR-2 row at
`IMPLEMENTATION.md:626` includes this validator. R1 §9 (`R1-DESIGN.md:188`)
limits new durable shapes to three; R2 §3.3 and §4.1
(`R2-DESIGN.md:96`, `R2-DESIGN.md:120`) add original-home evidence and admission
identity, without defining how every later provider reader obtains the new
volume's snapshot/template provenance. R2 evidence concerns the **old** home UID;
it cannot be interpreted as the restored volume's source UID.

The following differences prevent completing that behavior under the current
approved reader contract. Code paths are relative to `api/crates/`.

| Required behavior / possible existing carrier | Code fact and exact location | Unresolved contract |
| --- | --- | --- |
| Accept the exact pinned datasource and preserve other native checks | `layer3-adapter/src/agent_sandbox/pool/native_home.rs:227` accepts only `(volume, origin)`; `:263` and `:264` reject both datasource fields. `pool/home_origin.rs:10` contains only Claim authority and member/volume UIDs. | There is no expected snapshot authority in either argument. Reading a name from the observed PVC and comparing it to itself cannot prove the pinned backup. |
| Recognize the restore-specific immutable template | `layer3-adapter/src/agent_sandbox/pool/home.rs:215` reconstructs the ordinary template; `pool/model.rs:210` removes its datasource for native modes. `home.rs:237` rejects a mismatching native template. | A volume-only exception would still fail the member proof and would not authorize the restore initializer/template. |
| Reuse existing policy/template fields without a new shape | `layer3-adapter/src/home_volume.rs:107` requires native `template_name` and `template_generation` to be empty. `pool/model.rs:291` checks the binding digest and derived pool/template names. `home_volume.rs:143` hashes the policy; `:225` hashes the serialized attachment. | Filling these fields or selecting the legacy mode changes an existing strict policy, ownership/layout and identity contract. It is not an already approved representation of a restored native home. |
| Carry the retained lifecycle receipt to ordinary pause/adoption | `cma/src/sandbox/worker/lifecycle.rs:58` adopts the bound runtime; `cma/src/sandbox/runtime_mapping.rs:81` projects only `lifecycle.home.attachment()`. `layer3-adapter/src/agent_sandbox/native_runtime_proof.rs:89` receives only the attachment and `:99` invokes `prove_native_home`. | `lifecycle.home_restore` never crosses this boundary. `layer2-core/src/sandbox/home_restore.rs:51` retains completion/history, not provider snapshot/template authority. |
| Verify/revive/resume the restored home after completion | `cma/src/sandbox/worker/lifecycle/home_recovery.rs:75` calls `verify(&attachment)`; `layer3-adapter/src/agent_sandbox/pool/native_home_store.rs:144` receives only that attachment. `pool/native_home_recovery.rs:89` checks `member_spec`, then `:98` builds the native attachment. `suspension.rs:135` receives only the attachment and `:149` uses `prove_native_home`. | Later ordinary operations cannot distinguish admitted restoration from an unadmitted datasource-bearing home. A completed lifecycle receipt alone does not reach these readers. |
| Inventory/retire/clean up without a Sandbox lifecycle document | `layer3-adapter/src/agent_sandbox/pool/home_inventory.rs:83` obtains origin from the Claim and `:127` recovers the attachment. `pool/retirement_inventory.rs:32` proves retained homes and `:167` directly validates observed volumes. `pool/cleanup_inventory.rs:84`, `pool/drain.rs:93` and `pool/cleanup.rs:242` also call the validator directly. `pool/native_home.rs:294` can read origin from the volume annotation. | These readers have no lifecycle receipt or backup-catalog argument. A context-only solution needs a separately approved complete resolver/enumeration contract, including missing records and superseded homes; the adapter cannot import Layer 2. |

A pure helper with an explicit expected-source argument could compare a PVC,
but no approved rule supplies that argument to all these consumers or renders
the corresponding retained template. Adding an unused helper or testing only a
caller-supplied source would not meet the continuity requirement. No such helper
is presented as delivery. This finding does **not** establish that an additional
persisted shape is the only solution: the context-propagation option above also
needs a design decision and adapter-port changes before implementation.

The task-card items have these separate outcomes:

1. **Old/new strict reading: implemented and tested.**
   `layer2-core/src/sandbox/home_restore_tests.rs:27`, `:38`, `:113` and `:162`
   cover old absent fields, new pin/completion receipt, malformed shapes and the
   new event. `cma/src/store/sandbox_mongo_integration_tests/persistence_tests.rs:5`
   adds the real Mongo round-trip/replacement/event/unknown-field rejection test.
2. **Pinned ordinary revive typed wait: implemented and tested.**
   `cma/src/sandbox/worker/execute.rs:12` guards before lifecycle effects;
   `:26` returns `home_restore_wait_for_compatible_worker` with the existing
   waiting retry kind. `worker_tests/home_restore_reader_tests.rs:16` covers
   every revive phase, and `:99` distinguishes its own unfinished operation from
   a later ordinary revive with a historical completed receipt.
3. **`validate_restored_volume`: not implemented; blocked** on the authoritative
   source/template contract and its delivery to every reader above. Ordinary
   native validation remains unchanged. A missing or contradictory proof must
   continue to reject; there is no datasource-presence or receipt-presence bypass.
4. **An actually restored home's successful pause and revive: not tested;
   blocked** on item 3 and template proof. Existing fake-backend tests of a
   historical completion receipt do not create a snapshot-derived home or
   exercise provider validation and are not evidence for this requirement.
   Runtime startup additionally remains subject to R1 §7 V1
   (`R1-DESIGN.md:149`); its reported failure is a separate blocker, not a reason
   to remove the adapter continuity requirement.

The related implementation stops at these boundaries. Choosing a persisted
provenance extension, redefining existing strict policy fields, or redesigning
the resolver/port contract requires design-owner approval. None is implemented
by this review. The PR remains partial and **the complete reader floor remains
未交付**; PR-4 activation cannot rely on it.

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
- Retain the PR's local/CI results and version limits, including skipped-suite
  classification. Local fixtures do not replace the deployed-artifact matrix or
  production/provider continuity evidence.
- Obtain immutable candidate and recovery image identities and reader/drain
  receipts. **PR-4 activation must follow PR-2 by one deployed release**.
- R1/R2 admission semantics are approved for development; the external D3/D14
  decisions and R1/R2 runtime/provider demonstrations remain prerequisites of the
  later writer. This PR does not decide them or certify TC-087/TC-088.
