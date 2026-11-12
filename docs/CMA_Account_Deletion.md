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
the external identity account. Pending decision D7 uses existing verified owner
authority; recent sign-in verification is not implemented.

`POST /api/v1/account/deletion` requires `Idempotency-Key` and the JSON body
`{"acknowledge":"irreversible"}`. It returns `202 Accepted`,
`Location: /api/v1/account/deletion`, and `Cache-Control: private, no-store`.
Replay of the accepted key returns the existing receipt. The domain store also
deduplicates concurrent requests against the active deletion; after the gate
observes that deletion, a new or different-key POST is refused. Reuse cannot
create a second active deletion. A request writes
the deletion, its initial audit event and the owner's admission fence in one
transaction. Explicitly aborted transactions may retry with bounded backoff;
an unknown commit result requires replay of the original key to reconcile.
An uncertain outcome also inserts the subject hash into the accepting replica's
local denial set before returning the recovery problem. A subsequent successful
snapshot or the stale-snapshot authority check reconciles that denial with
durable records; an uncertain result never creates a success receipt.

`GET /api/v1/account/deletion` returns `200` with the latest receipt or
`404 account_deletion_not_found`. It remains available when new requests are
disabled. Both routes return private, non-cacheable responses, including errors.
Receipts contain `id`, `status`, `step`, `requested_at`, `deadline_at`,
`completed_at`, and `steps`. They expose no raw subject, hash, credentials,
resource inventory or worker leases. The deadline is seven days after request;
this release does not claim the cleanup deadline is achieved.

The request switch is `CHRONO_ACCOUNT_DELETION_ENABLED`, strictly `true` or
`false`, default `false`. Disabled requests return `503
account_deletion_unavailable`; the session omits `delete_own_account`.
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
authoritative point reads. A failed point read denies access with a typed service
unavailability response; absence from a stale snapshot never grants access.
Steady-state checks do not add a database command per request.

The gate covers:

- Ambient browser and bearer authentication, including the resource-request
  and worker-credential branches that return early from authentication.
- Sign-in callbacks and actor-link renewal.
- Agent tool capability authentication and provider tool relay, using the
  Sandbox owner.
- Resource authorization for both the caller and personal resource owner,
  including stream reauthorization and sharing to other callers.
- Document owner, recipient and link-token reads.
- Public Trigger preview, badge, entry and open routes; an erasing publisher's
  Trigger is unavailable.
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
`GET /api/v1/account/deletion`. An additional narrowly scoped recovery exception
allows `POST /api/v1/account/deletion` only when authoritative storage proves
the exact key was already accepted for this subject. A new or different key
remains refused. This reconciles the original request's unknown commit result
without reopening data access or admitting a second deletion. These exceptions
preserve identity inspection, sign-out and progress recovery while data access
is revoked. While erasing, session inspection retains authentication metadata
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
D2's minimal tombstone with any provider identity evidence it needs. This release
does not execute that verification or retain a separate recovery inventory.

The following remain later deliveries: worker scheduling, actual inventory,
Sandbox retirement, home finality, record/object erasure, credential revocation,
zero-residue verification, operator retry, tombstone export/restore replay, and
real deletion drills. External dependencies E1–E6 and policy decisions D1–D8
remain pending. D2's suggested 37-day receipt retention and other policy
exceptions require confirmation; this PR does not implement or assert physical
retention exceptions. Organization-owned resources remain outside personal
account ownership (D5). D6's suggested irreversible request has no cooling-off
period; final policy approval is pending.

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

| Required field                                       | Evidence                                                                                                                                                                                                                                                                                                                                                                                           |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Release / migration identity, phase and owners       | Issue #1500, PR B: additive reader-floor release; implementation source commits are in the review report. API/store owners must record the released digest before activation.                                                                                                                                                                                                                      |
| Affected stores and all writer triggers              | New `account_deletions` and `account_deletion_events`; only the enabled owner request writes new deletions. The same transaction increments existing `sandbox_admissions.fence`. Claim/commit interfaces support later workers; this release starts no worker. Document/resource share transactions also check recipient standing.                                                                 |
| Old/new format and preservation contract             | Existing resource aggregates and owner fence shape are preserved. Deletions have unique partial `active_subject_hash`, subject lookup, replay and due-work indexes; completed rows have a BSON date `expire_at` TTL. Events have unique `(deletion_id, seq)` and their own TTL. Exactly two collections are added; no object-inventory collection is created. Both are Independent census entries. |
| Compatible reader floor / allowed recovery artifacts | The release containing this PR's gate and typed record readers is the floor. Exact immutable image digest and tested rollback executable are pending release-owner evidence; no older artifact is certified.                                                                                                                                                                                       |
| Writer gate and cohort                               | `CHRONO_ACCOUNT_DELETION_ENABLED=false` in code and ConfigMap; strictly boolean. Enable only after every replica has the floor and later cascade capability is proved. Status reads and gates remain active with the writer disabled.                                                                                                                                                              |
| Backfill execution                                   | No existing rows are rewritten and no backfill runs. Startup creates idempotent indexes for the two collections. No raw subject is removed until explicit model completion; this release never completes a deletion automatically.                                                                                                                                                                 |
| Backup and dry-run                                   | Production backup IDs, timestamp, protected tombstones, restore owner and rehearsal evidence remain pending later PRs/operators. Local tests exercise synthetic isolated databases only and cannot prove production backup behavior.                                                                                                                                                               |
| Rollback / forward recovery                          | Close the writer first; preserve the two collections and existing admission fences. Retain the compatible API reader floor while any active deletion exists. Unknown commits reconcile through original-key replay. No destructive schema rollback.                                                                                                                                                |
| Verification and completion                          | Targeted model, HTTP, SDK, gate, transaction and census checks are recorded with actual outcomes in the PR B review report. Production activation, release digest, end-to-end erasure and operator sign-off remain unverified.                                                                                                                                                                     |
