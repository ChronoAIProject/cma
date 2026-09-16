# Channel lifecycle recovery

Channel-created Sandboxes retain the original configuring human's authority for
scoped credentials and lifecycle effects. A Bridge's runtime key does not grant
permission to mint, revoke or retire the parent. Every advancing operation
revalidates the original current management and custody proofs against the
retained allocation and resource identities.

## Original home allocation evidence

When the home adapter supplies a configured storage scope, fresh Channel
provisioning commits the exact home request and its opaque scope before calling
prepare. The separate `channel_home_provenance` sidecar binds the allocation,
Sandbox creation time, resource owner and creation approver. Only a successful
provider response for that exact request can bind its first physical receipt;
later stages preserve the request, scope and bound receipt. A lost prepare reply
retries the same durable request. A changed scope or bound physical generation
holds the operation before it can target replacement storage.

The adapter computes scope from its configured provider authority. The domain
stores an opaque digest and does not interpret provider namespaces or endpoints.
Adapters without this evidence capability preserve their existing provisioning
behavior and supply no deletion provenance.

This sidecar does not change the serialized provisioning state older workers
decode. Their existing field updates preserve the separate evidence while
accepted provisioning remains recoverable. Existing homes without provenance
are not backfilled from canonical observation. If an older worker advances past
Home without binding the new sidecar receipt, later stages keep that evidence
incomplete. Future destructive cleanup must hold on absent or incomplete
provenance. A first bound receipt is not proof that an earlier lost request
could never have created another physical generation.

Original allocation evidence alone does not enable home removal, archive
deletion, `Retired` publication, capacity release or a history deadline.

The separate `channel_home_prepare_evidence` sidecar commits before a new
worker calls prepare. It binds the immutable original request/provenance digest
and records that dispatch may have happened, generation history is unresolved,
provider incarnation and backing-storage absence are unproven, and physical
cleanup capability is unavailable. The marker can precede the actual call;
a crash before dispatch and a lost response both require conservative handling.
Its unknown history remains immutable after successful later observations,
receipt binding and old-worker progression. It does not count attempts or claim
that the first recorded response was the only physical generation ever created.

An already-admitted unbound Home row without dispatch evidence records unknown
history before a new worker makes another prepare call. Later phases never
backfill certainty from canonical state. Missing evidence also supplies no
cleanup capability. The new top-level sidecar leaves existing provisioning and
original-provenance serialization unchanged; existing `$set` updates preserve it.

The Kubernetes adapter currently provides configured scope and exact PVC
UID/resource-version deletion preconditions, but no permanent create-operation
ledger, provider-incarnation attestation or backing-disk absence receipt. A PVC
404 proves neither that a lost create cannot appear later nor that a PV/CSI disk
has been reclaimed. The current capability therefore remains unavailable;
this evidence step adds no physical removal operation or provider permission.

## An uncertain key mint

Provisioning, renewal and lifecycle credential replacement record the original
nonsecret mint plan before provider IO. If the reply is lost, CMA retains that
same plan and does not send another mint request. Cleanup queries the original
owner's inventory for the exact planned key name and metadata. A matching key's
identity and authority are recorded durably before scoped revocation.

An empty listing does not prove that an uncertain request cannot still finish.
Ambiguous, duplicate, inaccessible or incomplete inventory remains held. Finding
and revoking a key does not recover its secret or authorize a replacement mint.
The original uncertain operation remains visible for explicit recovery.

NyxID can return 404 for a key that is hidden from the caller. CMA therefore
requires a fresh, complete inventory under the original owner before accepting
absence after key lookup or revocation. A present key, lost organization access,
partial listing or unavailable provider preserves the original debt. Exact
revocation receipts and audits survive worker retries and failed local commits.

## Retirement of a completed Channel root

Before stopping Agents, the dedicated retirement path checks the current
original-human authority and the complete original Sandbox, Workspace, Worktree
and Agent creation evidence. An additional or misindexed child, changed origin,
foreign ownership or replaced custody prevents retirement from dispatching an
effect. Reusing an ordinary parent for a channel child never authorizes this
whole-parent retirement path.

Initial retirement requires an exact runtime binding at the durable runtime
generation high-water mark. A missing binding is insufficient: a canceled
revival can have an in-flight creation request. Paused or unbound roots and
uncertain runtime reservations remain held until an explicit absence contract
can account for those effects.

For an eligible root, CMA records stop intent before calling the ordinary Agent
stopper. It waits for the exact original child to reach a terminal state before
revoking scoped credentials. Each current, pending or retained prior credential
is checked against encrypted custody. Credential deletion, debt removal, progress
and audit publish in one transaction; an uncertain result retains the original
identity for recovery.

Runtime teardown also has a durable intent. A lost teardown response requires
the adapter's complete runtime inventory and absence of the exact retained
runtime handle. Unavailable, duplicate, oversized or contradictory inventory
does not complete cleanup. The runtime binding is cleared only with the matching
durable receipt and audit.

After runtime teardown, the dedicated path records a storage disposition under
the same original-human, allocation, tree and runtime proofs. It retains the
observed home identity, canonical checkpoint references and a bounded inventory
of published archives. The archive writer gate closes in the same transaction;
previously published bytes remain readable. In-flight unpublished uploads keep
their existing cleanup stages and permanent late-upload tombstones.

The disposition retains home data, published archives, checkpoint metadata,
resource rows, accepted command/control/source/reply identities, idempotency
tombstones and history. It is not permission to delete physical storage.
Generic checkpoint and row purge refuse Channel markers or retained Channel-root
allocation evidence, including malformed markers or stripped root sidecars.
Ordinary parents and unrelated storage keep their existing cleanup policy.

The root remains `Retiring` at `MarkRetired`, without a purge deadline or capacity
release. Final home cleanup, archive-body deletion, publication of `Retired`, and
history purge remain held. An older checkpoint janitor discovers `Retired`
roots without understanding this new disposition, so final publication also
requires compatible reader/janitor evidence. Physical cleanup needs complete
original-home provenance, exact ownership/absence receipts and an explicit
archive retention policy before it can be enabled.

## Shared parents and remaining work

Shared parents retain their ordinary lifecycle and credential workers. Their
channel-created children can resume and reply through the original accepted
commands under the authority described in
[Shared parent Bridge placement](CMA_Bridge_Shared_Placement.md).

Allocation-owned children have a separate explicit cleanup path that retains
the parent, unrelated siblings and history. Whole-Workspace removal requires
separate consent and proof that CMA created that exact Workspace for the
allocation. Durable removal receipts fence each effect, restore reconciles
retired paths before reopening conversations, and reserved capacity is released
only after complete removal proof. See [Child cleanup](BRIDGE_CHILD_CLEANUP.md)
and [Workspace cleanup](BRIDGE_WORKSPACE_CLEANUP.md). Binding reset retains old
resources and does not imply cleanup or capacity release.

Local provider fixtures and disposable persistence/runtime tests establish only
the behavior exercised by their recorded source. They do not establish live
Telegram delivery or production upgrade/rollback continuity. Channel image
rotation and the complete lifecycle/retention acceptance matrix remain pending.
