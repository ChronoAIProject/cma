# CMA account deletion: request, status and access revocation

This release implements the account deletion record, fenced state machine,
request and status APIs, and access revocation. Accepted records remain
`running` at `inventory`: the background worker, inventory collection, physical
erasure, independent residue verification, operator routes and restore tools
are delivered by later PRs. Acceptance is not evidence that data was erased.
The production writer switch remains disabled by default.

## Request and status contract

A verified human browser session or identity bearer may request deletion of its
own CMA account. Binding keys, Agent keys, resource-request actors, identity
proxy actors and worker credentials cannot request deletion. CMA does not delete
the external identity account. Existing verified owner authority is sufficient under the pending security
decision; recent sign-in verification is not implemented. Owner-session authority
is a subject-bound request marker, separate from generic Authorization. The
`delete_own_account` wire entitlement is derived from that marker only.
It has no generic `Capability` variant and cannot be serialized into persisted
lifecycle capabilities.

`POST /api/v1/account/deletion` requires `Idempotency-Key` and the JSON body
`{"acknowledge":"irreversible"}`. It returns `202 Accepted`,
`Location: /api/v1/account/deletion`, and `Cache-Control: private, no-store`.
Replay of the original accepted key returns the existing receipt, including
after the writer switch is disabled. While requests are enabled, a different
valid key also returns the current active receipt without storing another key.
Only the original accepted key is retained, bounding per-record replay storage.
When requests are disabled, a key without its own durable acceptance returns 503. Personal POST requests reach authoritative storage even while a conservative
local denial marker exists; a previously aborted transaction can be retried
without reopening ordinary data access. Reuse cannot create a second active deletion. A request writes
the deletion, its initial audit event and the owner's admission fence in one
transaction. Explicitly aborted transactions may retry with bounded backoff;
an unknown commit result requires replay of the original key to reconcile.
An uncertain outcome also inserts the subject hash into the accepting replica's
local denial set before returning the recovery problem. A subsequent successful
snapshot reconciles that denial with durable records; an uncertain result never
creates a success receipt.

`GET /api/v1/account/deletion` returns `200` with the latest receipt or
`404 account_deletion_not_found`. It remains available when new requests are
disabled for callers that still hold valid personal authentication. An erasing
account cannot sign in again; an expired browser session cannot reauthenticate
to read progress while deletion remains active. An already valid eligible
personal bearer can still read status. This release provides no separate
recovery login. Both routes return
private, non-cacheable responses, including errors.
Receipts contain `id`, `status`, `step`, `requested_at`, `deadline_at`,
`completed_at`, and `steps`. They expose no raw subject, hash, credentials,
resource inventory or worker leases. The deadline is seven days after request;
this release does not claim the cleanup deadline is achieved.

The request switch is `CHRONO_ACCOUNT_DELETION_ENABLED`, strictly `true` or
`false`, default `false`. Disabled new requests return `503
account_deletion_unavailable`; exact accepted-key replay remains available; the session omits `delete_own_account`.
Missing acknowledgement returns `422 account_deletion_acknowledgement_required`;
non-human authority returns `403 account_deletion_requires_owner_session`.
The standard idempotency and JSON problems apply. See [REST](API_REST.md) and
the repository SDK references for the exact wire contract.

## Access revocation and freshness

The gate always reads active deletions, including when request admission is off.
Each API process refreshes its in-memory hash set once per second. The accepting
replica synchronously inserts the hash after the transaction commits and before
returning acceptance. Other replicas deny access after their next successful
refresh, normally within one second plus scheduling/network delay. If a snapshot
has not refreshed successfully for more than ten seconds, requests fall back to
authoritative point reads. Concurrent fallback checks for the same subject hash
share one in-flight read, including Agent fragments, portal chunks and launch
progress frames. Completed denials and source failures are cached for at most
one second, the snapshot refresh period; completed allow results are not cached.
A failed point read denies access with a typed service unavailability response;
absence from a stale snapshot never grants access. Explicit authoritative
transaction/admission checks bypass this fallback cache. A successful snapshot
refresh reconciles cached denials, and a locally accepted deletion takes
precedence over an in-flight allow result. A concurrent refresh that permits
access cannot weaken a refusal from an already-started authority read. Fallback
state is bounded to 256
subject hashes per process; capacity occupied entirely by in-flight reads
returns typed unavailability for another subject instead of admitting access.
Cancelling one waiter preserves the shared read for other frames; cancelling the
last waiter releases an unfinished lookup and its resources immediately. A later
check starts a new read. No detached task drains cancelled reads after shutdown.

