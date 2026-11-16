# CMA account deletion: request, status and access revocation

This release implements the account deletion record, fenced state machine,
request and status APIs, and access revocation. Accepted records remain
`running` at `inventory`: the background worker, inventory collection, physical
erasure orchestration, independent residue verification, operator routes and restore tools
are delivered by later PRs. The standalone record-erasure effects and catalog
are available for direct invocation and testing; no worker invokes them yet. Acceptance is not evidence that data was erased.
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
receipt is not proof of revocation. Nonempty `inventory.pending_delegations`
also requires a separate `residue.delegations` receipt with zero remaining grants
and a parseable confirmation time. The credential reconciler must obtain that
authoritative confirmation for every inventoried delegation; an empty key list
does not prove delegation revocation. A missing delegation receipt leaves the
deletion, raw subject and debt unchanged. The optional receipt defaults absent
and is omitted from stored reports without delegation evidence, preserving
historical reports whose delegation inventory is empty. Later delayed verification must reconcile
the minimal tombstone with any provider identity evidence it needs. This release
does not execute that verification or retain a separate recovery inventory.

The following remain later deliveries: worker scheduling, actual inventory,
Sandbox retirement, home finality, record-erasure orchestration, object erasure, credential revocation,
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


## Erasure catalog

The [collection catalog](CMA_Account_Erasure_Catalog.md) declares a policy for
all 153 collections in the Agent Manager schema authority, with a mandatory
one-to-one coverage test. It records each writer, ownership path and existing
index. Adding a collection without a policy fails the test. No collection or
index is added by the record effects.

| Policy | Execution and boundary |
| --- | --- |
| `Owner` | Delete rows whose explicitly audited field equals the subject. |
| `Linked` | Recompute Sandbox, Workspace, Worktree, Agent, Launch, binding, Trigger, document, Profile and report IDs from current records. Inventory remains a lower bound. Delete children before their parent aggregates. |
| `OwnerKey` / `LinkedKey` | Match exact documented key grammars, with literal subject/ID boundaries; neighboring subjects and prefixes are preserved. |
| `Reference` | Rewrite personal references in surviving owners' documents, shares, share receipts, settings and rollout gates using their current revision CAS; delete activity whose target is the subject. |
| `ProvenAbsent` | Shared scans test the documented likely subject and known lineage selectors. A match refuses with `UnsupportedResidue { collection }`; it never authorizes deletion of an unqualified legacy row. |
| `Custody` / `Sealed` | Declare credential custody and encrypted envelopes. Record erasure skips these; credential revocation must handle them later. No encrypted-envelope erasure is advertised here. |
| `ObjectAccounting` | Preserve document quota and checkpoint object accounting until the later object step has proved object absence. |
| `Retained` | Preserve deletion records/events until their minimal receipt and tombstone lifecycle completes. |
| `Global` | Preserve shared deployment state without per-account ownership. |

`erase_records_with_options` accepts at most 64 subjects, their durable
inventories, a serializable cursor and the caller's authoritative
`billing_enabled` deployment setting. One invocation applies at most one page
of 500 effects and returns per-collection counts, newly discovered resource IDs,
pending credential IDs and waiting/more-work state. The caller must durably union
returned inventory before retrying; no source record is erased before that
acknowledgement. This PR supplies effects only: it starts no worker, inventory
writer, scheduler or remote revocation.

Historical ownership is recovered from persistent person-owned policies, usage
and billing projections, Agent creation/lifecycle evidence, shares, bindings and
conversations, preparations, reuse records, prepared resources, diagnostic
bindings and Launch protocol/handoff records. These sources are deleted after
ordinary descendants. IDs are cached within the cohort; each subsequent call
unions new supplied inventory and rechecks current ownership, lease and key
authority. Current foreign/organization ownership, including surviving Launch
and binding sidecars when their roots are absent, overrides stale inventory.
Discovery cursors stream batches of at most 500 projected rows. Each account is bounded
to 20,000 personal identities, 512 KiB of identity text and 128 KiB of live Sandbox snapshots;
exceeding a limit fails closed before effects. Larger accounts require an
explicit operator plan or a later paged-inventory design.

