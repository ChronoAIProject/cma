# Ledger maintenance release runbook

The release owner executes this runbook with reviewed immutable binaries and the
approved production access procedure. It does not grant permission to stop a
running owner's work. Track A supplies the commands and evidence checks below;
physical writer exclusion and home disposition remain release/owner decisions.
All private artifacts stay outside GitHub. Publish only counts and hashes.

**Current execution boundary:** read-only census, inventory and preview, detached
ledger backup/archive preparation and verification are executable. Real-home
maintenance admission, physical fencing, activation, apply, restore and release
have no qualified end-to-end command in this slice. Stop at those gates. Never
substitute a database edit, Pod deletion, format-marker file or a Boolean proof.
The internal V2 completion core is not a production maintenance command.

## 0. Pin the train and the cohort

Use the approved release procedure in [Release](Release.md). Pin the exact green
develop batch and candidate/recovery image digests from its CI artifacts. A green
PR run or another develop head is insufficient. Confirm the current merge freeze
with the lead before any merge; this runbook does not override a freeze.

Retain an evidence directory on independently protected storage, separate from
any home/PVC or checkpoint object that retirement can delete. Its access and
retention policy must already be approved. Use a private 0700 parent and 0600
files. Do not expose credentials in arguments, shell tracing, logs or notes.

The following snippets use bash, stop on errors, never overwrite output files,
and run only where the release owner has already provided the approved access,
exact binaries, original runtime configuration and qualified read view:

```bash
set -euo pipefail
set -C
set +x
umask 077
: "${LEDGER_EVIDENCE_DIR:?existing private evidence directory}"
: "${LEDGER_API_BIN:?absolute pinned service executable path}"
: "${LEDGER_RUNTIME_BIN:?absolute pinned runtime executable path}"
: "${LEDGER_COHORT_REQUEST:?absolute private census request path}"
```

The variable names are operator inputs, not new application configuration. Do not
set a different HOME or invent runtime identities. Preserve the actual original
Sandbox/home/generation and home/state/transfer mappings. A private release record
must bind image digests, executable hashes, versions, roots, UID/attachment chain,
maintenance operation/epoch, authoritative revisions and sample times. Plain
pathnames, mutable image tags or a Pod name are insufficient identities.

## 1. Refresh the failed-Agent cohort without effects (E0)

Resolve every intended cohort member from the retained private incident evidence.
The original aggregate contained 63 ledger-failed Agents; that is a historical
count, not current availability or eligibility. Do not reverse hashes or silently
omit unresolved members. Follow the exact request schema in
[ledger integration](CMA_Ledger_Integration.md).

```bash
"$LEDGER_API_BIN" --ledger-census "$LEDGER_COHORT_REQUEST" \
  "$LEDGER_EVIDENCE_DIR/census.json" \
  > "$LEDGER_EVIDENCE_DIR/census-summary.json"
```

The command requires existing authorized Mongo configuration. It performs bounded
reads in an aborted snapshot transaction, without service initialization or DDL.
The private per-member output classifies diagnostic cause, Agent/parent state,
pending cleanup/input and preservation requirements. Keep unknown and changed
members. Reconcile requested = assessed + unknown, and separately count unresolved
historical identities. C consumes the private report for recovery cohorts.

Retain the complete home, original operation keys, ledger evidence, files/Git
state, conversations, configuration and artifact references. Failed Agents are
recovery subjects, not garbage. Missing conversations do not prove no files or
effects exist. Retiring parents cannot silently become Active; restarting capacity
repair may resume their scheduled cleanup. Obtain the owner's explicit disposition
before any such restart. If completing retirement, use an independently authorized
canary for create/delete verification, not a retired parent.

## 2. Measure both stores and preview only

On the approved nonmutating view, with the original runtime configuration:

```bash
"$LEDGER_RUNTIME_BIN" --inspect-ledger-v1 \
  > "$LEDGER_EVIDENCE_DIR/v1-inventory.json"
: "${LEDGER_SANDBOX_ID:?exact private authoritative Sandbox identity}"
"$LEDGER_API_BIN" --ledger-retirement-snapshot "$LEDGER_SANDBOX_ID" \
  "$LEDGER_EVIDENCE_DIR/observation.json" \
  > "$LEDGER_EVIDENCE_DIR/observation-summary.json"
```

Record `snapshot_sha256` from the export summary independently, then use that
exact value (do not recompute a changed file and silently accept it):

