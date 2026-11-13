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


## Publisher erasure delivery (C2)

This section records the additional record-executor capabilities delivered after
the request/access-gate release whose original text remains above. C2 includes
the C1 catalog, bounded executor, personal/organization ownership separation,
historical inventory acknowledgement, billing guards, cursor/index budgets,
and supported base reference cleanup. It adds publisher Profile/Trigger
erasure and its admission/reader compatibility. No background deletion worker,
object deletion, external credential revocation, residue verification, operator
route, restore execution, or production activation is introduced here.

Before discovering publisher references, erasure majority-writes the Profile
and Trigger aggregate/revision fences with server timestamps. It waits
`max(ceil(1.5 * transactionLifetimeLimitSeconds) + 10 seconds, configured floor)`
to cover expired transaction cleanup and scheduling jitter. The floor is
`CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS`, default 120 seconds. If the
server lifetime cannot be read, the configured fallback must still cover that
bound on every server. Do not increase transaction lifetime during an active
drain. Unsupported configuration or invalid readable lifetime is a permanent
cohort intervention block; a failed server command remains retryable.

Reference discovery scans one ordered page of at most 500 rows per call and
persists its stage and scalar position. No record effects run before the complete
closure is validated. A late catalog insert restarts the barrier and discovery.
Admission transactions read committed catalog fences without incrementing or
upserting publisher catalog counters. B's Launch admission contract remains:
only the visitor serializes through its Sandbox admission fence, while a
different publisher's deletion standing is read in the same snapshot.

Unreferenced publisher Profile trees are removed. Referenced Profiles preserve
only required immutable revisions and exact transitive pins. Retained aggregates
use the canonical `deleted-account` owner, private visibility, permanent
retirement, empty display metadata, and no authoring draft or fork identity.
Strict retained readers recognize only the canonical representation. Manual new
references, publication, forks, and draft/default pins refuse committed erasure
fences. Already admitted exact Sandbox/Workspace declarations and surviving
Trigger Launch effects may continue only after the required pins are canonically
retained, with transactional proof of their durable parent, exact index/digest,
effect, input receipt, and resource lineage. A deleting personal parent waits;
verified organization ownership takes precedence over subject-string overlap.

An erased Trigger publisher leaves canonical revision tombstones and durable
erasure evidence. Completed visitor Launches and accepted Launches with an
already committed Agent retain their recorded resource path. Draft and
Agent-less accepted Launches return typed retired/unavailable. New resource
effects cannot use a retained anonymous Profile to bypass an erased Trigger
publisher. Missing-input recovery preserves published confirmation evidence
without treating malformed records as erased publishers. This retention policy
remains subject to product approval.

C2 replaces C1's `RetainedReferencePending { collection }` transitional block.
Identity rewriting on surviving foreign/organization audits, policies, channels,
Bot custody, and explicit share recipients remains unavailable until C3. The
executor and reference facade return the attributed permanent
`IdentityRedactionPending { collection }` before mutation. A block cannot become
a completed step or successful empty result. Generic share/receipt recipient
rewrites remain blocked through that facade even where a lower-level content
helper already exists. C3 supplies the identity rewrites and delegation debt
handoff together with the readers that recognize their canonical output.

## C2 migration and recovery record

| Required field | Evidence |
| --- | --- |
| Identity and dependency | Issue #1500 C2 follows the C1 executor floor. Source identity is the commit containing this section; release owner records its immutable image digest before execution. The original B migration record above remains intact. |
| Writer triggers | Publisher executor calls write additive `account_erasure_fenced` and server-date markers on Profile/Trigger catalog rows, then canonical retained/tombstone representations or physical removal. Draft/default/fork/publication and resource-admission writers validate exact catalog pins in their existing snapshot transaction. |
| Drain configuration | `CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS` is strictly validated, defaults to 120, and must cover transaction lifetime cleanup plus safety margin. Unsupported values require permanent intervention; never shorten a durable deadline or increase server lifetime during drain. |
| Format and preservation | Cursor adds durable drain/reference-discovery and retained-pin state under bounded serialized budgets. Canonical retained Profile and Trigger records preserve immutable execution content/digests required by existing foreign resources while removing publisher authoring identity. No identity-delegation checkpoint or canonical foreign audit-actor rewrite is written before C3. |
| Reader floor | Deploy the C2 retained Profile/Trigger readers and admission fence checks to every relevant API/executor process before any publisher erasure writes. Pin the exact C2 image digest as the minimum rollback artifact. An older binary can misread retained/tombstone rows or admit new pins; C1 is not a valid rollback executable after those writes. |
| Remaining permanent block | `IdentityRedactionPending { collection }` preserves affected foreign/organization sources and blocks completion until C3. No successful identity redaction, share facade rewrite, delegation handoff, or organization audit rewrite is claimed here. |
| Recovery | Retain marker timestamps, durable cursor, exact immutable revisions, erasure evidence, and all key/delegation custody. Resume discovery/retention from the C2-compatible cursor. Disabling new requests leaves active denials and stored authority intact; it does not authorize a downgrade or reverse completed effects. |
| Validation and release evidence | The C2 test slice restores complete publisher admission, draft/fork/default, barrier, exact-continuation, retained-reader and corruption tests from immutable FINAL. Identity-specific tests remain assigned to C3, with explicit C2 permanent-refusal tests supplied by the transitional suite. Record actual guarded checks and isolated Mongo results separately. Production activation, zero residue, and rollback qualification remain unproven. |