Steady-state checks do not add a database command per request. Initial snapshot
refresh failure logs a warning and starts in authoritative point-read mode while
background refresh continues. Both snapshot and point reads deny a subject when
an active marker exists or its record status is not the scalar `completed`,
including missing, unknown or array-valued status. An explicit array branch
prevents database scalar equality from treating `["completed", "running"]` as a
completed record. Snapshot scanning unions three hinted index scans: the
active-marker partial index, the status/subject index and the array-status
partial index. Subject point reads use the subject-history index. Malformed rows are logged individually; a recoverable subject hash stays
in the denial set and does not prevent other rows from being loaded or claimed.

The gate covers:

- Ambient browser and bearer authentication, including the resource-request
  and worker-credential branches that return early from authentication.
- Sign-in callbacks and actor-link renewal.
- Event Gateway portal reads, outbound sends and each streamed body chunk.
  Custody checks deny access before refreshing or forwarding the owner token.
- Agent tool capability authentication and provider tool relay, using the
  Sandbox owner.
- Resource authorization for both the caller and personal resource owner,
  including stream reauthorization of both caller and owner, and sharing to
  other callers. Launch progress streams and desktop connection rechecks also
  revoke an erasing caller.
- Document owner, recipient and link-token reads.
- Public Trigger preview, badge and open routes; an erasing publisher's
  Trigger is unavailable. Open returns its existing 404 unavailable response,
  clears the navigation-intent cookie and retains private/no-store and
  no-referrer headers. Consuming a navigation-intent cookie does not resolve
  a publisher or admit a launch.
- Deferred Agent commands, Sandbox controls and runtime subscription credentials.

An active deletion returns `403 account_deletion_in_progress` on authenticated
CMA data surfaces. Public Trigger reads retain their existing unavailable
projection. Grants to an erasing share recipient fail transactionally with
`409 share_recipient_unavailable`; existing record revision checks still apply.
Sandbox admission, including Trigger launch admission, increments the same
per-owner fence and checks active deletion in that transaction. It either
commits before deletion admission or returns a typed rejection. Already admitted
Agent/Workspace operations are handled by later inventory and erasure work;
the inventory must be recomputed from current resource lineage.

The ordinary exceptions are `GET /auth/session`, `/auth/logout`, and
`GET /api/v1/account/deletion`. Session inspection reports `api_commands: false`
for an erasing account, including an otherwise eligible scoped OAuth credential;
standing uncertainty returns service unavailability instead of command readiness.
A personal `POST /api/v1/account/deletion` also
reaches the authoritative request service for bounded active deduplication and
unknown-result recovery. The original accepted key remains replayable after
writer shutdown; an unaccepted key requires the writer to be enabled. These
exceptions preserve inspection and progress for existing valid authentication;
they do not permit renewed sign-in or ordinary data access.
While erasing, session inspection retains authentication metadata
but omits optional profile details such as name, email and avatar. Deletion's own owner-token
custody paths, including `delegated_credential_for`, deliberately bypass the
gate so later credential revocation can execute.

A scoped key used directly against an external service bypasses CMA's gate.
Its remote revocation is delivered by later retirement/credential PRs and must
be independently measured; this release does not claim remote key revocation.

## Ordered state and later deliveries

The neutral step order is `inventory`, `retire_sandboxes`, `release_homes`,
`erase_records`, `erase_objects`, `revoke_credentials`, `verify_no_residue`.
The Issue's Sandbox, persistent home, database, document object and credential
steps map to those names in that order. Lease fences and revision checks reject
stale workers. A typed block retains the same step; rearming resumes it.
Completion is allowed only from verification, clears the raw subject and active
hash, removes resource and credential identifiers from inventory while retaining
aggregate counts, and applies the caller-supplied tombstone retention interval to the record
and its events. No tombstone retention configuration is introduced here.
Zero residue requires an explicit, parseable empty-key-list receipt; an absent
receipt is not proof of revocation. Later delayed verification must reconcile
the minimal tombstone with any provider identity evidence it needs. This release
does not execute that verification or retain a separate recovery inventory.