Indexed effect selection has no `_id` sort: it uses the selector's existing
index and limits results to 500. Shared scans use ordered `_id` pages for the
whole cohort. Cursors bind the subject set, collection name, indexed flag,
catalog digest and billing mode; a mismatch restarts idempotently. The cursor is
an optimization, not a worker lease or completion proof. Concurrent deletion of
the same IDs is reconciled by rereading remaining rows. Every failure after
successful effects returns `Error::Interrupted` containing those counts and the
underlying error. `BillingSettlementPending` and `UnsupportedResidue` include a
subject hash so a caller can isolate that account from its cohort.

Billing funding-state discovery is shared once before effects; final funding
deletes compare the checked record and the successful preflight is cached for the cohort. `Options` has no default: callers must explicitly provide billing mode. With billing enabled, every undelivered usage event,
including an event never claimed for delivery, blocks with
`BillingSettlementPending`. With billing disabled, unclaimed events cannot be
delivered and may be erased; an existing undelivered claim still blocks. Pending
reservations and retained funding windows block in either mode. Confirmed
historical `current` receipts without pending requests or windows are settled.
Final deletion predicates compare the checked funding snapshot; retries cannot
mistake another erasure worker's successful deletion for settlement debt.

The caller must prove home finality and retire every owned Sandbox. The effect
waits for retired status and absent/expired work leases, parsing RFC3339 instants
consistently rather than ordering timestamp strings. Sandbox deletion is last,
with exact owner, revision, status and checked lease CAS. Revoke debt and current
or pending key IDs must be acknowledged in `inventory.pending_key_ids` before
the containing Sandbox may disappear. A newer revision requires a fresh check.

Organization-owned resources and their attributable audit records survive with
personal references rewritten to `deleted-account` through exact revision or
record CAS. This includes policy admission evidence, binding managers, creation
provenance, command/control/artifact actors, channel usage and Bot creation
identity. Personal feedback and suggestions are removed even when they concern
an organization resource. Explicit activity resource targets can prove an
organization relationship only when an authoritative organization policy agrees.
An activity actor never establishes personal ownership of its target, including
when both the target Sandbox and its policy have already been purged. A generic
HTTP path template cannot establish an organization target and remains subject
to the ordinary activity actor rule. These policies are pending product
decision. Bot key and revocation-debt identifiers remain intact. Personal Bot
delegation debt must be durably captured before its credential identity is
rewritten. A deleted creator cannot supply renewable authority; reassignment and
reauthorization are pending product decision and no external action is performed.

Referenced Profiles are retained only for existing exact immutable references.
Aggregate `owner` and `record.owner` become `deleted-account`, visibility becomes
private, `retired_at` is set, names and descriptions are emptied, and unpublished
`record.draft` and fork attribution are removed. Only referenced revisions and
their exact transitive child revisions survive; unused revisions are erased.
The aggregate remains fenced, so it cannot be forked, published or used for a
new reference. Existing revisions and the mutable inactivity policy remain
readable for surviving resources. Readers reconstruct only a fixed inert draft
when decoding the strictly validated retired anonymous aggregate; no authoring
content is recovered from a retained revision. This policy is pending product decision.
The compatible reader floor also includes neutral retained-configuration trust
disclosure labels. Frozen commands, environment variables, digests and consent
remain authoritative; a previously created Draft retains its stored manifest.

Catalogue admission and authoring transactions only read erasure fences. They do
not increment shared Profile or publisher counters. Before discovering references,
the erasure executor majority-writes `account_erasure_fenced` and a server BSON
`account_erasure_fenced_at` timestamp on owned Profile and Trigger records and
commits those writes. It then waits longer than `transactionLifetimeLimitSeconds`.
When permitted, it reads that server parameter and waits
`max(ceil(1.5 * limit) + 10 seconds, CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS)`.
MongoDB checks expired transactions every `limit / 2` seconds (at most every
60 seconds), so expiry may take approximately 1.5 times the nominal lifetime.
The additional ten seconds cover cleanup scheduling and acknowledgement jitter.
The configured value remains a floor even when the parameter is readable. If
reading the parameter is forbidden, the configured fallback is used (default
120 seconds, validated range 120..86400). Operators must set it above the same
cleanup bound for every participating server when that parameter is unreadable. Do not increase the server lifetime during an
active drain. A resumable cursor retains the deadline; losing the cursor repeats
the barrier safely. A newly observed unfenced owned Profile restarts the barrier
and discards the cached ownership/reference snapshot before effects resume.
Transactions with older snapshots must commit within this
window or expire; their references are then included in the single shared scan.
New transactions see the committed fence and refuse. Unresolved authoring
placeholders retain their existing contract. The request switch remains off by
default and this PR starts no erasure worker.

