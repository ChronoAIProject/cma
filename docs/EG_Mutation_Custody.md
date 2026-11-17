# EG mutation custody rollout

Issues: #2196 (channel/provider mutations), #2197 (Telegram webhook mutations).
These fixes are for the EG release after E1. No production rollout is part of
this change.

## Telegram custody

A retained webhook lease identifies an unresolved operation. Its 60-second
expiry closes new dispatch by that claimant; it does not prove cancellation of
an accepted Telegram request. Set/delete timeouts, malformed responses and
process loss keep the exact lease, channel pointers and bot claim. Recovery,
cancellation and readback must not clear these obligations or replay the effect.
Only an exact live claimant may settle confirmed completion, a proven local
pre-send refusal, or an authoritative Telegram error envelope. Rate-limit
refusals preserve bounded retry information and permit a later authorized retry.
Generic HTTP errors are insufficient terminal evidence.

The stored lease format is unchanged, but old replicas can still expire and
replace it. Quiesce connection management mutations and recovery workers, finish
or account for existing calls, replace **all** writers and workers with this
reader/custody implementation, then resume. Mixed old/new writers do not provide
this guarantee. Reverting to expiry-based code while any custody remains can
reintroduce stale remote writes; retain the capable image and recover forward.
Do not edit deadlines/nonces or clear pointers as a recovery shortcut. A crashed
unknown effect may require provider-side proof that it can no longer execute;
Telegram readback and elapsed time alone do not provide that proof.

## Validation

Local barrier tests cover retained custody after cancelled activation and failed
delete, exact late completion, terminal rate-limit refusals and local pre-send
validation failures. The ignored `connections::platform_tests::native_tests`
module requires an owned authenticated disposable Mongo replica set via
`CMA_IM_TEST_MONGODB_URI`. It uses independently authenticated runtime clients,
real CAS writes and a loopback Telegram receiver whose accepted operations
survive HTTP timeouts. Tests cover delayed set/delete, lease expiry, cancellation,
bot claim retention, pre-dispatch expiry and known-unsent read failures. No test
calls Telegram or reads production state.

## Channel custody

`CMAEG_CHANNEL_MUTATION_CUSTODY_ENABLED` defaults to `false` and refuses invalid
boolean values. The same flag configures the serving record client and operator
audits. Once enabled, creation (including requests without a supplied record
UUID), update, deletion and audit unbinds use the channel record's existing CAS
contract. Both Gateway Mongo and the remote HTTP record backend are supported.
No new collection, index, TTL, plaintext projection or Mongo schema is introduced.

The original sealed definition retains a private `mutation` object containing
an immutable operation UUID, original expected revision, exact request digest,
separately sealed bounded target, completed effect identities and the currently
admitted effect. A separate claimant nonce changes only at a safe checkpoint.
Admission and every next step use CAS before dispatch. Rekey, routing updates
(including their immediate-version replay path), other mutations and audits
cannot overwrite held custody. The import path remains insert-only and refuses
an existing different record. Public serialization omits the private marker.

A successful provider response checkpoints its negotiated target before the
next effect. An exact same-owner/same-operation retry can continue completed or
proven-unsent steps; it never resends an admitted unknown step. A competing exact
retry at a safe checkpoint obtains a fresh claimant nonce and revision, so the
previous claimant cannot subsequently dispatch. Known-unsent provider failures
and exactly settled Telegram read/refusal failures are retryable. A timeout,
process loss during dispatch, or failed settlement retains the step. Lost local
write acknowledgements reconcile exact sealed bytes and revisions; retries
resend only local CAS publications. No losing creation compensates by unbinding.

A successful mutation advances several record revisions. Clients must retain
the original request/revision for exact recovery and consume the returned latest
revision after completion. A stable creation UUID remains the recovery handle.
An automatically generated creation UUID can be found through the owner's
record listing if the response is lost; a new request without the UUID is a new
creation. Sealing the complete nested reservation must fit the existing 65,536
byte record limit **before** dispatch; large definitions can therefore be refused
even when the unreserved definition fit. Unknown effects remain held indefinitely
unless the original claimant can durably settle or externally proven terminality
supports a separately reviewed recovery procedure. This patch does not invent
provider-side cancellation or fencing.

## Existing-Agent creation and exact recovery

With the default channel mutation custody gate closed, creation regressions
cover the #1242 existing-Agent report:
non-reserved creation binds before writing a record and preserves recognized
400/404/409 refusals; reserved creation retains an encrypted pending reservation,
refuses ingress with actionable 409 `channel_creation_pending`, and completes on
an exact original request retry. The current provider contract allows multiple
bindings to borrow one owner-controlled Agent; the Mongo uniqueness constraint is
per binding generation. A second borrowing channel is not a binding conflict.

With channel mutation custody enabled, creation always reserves a stable record
identity and follows [retained mutation recovery](EG_Mutation_Custody.md).
An ambiguous provider effect remains held; exact retries cannot resend it until
its original outcome is settled. Pending ingress still returns the explicit
`channel_creation_pending` refusal. Closing the channel mutation custody gate
does not erase that custody or authorize a new operation over it.

An exact concurrent create that loses final publication reads the retained
owner-authorized record and returns the already completed result only when its
original request fingerprint matches. A still-pending or conflicting record
remains a typed conflict. Readback does not replay an external effect or clear
mutation custody. If the confirmation read is unavailable, the original
publication conflict remains the outcome; it is not a provider failure or proof
of completion. Retry the original creation request and reserved record identity.

## Migration and release record

| Requirement | Candidate contract |
| --- | --- |
| Identity / phase | #2196/#2197; expand readers and closed channel writers, next EG release after E1; release lead owns activation |
| Stores / writers | Sealed channel and connection records; create/update/delete/rekey/routing, audit clear, exact retries, connection workers and cancellation; import is insert-only |
| Preservation | Original identities/revision, encrypted intent/results, unresolved custody, channel pointers and bot claims; no expiry/backfill/validator change |
| Reader floor | This candidate's channel/connection semantics and master keys; release must bind exact reviewed commit and immutable image before activation. Earlier strict channel readers reject new markers and older connection workers can discard unknown custody; neither is an allowed recovery artifact |
| Writer gate | Channel flag defaults off; **quiesce all management/audit mutation sources**, account for earlier non-custodied calls, deploy capable serving/recovery artifacts, enable every writer before reopening mutation traffic. Capable but disabled writers are insufficient during activation |
| Closure / recovery | Turning off new channel reservations does not clear existing markers; exact safe retries remain supported. Quiesce before any coordinated writer-policy change; do not interpret closure as permission to run an older image |
| Backfill | None; prior lost remote-effect history cannot be reconstructed from current readback |
| Backup / restore | Operator records protected backup ID/time for both channel and connection records, master-key custody and post-backup effects; rehearse only with a capable reader. No backup or production restore was run here |
| Rollback boundary | First emitted channel marker and any unresolved Telegram effect impose forward recovery. Never remove custody or downgrade records to fit an old image; preserve every key needed by both outer and nested seals |
| Qualification | Independent authenticated Mongo and remote-CAS regressions are local evidence. Exact image, coordinated writer closure, production observation and release sign-off remain deployment obligations |

The storage reservation coordinates management effects and authoritative
publication. It does not make ordinary event delivery and provider execution a
cross-service transaction, nor add a global cache invalidation mechanism.