The following remain later deliveries: worker scheduling, actual inventory,
Sandbox retirement, home finality, record/object erasure, credential revocation,
zero-residue verification, operator retry, tombstone export/restore replay, and
real deletion drills. External provider capabilities and final product/security
policies remain pending. The suggested 37-day receipt retention and physical
retention exceptions require product approval; this release does not implement
or assert those exceptions. Organization-owned resources remain outside personal
account ownership. The irreversible request has no cooling-off period; final
policy approval is pending. Audit retry/refusal payloads use closed failure
codes; resource identifiers in block events are hashed rather than retained raw.
Lease renewal and external-state deferral increment revisions without failure
attempts or synthetic retry events.

## Activation and rollback

First deploy the compatible reader floor, with access gates, to every API
replica while the writer switch is false. Pin the exact image digest as the
minimum rollback artifact in the release record. Only a later authorized
activation may set the switch true after the remaining cleanup capabilities and
drills are delivered. This PR alone is insufficient for production activation.

Disabling the switch closes new requests; it does not remove active deletions or
turn off their gate. While active records exist, never roll back below the
reader floor. Do not remove collections, indexes, leases or audit events during
rollback. An unknown request result must be reconciled by the original key;
operators must preserve durable state rather than claim that deletion failed.

## Data migration record