Reference rewrites preserve surviving owners' content and identities. Comments
by the erased subject are removed; their receipt comment author becomes
`deleted-account` and the body becomes empty. Trigger definitions are removed before revision rewriting;
published revisions become canonical `publisher_deleted` tombstones retaining
original key, published revision, digest and timestamps. Public readers map
canonical publisher tombstones and validated durable erasure markers to typed
unavailable, including definition-before-tombstone intermediate states. An
unexplained missing mapping, revision or publisher remains Corrupt. Draft visitor Launches and accepted Launches
without an Agent refuse with the existing retired/unavailable problem and
remediation; completed Launches and accepted Launches with a created Agent retain
their existing resource path. New Trigger Agent admission reads the exact
live definition and matching non-tombstone revision and their fences in its resource transaction,
after checking for a durable committed admission replay. Cached resolutions and
older transactions that commit during the drain are visible before retention decisions.
When a surviving Trigger publisher references another erased author's Profiles,
its exact frozen Launch effects wait through the drain and resume only after the
required tree is canonically retained. Durable Profile-declared parent resources
can finish their exact child declarations during the fence as described below.
Admission for a new Trigger effect verifies the durable Launch,
effect, lease, input receipt, surviving exact Trigger revision and resource
lineage transactionally; this does not admit a new direct Profile reference or
permit an erased Trigger publisher to create resources.
Already committed Agent/reuse receipts remain replayable; completing the visitor
Launch does not require statistics on a publisher definition removed with validated
erasure evidence; an unexplained disappearance remains Corrupt.
See [Trigger continuity](CMA_Trigger.md). Mutable storage CAS
uses a separate `mutation_version` for immutable Trigger/share-command snapshots.
A final explicit share audience becomes empty only after the link is revoked;
an active empty audience remains invalid. CAS conflicts leave the scan page
pending, so a fresh read retries without overwriting concurrent owner edits.
Personal-share rewrites reserve the matching policy revision by exact CAS before
incrementing the share revision. Every intermediate state preserves the reader's
policy-revision fence; an interrupted reservation can be retried safely. A live
Sandbox requires a matching policy and lineage. An orphan share without either
Sandbox or policy keeps its audit-only rewrite path. Reservations are concurrency
fences and do not count as redacted-content effects.
Live matching rollout slots similarly wait before any slot can be cleared.
Removing the final explicit rollout cohort disables that rollout when its
sampling basis is zero. Removing the final budget owner override disables model
budget enforcement only when no default budget policy remains. Both changes
preserve current reader validation and all unrelated policy and limits.

These policies are **pending product decision**: default deletion of settled
usage/billing data; redaction of comments and their receipts; publisher revision
tombstones; preservation of organization resources while removing personal
references; and the deletion-receipt, backup and legal-hold exceptions. Permanent
retirement archives require the later retirement/legal-hold step to block before
record effects. This component neither certifies external retention policy nor
implements that hold. The existing suggested retention periods remain pending
product decision.

Operational read-only confirmation is still required for all 55 `ProvenAbsent`
names listed individually in the catalog evidence. Most have no active writer;
remaining partial legacy writers and readers establish some likely paths but
do not prove complete historical ownership schemas. Orphaned indirect legacy
references without a resolvable subject/lineage remain an explicit limitation,
requiring an operator
census and a concrete selector before production activation. The later residue
step must independently detect late writers, including operator changes to
shared budget/rollout cohorts, and verify again after completion. Batch counters
cover acknowledged successful effects; they are not independent residue proof
or an external billing settlement receipt.

### Record-redaction migration record