```bash
: "${LEDGER_OBSERVATION_SHA256:?independently retained snapshot_sha256}"
"$LEDGER_RUNTIME_BIN" --preview-ledger-retirement \
  "$LEDGER_EVIDENCE_DIR/observation.json" "$LEDGER_OBSERVATION_SHA256" \
  > "$LEDGER_EVIDENCE_DIR/preview.json"
```

This is the dry-run path. It reports counts/hashes and leaves sources intact.
`source_mutated: false`, `apply_authority: false` and required stale replay
protection must remain explicit. No candidate count grants cleanup authority.
Unknown, live, Failed, Started or Indeterminate records remain preserved.
A concurrent-change, malformed-record, permission or inventory-bound refusal is
UNKNOWN, not zero occupancy. Never run normal startup or mutating validation to
obtain a measurement. Two equal samples do not establish exclusion.

For V2 use the existing authenticated RuntimeStatus diagnostic transport with
`Accept: application/vnd.cma.runtime-ledger-status.v1+json`. It returns committed
accounting; do not query or edit the SQLite files manually. A baseline/legacy
response has unknown counts. It cannot be relabelled a V2 observation.

Include all retained homes and unresolved/unreachable homes in the coverage
count. V1 threshold is journal >=717 of 1024 **or** authority >=2868 of 4096.
Journal temporary files consume its scan count; historical generations have
separate preservation obligations. Capacity refusals require attention regardless
of percentage. For V2 use outstanding/limit and
`(allocated_bytes + reserved_bytes + transaction_headroom_bytes)/maximum_bytes`;
report spent emergency reserve separately. V1 lifetime limits do not apply to V2.

Take two comparable samples at least 30 minutes apart, with home/generation/format
and counting semantics unchanged. For each store, positive growth per hour gives
`max(0, (bound-current)/rate)` as a scenario. One sample or zero growth has unknown
ETA. Decreases/reset invalidate the rate; do not average them into negative growth.
Use the maximum recent comparable positive interval rate and record sample count,
window, remaining margin, 85% and hard-bound scenarios. Report enumerated,
measured, unknown and over-threshold totals. Do not claim a safe-until date.

## 3. Establish durable admission and physical exclusion (E1–E4)

**HOLD until the qualified release mechanism and its exact commands are supplied.**
The current observer and detached archive commands cannot perform this step.
Maintenance Requested is not Excluded. C's structural binding is not a permit.
The frozen execution binding includes the actual exclusion-receipt hash. E1
therefore needs a separately versioned pre-exclusion admission artifact, followed
by an atomic hold-preserving transition to the immutable execution binding after
E2/E3. Do not use a placeholder hash or mutate already accepted plan bytes. That
bootstrap and its complete writer integration remain unqualified.

The release procedure must supply all of the following evidence, in this order:

1. E1: durable, transactional admission fencing for all 18 writer classes in
   [the recovery contract](CMA_Ledger_Recovery.md), across old/current API and
   controller processes, restart, takeover and lease expiry. Include checkpoint
   object purge and home cleanup. A separate pre-check is not an admission fence.
2. E2: enumerate already-accepted runtime/provider work and establish finality
   tied to request identity, original UID/generation and barrier epoch, including
   lost responses and delayed creates, resumes, mounts, exec and delete work.
3. E3: the qualified provider/storage mechanism excludes every old writable
   consumer/handle of home, state and transfer roots, including aliases, surviving
   same-node handles and a partitioned node. RWO, stopped/absent Pods, quiet logs,
   timeouts, cancellation and a local file lock are insufficient.
4. E4: acquire the exclusive maintenance authority on those exact roots after
   E1–E3; re-observe authoritative state/source under it. Any changed binding,
   epoch, mount, generation, digest or eligibility invalidates selection.

Qualify delayed pre-barrier work, old-controller restart, partitioned writers,
maintenance crash/cancellation, takeover and delayed work after release on a
separate disposable home using the same versions/storage topology. Record exact
commands and results privately. Unknown finality stays held. Do not create
production mounts or change infra/policy merely because this runbook requires proof.

## 4. Retain and rehearse recovery before source work (E5)

Keep an independently verified **full-home** backup outside all cleanup authorities
and the PVC to be retired. A ledger-only plan is not a full-home backup. Pin the
backup manifest/content, complete root mapping and retention boundary; qualify
read/restore against the candidate and approved recovery artifacts on disposable
storage. Already-issued checkpoint deletion must be final or fenced too.

On an already qualified view, the existing V1 planner and detached archive builder
can prepare the ledger portion. They never activate or reclaim a real source:

```bash
"$LEDGER_RUNTIME_BIN" --plan-ledger-v1 "$LEDGER_EVIDENCE_DIR/ledger-plan" \
  > "$LEDGER_EVIDENCE_DIR/plan-summary.json"
: "${LEDGER_PLAN_SHA256:?independently retained plan_sha256}"
"$LEDGER_RUNTIME_BIN" --verify-ledger-plan \
  "$LEDGER_EVIDENCE_DIR/ledger-plan" "$LEDGER_PLAN_SHA256" \
  > "$LEDGER_EVIDENCE_DIR/plan-verification.json"
"$LEDGER_RUNTIME_BIN" --stage-ledger-retirement \
  "$LEDGER_EVIDENCE_DIR/ledger-plan" "$LEDGER_PLAN_SHA256" \
  "$LEDGER_EVIDENCE_DIR/observation.json" "$LEDGER_OBSERVATION_SHA256" \
  "$LEDGER_EVIDENCE_DIR/retirement-archive" \
  > "$LEDGER_EVIDENCE_DIR/archive-summary.json"
: "${LEDGER_READY_SHA256:?independently retained ready_sha256}"
"$LEDGER_RUNTIME_BIN" --verify-ledger-retirement \
  "$LEDGER_EVIDENCE_DIR/retirement-archive" "$LEDGER_READY_SHA256" \
  > "$LEDGER_EVIDENCE_DIR/archive-verification.json"
```

Use new output paths. Retain the original plan and observation. An incomplete
output without valid ready evidence is unusable; rebuild at a new path from the
same pinned input. Re-running at an existing path refuses without overwrite.
These artifacts remain `retirement_held`, `writer_exclusion: unproved` and
`source_deactivated: false`. They do not reclaim capacity. The V1 observation
and bounded V2 collector batch have different protocols; never substitute one.

## 5. Apply, restore, release and verification (E6–E8)

**HOLD: no production commands for this stage are advertised by this slice.**
A reviewed integrated C coordinator must supply the exact invocation and accept
qualified E0–E5 evidence before this runbook can advance. Do not call an internal
Rust primitive or manufacture a descriptor to bridge this gap.

The coordinator must retain original accepted bytes before Selected, publish
required archive/replay dependencies before source changes, and preserve the
hold through cancellation and storage-worker completion. For V2, archive version 3
completion retains one pending transition and the original immutable final receipt;
source rows and accounting commit together. Retry uses the same operation, binding,
plan and exact bytes. Never refresh observation time to disguise a new plan as
retry. A cold acknowledged result survives later groups. Archive exhaustion
applies backpressure; archival does not mean unlimited physical storage.

On any unknown/error/lost response, keep the independent backup, original artifacts,
physical exclusion and serving hold. Inspect through the reviewed coordinator and
retry the same operation only after its exact state is reconciled. Neither missing
rows nor matching counters proves completion. Do not automatically restore or
start an older image. The descriptor and archive versions must agree; old archive
readers refuse version 3. Exact recovery images still need qualification.

The restore path is a **separate held operation**, admitted only after C verifies
original/after receipts and independently proves no newer filesystem, conversation,
provider or external effects (E7). SQLite after-state alone cannot prove this.
Restore must use retained original bytes, keep permanent replay refusal and the
serving hold, and refuse changed or uncertain destinations. If newer effects exist
or cannot be excluded, retain the hold and pursue compatible forward recovery or
explicitly authorized migration. The full-home backup/restore rehearsal is the
preservation path while the production restore coordinator is unavailable.

E8 requires exact source/archive/backup binding, no unfinished transition, current
physical exclusion, the qualified candidate/recovery pair and the owner's relayed
disposition. Only the reviewed release operation permits the new writer; stale
pre-barrier work must still fail afterward. It must not imply Failed→Ready,
Retiring→Active or automatic replay of attempted input. C must name whether the
same Agent is safely retried or preserved work migrates to explicit new resources.

The release owner then verifies the authorized create/delete canary and preservation
of the original cohort. Count a recovered Agent only after the documented recovery
path and usability check succeed. Report preserved/migrated/recovered/held/unknown
counts, both-store margins, stale-replay refusal, exact images, evidence hashes and
retention/restore result. Project status remains In Progress until acceptance and
production verification are complete; only close with the relayed evidence.

## Full-data retirement archives

The default-off full-data archive command and durable archive-before-retirement
requirement are specified in [CMA Retirement Archives](CMA_Retirement_Archives.md).
Its command, custody and reader-floor gates are additional to this runbook's
physical old-writer/provider exclusion. Production execution remains HOLD until
all actual source, permanent-custody and restore qualifications are reviewed.