| Required field                                       | Evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Release / migration identity, phase and owners       | Issue #1500, additive account-deletion reader floor. Source identity: the commit containing this document (`git log -1 --format=%H -- docs/CMA_Account_Deletion.md`). Pending: release owner records immutable image digest.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Affected stores and all writer triggers              | New `account_deletions` and `account_deletion_events`; enabled personal requests seed an owner row in `sandbox_admissions` outside the transaction, then increment its fence with deletion and initial-event writes in one transaction. Exact replay is read-only. Existing Sandbox admission, Trigger visitor admission, personal document grants, selected-user share grants and share redemption also upsert/increment per-person fences independently of the deletion writer switch. Organization IDs are never personal-account fences. Claim, commit, renewal and deferral support later workers; this release starts none.                                                                                                           |
| Launch publisher authority | A visitor Launch writes only the visitor fence. Publisher standing uses an indexed, projected `account_deletions` point read in the same transaction, without upserting or incrementing the publisher fence. Existing transaction snapshots may admit visitor-owned work after publisher deletion is accepted; later erasure removes publisher counters and other publisher-owned residue. A fresh snapshot refuses an erasing publisher with typed `TriggerRetired`; visitor deletion still serializes with Launch admission. Validate with `fresh_launch_fences_only_the_visitor_and_reads_publisher_authority`, `publisher_deletion_refuses_fresh_launch_without_changing_admission_counts`, `visitor_deletion_committed_before_fence_retries_and_refuses_launch` and `publisher_deletion_does_not_serialize_an_existing_visitor_snapshot`, using guarded `cargo nextest run --locked -p cma --run-ignored only -E 'test(store::trigger_definitions::launch_mongo_integration_tests::account_deletion::)'` against the isolated replica set. |
| Old/new format and preservation contract             | Existing owner fence shape is preserved. Shared fence writes serialize conflicting admission/grant/deletion transactions; contention may cause bounded abort/retry and latency even with deletion admission disabled. Two new Independent census collections have active/replay/due indexes, unique `(deletion_id, seq)` events and BSON date TTL. No object-inventory collection exists. `AccountDeletion` and nested state use `deny_unknown_fields`: future field additions require compatible readers and schema-version handling before writing; the current reader floor must not be assumed to ignore new fields. Corrupt rows are warned and skipped individually, with recoverable subjects denied.                                                              |
| Standing index addition                              | Startup idempotently creates two indexes on `account_deletions` before serving hinted standing scans: `account_deletions_status_subject`, keys `{status: 1, subject_hash: 1}` with no partial filter; and `account_deletions_array_status_subject`, keys `{subject_hash: 1}` with partial filter `{status: {$type: "array"}}`. Both are nonunique with no TTL. Keep the existing active-marker partial and subject-history indexes. These additive indexes deny malformed noncompleted and array-valued status records without scanning completed history; no row backfill or schema change is required. Retain indexes during rollback. Validate both `IXSCAN` plans and bounded examined keys/documents with `standing_queries_use_indexes_and_project_only_authority`. |
| Authorization preservation                           | Personal deletion authority is a request-only subject marker. Generic Authorization and `agent_lifecycle_operations.capabilities` retain their existing capability values; `delete_own_account` has no core Capability enum variant and is emitted only from subject-bound OwnerSessionAuthority for session inspection. It cannot be captured in durable operations. Validate with `generic_authorization_cannot_deserialize_session_deletion_entitlements` and `personal_account_authority_is_not_persisted_in_lifecycle_capabilities`.                                                                                                                                                                                                                                 |
| Compatible reader floor / allowed recovery artifacts | Deploy this gate and record-reader release to every replica before enabling writes. Pending: exact immutable image digest and tested rollback executable; no older artifact is certified.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Writer gate and cohort                               | `CHRONO_ACCOUNT_DELETION_ENABLED=false` in code and ConfigMap. It gates new deletion acceptance, not status, original-key replay, standing checks, indexes or the fence writers listed above. Enable only after the reader floor and later cascade capabilities are proved.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Backfill execution                                   | No existing rows are rewritten or backfilled. Startup creates idempotent indexes. Unknown snapshot state starts with point reads and retries; one malformed deletion does not fail process startup or block unrelated claims. No deletion completes automatically.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Backup and dry-run                                   | Pending: staging drill, production backup IDs/timestamp, protected tombstones, restore owner and rehearsal evidence. Synthetic isolated tests cannot prove production backup behavior.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Rollback / forward recovery                          | Close new requests first; retain the reader floor, both collections, events and existing fences. Original accepted keys remain replayable. Reconcile uncertain outcomes from durable storage; do not destructively roll back state. Strict record readers require coordinated future schema changes.                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Verification and completion                          | Run guarded `cargo nextest run -p layer2-core -p layer1-public -p cma` for model, HTTP and gate checks. With `CHRONO_TEST_MONGODB_URI` set, use `--run-ignored only -E 'test(store::account_deletions::tests::)'` and separately `--run-ignored only -E 'test(auth::account_standing::mongo_tests::)'` for real transaction and snapshot checks. `shared_public_contract_matches_real_backend` compares the public corpus; `every_documented_operation_and_named_problem_has_exact_contract_coverage` checks inventory. Run `npm test` and `npm run docs:check` in `sdk/typescript`, and guarded `cargo test` in `sdk/rust`. Pending: staging drill, released digest, end-to-end erasure and operator sign-off.                                                           |


## Record erasure core delivery (C1)

This section records the additional persistence primitive delivered after the
request and access-gate release described above. The preceding release text is
preserved as its original migration record. C1 adds a callable, bounded
`EraseRecords` executor; it does not add worker scheduling, a complete deletion
pipeline, object deletion, remote credential revocation, independent residue
verification, operator routes, restore execution, or production activation.
An accepted deletion request still does not prove that data was erased.

The executable catalog classifies every registered collection once. Personal
ownership comes from exact typed live and historical lineage, including durable
sidecars after their live root disappears. Organization-owned roots and their
descendants are protected before personal ownership, capacity, or destructive
selection is evaluated. An audit actor or activity target does not establish
personal ownership. Unknown legacy ownership produces a typed permanent
`UnsupportedResidue { collection }` block instead of authorizing deletion.

The core removes supported personal records and supports exact revision-fenced
document-reference and global-policy cleanup. It protects settled billing
authority, copies Sandbox key revocation debt before deleting its source, and
returns discovered inventory for durable acknowledgement before consuming its
historical evidence. Each call consumes at most one nonempty page of 500 effect
rows. Indexed pages retain an exact selector with a scalar identity or the native
Agent log tuple; unindexed scans are shared across at most 64 cohort subjects.
Capacity limits bound each person's resource closure, serialized member state,
and the serialized cohort cursor. Partial effects accompany a later error.