| Required field | Evidence |
| --- | --- |
| Release / migration identity, phase and owners | Account record erasure catalog and reference redaction; source commit containing this record; implementation and review only. Release/operator artifact approval remains unverified. |
| Affected stores and all writer triggers | The complete catalog and direct `erase_records_with_options` calls; document/share mutations, immutable share receipts, Trigger revisions, anonymous Profile retention, organization actor/channel/audit CAS, Bot delegation handoff, settings and log gate CAS. Catalogue resource admission, draft/default writes, marketplace forks, Profile/Trigger publication and Event Worker saves read usable Profile tree fences transactionally without catalogue writes. Trigger Agent admission reads the exact live definition/revision fences. Only the erasure side installs majority fences and waits for the transaction lifetime before discovering references. Personal-share redaction reserves its policy revision before the share CAS. No startup/backfill/worker invocation is enabled. |
| Old/new format and preservation contract | Existing document revision CAS; receipt comment redaction exception; revoked empty explicit share audiences; additive `publisher_deleted` marker on canonical Trigger tombstones; private retired anonymous Profile aggregates without authoring drafts, exact retained revisions and erasure-side fence timestamps; canonical erased-actor receipts and channel usage metadata; acknowledged historical inventory and Bot delegation debt; separate mutable `mutation_version` for immutable snapshots. Existing IDs/digests/publication numbers/timestamps are preserved. No new collection or TTL. Discovery indexes and the transaction-drain fallback configuration are added in this review. `ErasureInventory.pending_delegations` is additive, defaults empty and is omitted when empty; older strict inventory readers cannot accept nonempty new records. |
| Compatible reader floor / allowed recovery artifacts | Every serving and approved recovery binary must contain the strict Trigger tombstone, revoked-empty-share, canonical erased-actor receipt, channel-usage redaction and anonymous-Profile readers in this source, together with all catalogue-pin, draft/default/fork and exact Trigger Agent fence readers described above. Account inventory readers must accept the additive pending delegation field before any writer stores it. Residue readers must accept optional `delegations` receipts before those are written; old strict readers reject a present new field, while absent receipts retain their historical serialized shape. Tests exercise current document/share/receipt/Trigger digest readers, full surviving-share authorization, retained Profile maintenance/resolution, organization audit/channel readers and Draft/admitted/completed visitor Launch continuity. Earlier binaries rejecting these shapes or ignoring these fences are excluded. Exact deployed image digest and predecessor-artifact execution are not verified locally. |
| Writer gate and cohort | Production request flag remains off by default; no worker or rollout enables effects in this PR. Deploy the compatible reader and catalogue-reference writer floor everywhere before an authorized worker can redact any selected account. Older writers ignore `account_erasure_fenced` and cannot overlap or serve as rollback artifacts during erasure. Shared scans bind an explicit cohort. |
| Backfill execution | No automatic backfill. Direct batches are limited to 500 selected rows, no transactions; resumable cursor with catalog/collection/mode binding, bounded cached historical discovery, revision/slot CAS, idempotent `_id` deletion, resource/key/delegation handoff and durable acknowledgement precede removal of their source. One worker/budget admission and scheduling are later deliveries. |
| Backup and dry-run | Disposable replica-set fixtures only, including cross-owner isolation and 10,000 history rows. No production backup or dry-run is claimed. Authorized activation must record a protected coherent backup, restore owner and post-backup write reconciliation before irreversible deletion. |
| Rollback / forward recovery | Closing the request/worker writer stops future changes but cannot restore erased bytes. Keep the new reader floor and use compatible forward recovery. Original immutable event digests and command fingerprints remain, with strictly validated canonical erasure metadata and replacement digests. Redacted share-command replay returns the existing typed idempotency conflict rather than restoring personal data. Retained Bot credential tombstones require later authorized reassignment; the executor does not renew old personal authority. Never downgrade tombstones or restore a stale snapshot over live writes. |
| Verification and completion | Named real-store tests and local checks are reported with the implementation commits. Production rollout, independent residue proof, exact artifact matrix and product/operator sign-off remain unverified. |


### Second-review discovery and drain migration

