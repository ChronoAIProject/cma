# Bridge allocation foundation

This foundation reserves and retains one allocation generation for a normalized
partition. It provides storage and authority contracts for #863/#865. It does
not provision a Sandbox, submit an Agent command, deliver a reply, or establish
an end-to-end exactly-once guarantee.

## Identity and reservation

`AllocationAdmission` consumes a checked `NormalizedPartitionSource` and
rechecks its exact owner, accepted configuration pin and partition admission.
Deserialized key strings and inbox rows alone cannot construct an admitted
source. The partition domain owns canonical key encoding and verified platform
evidence.

The durable head is unique for `(bridge ID, canonical key version/hash)`. Each
generation has a deterministic ID derived from that identity and an explicit
allocation generation counter. The allocation counter is independent of the
Bridge configuration revision. The Mongo adapter additionally enforces a unique
index on `(bridge ID, key hash, key version, allocation generation)`.

Before effects, the record pins the original configuration, source inbox/body
hash, owner, placement, selected profile snapshots, resource IDs and provisioning
operation ID. New Sandbox/Workspace/Worktree/Agent IDs are deterministic from the
allocation ID and resource kind. Concurrent first messages and retries therefore
refer to the same identities. Ordinary configuration changes reuse the current
head; changing the retained resource tree requires an explicit allocation reset.

## Separate authority boundaries

For an existing Agent, reservation consumes a current `BridgeInvokeProof` for
the exact `BindExistingTarget { allocation_id }` effect. It checks the exact
target and retained lineage, grant, owner, configuration and selected custody.
The record becomes `ready` without provisioning any resource. The Mongo invoke
adapter fences grant revocation and current resource lineage in the same
transaction as the allocation write. Later message admission and execution use
the invoke domain's separate effects.

For a new Sandbox, `AllocationProfilePlan` requires actual current Bridge
management authority, exact selected snapshots, an accepted digest of all three
snapshots, and trusted live quota/runtime/model/service/skill observations. It
checks content digests, selected revisions, conflicting selected ancestry and
Sandbox capacity bounds. The provisioning proof additionally requires separate
current `BridgeValidation`, whose NewSandbox branch requires renewable creation
authority. An invoke proof cannot authorize creation.

The creation plan is exactly one clean Sandbox, one selected Workspace, one
Worktree and one selected Agent. The original snapshot contents are retained.
A future provisioning adapter must suppress automatic child profile expansion
and must not clone a live home directory, credentials or conversation history.
The store does not implement that adapter or the existing resource-admission
composition. Capability observations are trusted adapter inputs and are not
deserializable API or callback payloads.

Authority is request-local and bounded by the original management/provider
deadlines. `prepare_for_generation` can renew the original accepted generation
only under explicit drain policies. It retains the old snapshots, configuration
and selected custody even after a key rotation. Suspend policies and execution
stops fail closed. New reservation still requires current ingress admission.
There is no conversion from an external sender or an allocation record to an
ordinary owner `CallerContext`.

The current NyxID adapter still refuses NewSandbox setup when renewable creation
authority is unavailable. Durable owner-approved autonomous profile-plan
recovery and the production renewable provisioning adapter remain integration
work; the current profile-plan API uses a fresh management lease.

## Progress and recovery

Records distinguish `reserved`, `provisioning`, `ready`, `needs_connection`,
`failed_retryable`, `failed_terminal` and `retired`. A lease contains holder,
monotonic fence number and wall-clock expiry. Every mutation compares the full
previous record, revision and fence; the adapter rechecks lease expiry after IO
and immediately before commit. A lease cannot extend an authority deadline.

`Start` records that the original provisioning operation may have begun. Once
this flag is set, another `Start` is rejected, including after takeover and
retryable failure. A worker must reconcile the original operation. Confirmation
requires the original owner, operation ID, exact resource IDs, selected profile
digest and a receipt digest supplied by the trusted reconciliation adapter.

Reset requires an actual current management lease and the exact predecessor.
It refuses an active lease or unresolved started effect. The transaction retires
the predecessor, inserts generation N+1 and advances the head atomically. It
retains old resource identities, configuration, profile snapshots and history.
Retired resource trees continue to count against Sandbox capacity; an explicit
authorized deletion/reconciliation path must release that capacity in future
work. Reset never deletes resources.

The Mongo store checks exact selected encrypted custody and writes the shared
Bridge definition in the allocation transaction. This serializes capacity
admissions and conflicts with disable, rotation and custody revocation. Fixed
allocation additionally uses the shared invoke/resource fence. All collection
documents, including partition heads, carry a document schema version.

An unknown commit returns `Unavailable`. Deterministic reservation and progress
identity permit subsequent retries to observe the already committed record;
uncertainty never permits another resource tree. This is a storage guarantee,
not proof that an unimplemented provider effect or Agent command is exactly once.

## Ordinary owner capacity

Fresh Channel Sandbox admission leaves one free slot within the owner's current
live Sandbox limit for ordinary UI or Trigger creation. The transaction counts
all live Sandboxes for that personal or organization owner, across all Bridges,
under the same owner serialization fence as ordinary admission. A limit of one
therefore permits no fresh Channel Sandbox. Capacity refusal inserts no Sandbox,
resource policy, creation event or started-effect linkage; the retained
allocation can retry with fresh authority when capacity becomes available.

An exact already-admitted held Sandbox recovers its original receipt before the
fresh-admission limit is checked. This does not revoke retained resources or
change live profiles or owner limits. Shared-parent placement continues using
its separately configured Workspace/Agent reserves. The owner slot reserve is
one bounded quota rule; broader operational limits remain tracked in #868.

The reserve changes admission behavior without changing the stored schema.
Older Channel writers still apply the full owner limit and can consume the
last free slot. Enabling Channel creation with mixed writer versions therefore
remains subject to the #850 deployment gate; the reserve is not a guarantee
until every Channel admission writer enforces it. Existing admitted resources
remain recoverable throughout that upgrade.

## Verification boundary

Focused core tests cover stable identity, profile/trust pinning, original
authority expiry, lease expiry at commit, lost-receipt takeover, original
generation draining, failure states and rejection of substituted resources.
Disposable Mongo fixtures cover first-message and lease races, restart,
transactional reset/history/capacity, custody revoke before new and replayed
reservation, crash reconciliation, old-key draining versus suspension and
corrupted index data. A separate invoke fixture covers fixed allocation and
grant revocation. Fixture observations do not establish live provider capability
or production platform behavior.

Accepted-inbox work selection, renewable autonomous creation, truthful
ChannelBridge resource-creation admission, ordered Agent command correlation,
outbox/reply delivery and rollout remain subsequent work.

## Public partition observations

[Bridge partition status](BRIDGE_PARTITIONS.md) documents the authorized list and
detail GET routes, exact retained targets, first-source labels and bounded
Channel response correlation. These observations preserve native conversation
continuity and do not grant authority for allocation or cleanup effects.
