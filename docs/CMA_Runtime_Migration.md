# Explicit retained-home runtime migration

Issue: [#2064](https://github.com/ChronoAIProject/cma/issues/2064).

Custom profiles retain their original immutable image and full profile snapshot.
An operator migration adds per-Sandbox image authority, retains the original home,
Claim, native Sandbox and PVC, and uses the existing ImageRotation/native upgrade
protocol. Daily managed selection and owner runtime-update routes continue to
exclude custom profiles. Initial custom admission accepts Active or Paused only.
Failed/Retiring/application-404 dispositions are separate operations.

The source image remains authoritative while a migration is queued or suspending
its source. The admitted successor image becomes valid for inspection once that
exact successor is bound. Successful lifecycle publication commits the effective
pin. Explicit/child resume, retained recovery and later migrations resolve this
same authority. Failed work retains its phase, target and home. Automatic
unpublished recovery is restricted to the exact previously admitted custom target;
changing the global default does not retarget that recovery.

## Deployment and compatibility

`CHRONO_RUNTIME_MIGRATION_ENABLED` defaults to `false`. The existing-record readers
and lifecycle settlement remain enabled regardless of this flag. New admissions
require the enabled writer, policy and bundle paths, and the qualified single-AM
replica-OFF deployment. This release makes no multiprocess desktop-history claim.

`CHRONO_RUNTIME_MIGRATION_POLICY_FILE` names operator-controlled server configuration
with schema version 1, policy/deployment identities, exact scoped Sandbox IDs,
one approved immutable image per exact repository family, and qualified source
contexts. It is bounded to 64 KiB, 128 Sandbox IDs, 16 target families and 16 source
contexts. These are configuration inputs, never HTTP parameters. The server binds
the local desktop process incarnation at startup and starts a new observation
baseline; evidence from an older process is never inherited. Source qualification
must cover every writer, database identity, image/source revision, membership
admission and reset/retention behavior. A context's coverage enum alone does not
qualify it.

`CHRONO_RUNTIME_MIGRATION_BUNDLE_FILE` names the operator publication path for
schema-version-1 per-ID authority entries. Reads are bounded to 4 MiB and 128
entries; duplicate identities/authority IDs, unknown fields, ambiguous family
mappings and malformed data refuse admission. Publish an entire new file
atomically. Loading or retrying never renews evidence timestamps. A fresh entry
has a maximum 30-second validity and exact identity/revision/request/target guards.
The policy and bundle are installed through the release's existing trusted
operator configuration boundary; they need no new signing scheme or collection.

Before the first writer activation, roll out compatible API and worker readers
and prove the selected single-AM/OFF source context. The first protected migration
transaction writes envelope version 5 and the permanent
`runtime-migration-writer-floor-v1` sentinel in `sandbox_credentials`. Previous
readers refuse version 5; the previous startup floor also refuses the additional
sentinel even after all migrated Sandbox records are removed. This is the rollback
floor: closing the writer switch does not make rollback to an older binary safe.
Retain both the sentinel and custom authority. Generic create/save cannot install,
strip or replace the authority. Existing lifecycle transactions preserve its
identity and admission basis while completing the admitted operation.

## Operator API

`POST /api/v1/admin/sandboxes/{id}/runtime-migration` requires
`ControlAnySandbox`. The request contains the typed custom migration (request key,
original expected revision/image and immutable origin/home/profile guards), the
server authority lookup ID, current expected revision and an explicit
`restart_failed` flag. It contains no quiet interval, verifier, file path or
self-approved clearance. The service independently matches the configured approved
target, source qualification, current identity and exact server authority entry.

Admission acquires the common desktop/migration exclusion guard before its source
snapshot and keeps it through commit or unknown-commit reconciliation. It checks
all child/work/session evidence, compares the original complete activity vector,
fences the corresponding durable writers, and queues plus starts ImageRotation
in the same transaction and revision increment. There is no delayed custom queue
that can start later using expired clearance. The verified basis is stored with
the operation in that transaction, including original source/interval evidence.

An exact accepted replay can read the durable result after bundle removal or
expiry and with the writer disabled, under fresh operator authorization. It does
not run effects again. Reusing a key with another payload or authority refuses.
Custom failed-work restart requires a new authority entry and current complete
quiet evidence, while retaining the original target and operation phase. The
bounded retained history refuses further requests rather than forgetting old keys
or restart admissions when its limit is reached.

Ordinary image rotations for IDs in the configured policy scope use the same
proof/session guard at actual scheduling. Paused admissions retain full current
child, command and session checks. Ordinary policies outside that explicit scope
are unchanged. All Running fleet IDs must be in the configured scope before an
operator campaign can use this protection.

For an ordinary managed rotation carrying scoped admission, explicit failed-work
retry and explicit unpublished-runtime recovery are unavailable in this release:
the retained basis cannot authorize another attempt, and no ordinary fresh-proof
restart endpoint is implemented. Eligibility and the service refuse before
recovery inspection; the model also refuses direct explicit recovery. Exact
no-failure replay, existing automatic recovery with its original target and
once-only checks, and new ordinary requests after completion remain available.
Requests outside scoped admission retain their existing retry behavior.

## Forward observation

`GET /api/v1/admin/sandboxes/{id}/runtime-update/activity` requires
`ReadAllSandboxes`. It accepts no source, timestamp or proof assertion. It exposes
only UNKNOWN/ACTIVE/OBSERVING/QUIET plus an opaque proof reference when available;
like the existing admin Sandbox GET, it appends no operator mutation audit. The
observer does not invoke maintenance, fence, repair or migration writes as probes.

The configured server verifier retains bounded coherent metadata snapshots and
source/incarnation watermarks. A proof requires at least 900,000 ms under
conservative bounds, a final age no greater than 30,000 ms, unchanged complete
membership/activity contexts and no work/session. Every missing/unsupported source,
error, cardinality overflow, activity drift or reset refuses or restarts the
baseline. Database and desktop process/tracker replacement invalidate continuity.
The original final vector must remain unchanged through transactional admission;
30 seconds is a ceiling, never an allowed evidence gap or extra waiting period.

The Mongo adapter binds a complete UUID inventory for `sandboxes`, `workspaces`,
`agents`, `agent_command_orders`, `agent_commands`, `agent_log`, `agent_log_heads`
and `sandbox_sessions`, alongside the replica-set name, server process identity
and database name. It reads metadata only for those exact collection names,
before and after each capture and again during admission; it retains only the
identity digest. Missing collections/UUIDs, duplicate identities, unsupported
collection kinds, metadata overflow or unavailable catalog access remain UNKNOWN.
The observer creates no collections. Even a legacy-only fixture/source must have
the existing additive log schema installed before this complete inventory can
qualify. Collection replacement invalidates continuity despite identical rows.

Qualification covers the selected supported admission/publication writers; it
must explicitly exclude generic create/reparent ports and direct database tools
throughout observation and admission. Those lower-level paths do not universally
fence a parent against phantom membership. A restore that preserves collection
UUIDs requires a fresh operator source/policy generation and new observation
baseline, even if every stored counter matches. Change the configured source
`policy_id` or `source_revision` consistently through the approved configuration
path; never rewrite an existing proof or inherit its interval across the change.

The common desktop gate starts before native-open effects. In-flight opens and
connections retain a shared permit through their complete lifetime; migration
holds the exclusive permit. The process-local activity epoch records attempted,
failed and cancelled opens as well as close, including attempts that never create
a durable lease. New opens revalidate Active state and exact runtime binding after
taking their permit. A tracker eviction changes its incarnation; missing history
cannot inherit an earlier quiet interval.

A bundle entry may name an `observation_reference`; the server resolves it to the
original proof for that exact Sandbox and then independently matches the operator
entry and target. The reference is not bearer authority. A bundle can alternatively
carry evidence from a separately qualified producer. Neither carrier can make an
incomplete source complete. The memory-only synthetic quiet-proof builder is
compiled solely for tests and is rejected by the production Mongo adapter.

## Release evidence

Code or a PR is not production-source qualification, activity clearance, receiver
qualification or permission to execute a migration. Run the focused model,
service/HTTP, desktop race, native retained-home and real-Mongo transaction/race
checks on the integrated head. Qualify one controlled retained-home canary for each
published family before expanding. Runtime publication, lifecycle repair/retirement,
source activation and production migrations keep their existing owner approvals.