The never-released `profile_admission_fence` and
`publisher_agent_admission_fence` fields are removed outright, without backfill.
No hot-path writer emits either field or an entire-document literal guard.
Deploy all read-only fence readers, the strict retired-Profile decoder, Trigger
erasure-proof readers and the drain-capable executor together before erasure.
Pre-drain executors and writers ignoring the fence are not recovery artifacts.

First cohort load installs idempotent, nonunique discovery indexes from
`store/account_erasure/indexes.rs`: durable-source owner fields, hierarchy parent fields, binding sidecar identity
fields, and usage-event Sandbox IDs. Existing full indexes with a
suitable leading key are reused; partial/sparse indexes do not cover historical
rows outside their predicate. Full Sandbox-child indexes retain the normal
`agents_sandbox`, `workspaces_sandbox` and `worktrees_sandbox` names so a discovery
pass before ordinary store startup cannot create a conflicting equivalent key.
Operators may build these indexes ahead of an
authorized erasure to move index-build cost outside its budget. The collection
catalog remains conservative about indexed effect rules. History and exact
Profile-reference sources stream through shared pages of 500 once per cohort;
reference sets are cached after the drain. Indexed, resumable owner pages first
bound candidate Profile IDs; each foreign reference source then uses one shared keyset pass.
Unrelated tenants' pins are discarded as each row is read, and a cohort without
owned Profiles skips reference discovery entirely. No per-revision reference scan
runs. Profile discovery bounds each person's owned IDs to 20,000 and revision
links to 100,000, subject to the shared 768 KiB serialized member-state budget
for resource identities, working graph and retained pins. An overflow is an attributed permanent capacity block. Revision graph discovery
checks immutable record identity against its query fields and canonical key. Every
required owned root and transitive pin must resolve before effects; damaged or
missing edges return an attributed corruption block rather than dropping a
surviving dependency. Unrelated foreign corruption is not claimed by the cohort.
Fresh ownership checks use indexed roots and candidate pages; organization
subtrees are not enumerated into the personal closure.

Personal closure capacity is checked per subject, including 20,000 resource IDs,
512 KiB ID text, 1 MiB per projected ownership-query result and 128 KiB live Sandbox snapshots. Incoming inventory is classified in pages of 500 against current roots and durable
sidecar ownership before it consumes personal capacity. Repeated known or protected
identities are cached; stale foreign and organization identities remain excluded.
Organization protection does not consume those personal limits. The serialized resumable cursor has a separate
1 MiB hard limit, well below the BSON document limit. Capacity and corrupt-record
refusals are typed and carry `subject_hash`; callers can isolate that account
rather than retrying it indefinitely as a store outage. Delegation scan totals
are also per subject and advance only after the caller acknowledges every tuple;
a missing acknowledgement replays the page without incrementing its totals.

Operator activity is retained with its operator actor, resource target and audit
outcome intact; an erased personal `detail.subject` becomes `deleted-account`.
Unrelated subjects are unchanged. This policy is pending product decision.


### Third-review continuation and cursor migration

The reader floor additionally requires exact existing-parent continuation for
Profile-declared Workspaces and Agents. The parent must already be durable and
its immutable snapshot, child index, exact revision, digest, owner and lineage
must agree inside the admission transaction. A Workspace created by a frozen
Trigger is also an existing parent for its declared Agents. This continuation
works while the Profile is fenced and after canonical anonymous retirement;
manual new references still refuse. A pending new Trigger effect retains its
strict Launch/effect proof and canonical-retention boundary.

Organization binding input copies `metadata.owner.subject` into the binding and
then into `Sandbox.owner`; the manager is a separate actor. The policy records
`owner_kind = organization` with the same organization subject. Profile reference
discovery preserves pins on these organization resources and their descendants,
independently of a personal creator or admission actor. Personal resources that
are being erased do not establish retention roots.

Reference discovery now checkpoints its ownership/source stage and last `_id`
in the versioned erasure cursor. Each invocation reads at most one projected
page of 500 reference rows. Each shared source pass has ordered keyset pages;
no effects occur until the complete exact closure validates. A lost cursor
restarts the majority barrier and scan. The owner-page queries use nonunique
`{owner: 1, _id: 1}` indexes on the three Profile aggregate collections. Other
reference sources share one `_id` pass across the cohort. Cursor binding version
changes invalidate older optimization state safely. The indexed effect path
follows each exact selector with an identity keyset; unchanged and protected pages advance within that selector's compound index,
without switching into the shared unindexed pass.

