# Channel lifecycle recovery

Channel-created Sandboxes retain the original configuring human's authority for
scoped credentials and lifecycle effects. A Bridge's runtime key does not grant
permission to mint, revoke or retire the parent. Every advancing operation
revalidates the original current management and custody proofs against the
retained allocation and resource identities.

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

Final home cleanup, checkpoint disposition, publication of `Retired`, and history
purge remain held. Ordinary `Retired` publication can allow the checkpoint
janitor to delete archives, and ordinary purge removes rows needed by accepted
command and reply recovery. These effects need their own retained ownership and
history contracts before they can be enabled for Channel roots.

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