Profile and Trigger publisher erasure is not executable in C1. An owned
publisher catalog root, including an apparently unused Profile, produces the
attributed permanent `RetainedReferencePending { collection }` block before
context loading or mutation. C1 does not install publisher erasure markers,
drain older transactions, anonymize retained Profiles, delete immutable Profile
revisions, or rewrite Trigger publisher tombstones. A consumer must not treat
this block as transient unavailability, a completed step, an empty result, or a
successful deletion. C2 replaces this transitional refusal with the publisher
implementation and its compatible readers and admission checks.

Identity rewriting on surviving foreign or organization records is also not
executable in C1. An affected retained actor, policy/audit reference, channel or
Bot custody record, or explicit share-recipient reference produces the
attributed permanent `IdentityRedactionPending { collection }` block. Detection
precedes erasure effects and does not weaken organization ownership protection.
The executor and public reference facade preserve the source rather than run
only part of a required identity rewrite. C3 replaces that block. In particular,
generic share/receipt recipient rewrites remain unavailable through the facade
until C3, even where the lower-level content transformation exists in C1.

Unsettled billing or undelivered claims block usage deletion. Billing-enabled
callers also block on unclaimed, undelivered usage; the disabled billing pilot
may remove unclaimed events. A stopped billing state with no pending request or
retained settlement window is settled even if its confirmed `current` receipt
remains. Driver and clock failures remain retryable without assigning a false
personal cause. Deterministic corruption with no attributable owner produces
typed permanent `CohortBlocked`, and repair may resume the same step.

## C1 migration and recovery record

| Required field | Evidence |
| --- | --- |
| Identity and delivery boundary | Issue #1500 C1, catalog/executor core. The source commit containing this section identifies the implementation. Release owner and immutable image digest remain deployment evidence to be recorded. |
| Stores and triggers | The callable record executor reads the audited collections and may delete supported personally owned rows or rewrite supported references. It creates no collection and starts no worker. Existing B request, standing, admission, and publisher Launch fence behavior remains the earlier migration contract. |
| Index changes | Discovery installs full owner/lineage indexes needed for orphan recovery; effect preparation uses full selector/identity coverage. Partial indexes do not certify historical rows. Agent log carriers reuse frozen native indexes; absent carriers remain absent, and partial explicit log migrations remain restartable. The catalog names writer/index evidence for every namespace. |
| Cursor format | Cursor state binds subject set, catalog digest, collection, indexed mode, and billing mode. A mismatched binding restarts safely. Position is a scalar key or native tuple within its exact selector; bounded probes and one nonempty page prevent unbounded work. Publisher-drain, retained-profile-graph, and identity-delegation checkpoints are later formats and are not written by C1. |
| Reader floor | Every process permitted to execute C1 must understand the core cursor and typed permanent pending/cohort failures. Record the exact immutable C1 image digest as the rollback floor before invoking the executor. No older reader or executable is certified to continue these effects. C2 and C3 establish their own higher floors before writing their additional markers or retained formats. |
| Transitional blocks | `RetainedReferencePending { collection }` and `IdentityRedactionPending { collection }` are permanent typed blocks with the responsible subject hash. They retain source records and cannot complete `EraseRecords`. C2/C3 remove only the block each slice implements. |
| Activation and rollback | Keep `CHRONO_ACCOUNT_DELETION_ENABLED=false` for production activation purposes until the complete pipeline and drills exist. Disabling requests does not undo active records or access gates. Preserve catalog data, indexes, cursors, custody and audit authority; never downgrade below the recorded compatible executor/reader floor after effects begin. |
| Validation and completion evidence | Review the catalog fixture split: supported writer-shaped rows erase, each pending publisher fixture separately refuses without mutation, organization rows remain protected, and billing/cursor/index boundaries are exercised on an isolated replica set. Branch-specific verification results are external review evidence; this document does not assert production deletion, zero residue, or rollback qualification. |

Pending product decisions still govern retention policy, billing retention,
cross-owner document comments, and treatment of organization audit identity.
The conservative C1 blocks do not decide those policies or claim external
platform capabilities.