Serialized personal resource and Profile state share a 768 KiB member budget,
including the discovery graph and retained pins. Live Sandbox snapshots have a
separate 128 KiB budget and are never serialized into the cursor. Their combined
896 KiB ceiling leaves room within the 1 MiB cohort cursor limit. The existing
20,000-ID and 512 KiB identity-text guards remain additional limits. Capacity
attribution includes actual Profile-state contributions, so a large unrelated
resource context cannot receive another member's refusal. Infrastructure clock
and network failures remain retryable store errors without account attribution. Deterministic corruption without ownership evidence is a
typed permanent cohort block requiring operator intervention, as described below.

Operator `detail.subject` redaction applies to all audit targets, including
`profiles`, `profile:<kind>:<id>`, `sandboxes`, `sandbox:<id>` and event targets.
The operator, outcome, other details and target remain intact through exact CAS;
this retention policy is pending product decision. Missing publisher statistics
are skipped only with independently validated erasure evidence; unexplained
missing definitions remain corruption. Draft/default CAS conflicts, including
unknown commits, require reload through the existing Conflict contract. Fenced
platform defaults receive a permanent `InvalidRequest` from settings mutations;
genuine store failures remain unavailable and reads retain their existing
corruption handling. Marketplace forks retain their existing refusal and
transient-store contracts.

### Fourth-review publisher protection

The compatible reader floor additionally requires the deleting-parent check for
Profile-declared continuation. An unretained Pending pin never authorizes a new
child of a deleting personal Sandbox. Its owner is checked inside the admission
snapshot against active deletion evidence and owner-specific catalogue fences.
A valid organization policy takes precedence over overlapping subject strings;
canonical retained exact pins and surviving personal roots keep their existing
completion contract. These checks perform reads only. This policy remains
pending product decision together with the retained-Profile exception.

Profile create retains Duplicate and fenced-reference refusal, while a final
driver failure, including an unknown commit, remains Unavailable without mutation
replay. Draft/fork/settings CAS conflicts retain their existing Conflict contract.
Platform mutations distinguish explicit unavailable-reference input from malformed
stored data: only the former is InvalidRequest; persisted corruption remains a
server/store failure. Repairing the stored evidence permits an ordinary retry.

### Fourth-review selector and permanent-refusal migration

The executor's cursor binding is now `account-record-effects-v7`. A previous
cursor restarts safely. Each indexed effect selector has a semantic field/value
position plus a scalar `_id` or bounded native key tuple; selector removal cannot
shift an array offset or skip a following resource. Each indexed page call probes at most eight empty
selector ranges. The executor reads at most one nonempty effect page of 500 rows
per invocation. Unchanged and protected pages advance on the same index. Conflicts retain the previous position. Only
catalogue operations explicitly classified as unindexed run a shared `_id`
collection pass. These passes are independent of indexed continuation.

Before any cohort query, `store/account_erasure/indexes.rs` ensures full,
nonunique `{selector_field: 1, _id: 1}` indexes for every indexed `Owner` and
`Linked` field except `_id`, plus `resource_shares.accepted_subjects`. Exact
identity and identity-prefix selectors reuse `_id_`. Equivalent complete compound
indexes are reused regardless of name; partial and sparse indexes do not qualify.
Existing leading indexes and their ordinary startup names are retained. Operators
may build the compound indexes before enabling erasure to move build cost outside
the cohort budget. No collection, uniqueness constraint, TTL or document backfill
is added. The versioned Agent log carriers are an exception to generic compound
index preparation: absent carriers remain absent, validated carriers in a partial
explicit migration remain restartable, and installed frozen indexes are reused
without altering their complete DDL inventory or migration checksum.
Segments seek by incarnation/ordinal, publications and admissions by
incarnation/publication key, evidence by incarnation/generation/identity, audits
by audit identity, and capacity recovery by format version/identity. Their cursor
stores one bounded tuple rather than a scalar identity. Pre-existing validator-free
legacy orphan collections without a native index retain the ordinary compound selector path;
erasure never claims to prepare or repair their versioned DDL.
Command-monitoring explain tests replay the actual keyset queries:
1,101 protected rows and three eligible rows examine 1,104 matching documents,
with at most 501 keys and 500 documents examined per page, despite 20,000 unrelated
rows in the same collection.

`InvalidRecord` with proven personal ownership remains an attributed
`CorruptRecord`. Without that evidence, the executor returns
`CohortBlocked(CorruptRecord)` rather than a transient store error or an arbitrary
member hash. Invalid readable transaction-lifetime values and unsupported drain
configuration return `CohortBlocked(InvalidTransactionDrain)`. PR D must alert and
stop that cohort for manual repair; automatic retries cannot cure these failures.
Partial counts and inventory accompany either block. Clock and driver failures
remain transient. Server lifetimes must be positive integers at most 3,600
seconds; the configured drain floor must not exceed 86,400 seconds. Readable
malformed or excessive values never silently use the fallback. An unreadable
parameter still uses the configured conservative floor. The existing cleanup
formula and prohibition on increasing lifetime during a drain continue to apply.

The unreachable standalone 1 MiB Profile-graph byte counter is removed. The
serialized 768 KiB per-member budget and 1 MiB cohort cursor budget include the
actual graph and retained-pin contribution; their attributed limits remain
mandatory. Owned-Profile and revision-link count limits remain in force.

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

## Historical intermediate refusal readers

C1 may persist `RetainedReferencePending { collection }` as
`{"kind":"retained_reference_pending","collection":"agent_profiles"}` in an
`account_deletions.block` and in the `reason` of append-only Blocked events.
C2 replaces that executor refusal with publisher retention, but keeps the exact
variant, wire name and required string field as a historical reader contract.
`IdentityRedactionPending { collection }` uses
`{"kind":"identity_redaction_pending","collection":"resource_policies"}`;
C2 emits this safety refusal. C3 replaces its producer while retaining both
historical reader variants. Do not remove a variant when retiring its
producer.

This is an additive reader correction, with no schema-version change, new
collection, record rewrite, event rewrite or destructive migration. Existing
blocked operations stay blocked until explicitly rearmed. The original request
key resolves the same durable deletion; rearm and a new lease resume its current
step, and old Blocked events remain readable with their original sequence, wire
reason and BSON values. Rearming does not bypass the current executor or
certify that the required preservation or identity redaction has completed. Unknown reason
kinds, missing required collection fields and wrong collection types remain
malformed; they are never silently ignored.

The C2 reader floor includes this correction before accepting C1 blocked state.
Pin the corrected immutable executable in the release record and keep it during
rollback; a prior C2 binary without `retained_reference_pending` cannot replay
those records or read their events. Deploy compatible readers before any later
producer change, preserve all request keys and append-only events, and retain
both intermediate wire variants in C3 recovery artifacts. Product policy and
production activation remain pending; this correction starts no worker or
production erasure.

Validate the independent C1 BSON fixtures with the registered Mongo prefix
`store::account_deletions::tests::legacy_blocks::`. Tests cover both historical
record/event variants, exact owner/key replay, explicit rearm and resume,
unchanged historical BSON event values, and strict malformed-input refusal.
Current
`store::account_erasure::profile_tests::` verifies actual publisher retention
rather than the retired publisher-pending path;
C2's transitional identity-pending tests prove its safety refusal; C3 replaces
them with `store::account_erasure::organization_tests::` and related tests of
actual identity redaction. Both stages keep the historical reader regressions.

Migration identity: `2026-11-14-account-erasure-intermediate-refusal-readers`;
service owner: CMA account deletion, issue #1500. The source commit is the
commit containing this section; pin its immutable executable/image digest in
the release record. Prerequisites are the C1 record/event schema and C2 publisher
reader floor. There is no DDL, backfill or transformation command. Deploy the
compatible reader before allowing a later executor to replace a pending reason;
keep the same reader during recovery and rollback. Local regression receipts do
not certify production deployment or an executable release artifact.
