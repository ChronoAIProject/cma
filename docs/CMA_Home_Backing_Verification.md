# Provider cleanup with existing permissions

Channel retirement uses the provider interfaces and namespaced Kubernetes
permissions CMA already has. It requires no BatchSandbox watch, PersistentVolume
read, cluster role, or Google Compute disk access. The service-account manifest
is unchanged by this feature.

## Runtime authority

A fresh runtime dispatch is journaled before the provider POST. A successful
response identifies the logical provider allocation. Before CMA binds that
runtime, installs software or delivers credentials, the adapter reads the exact
namespaced object and pins its UID, generation, specification and ownership
metadata. It rechecks the same incarnation before binding it.

This receipt identifies the runtime CMA adopted. It does not claim that a later
read recovered the first Kubernetes object ever created for the provider handle.
The provider controls its creation process; CMA does not reconstruct historical
creation events. A lost POST response remains unresolved and cannot be replaced
with a synthetic creation acknowledgement from discovery.

Retirement checks that pinned incarnation, deletes with UID and resource-version
preconditions, and records the actual deletion acknowledgement. Complete bounded
inventories must then show that the runtime and its owned Pods are absent.
Replacement identities, incomplete inventories and uncertain acknowledgements
remain held. Reused parents and unrelated workloads are outside this authority.

## Persistent home authority

The actual successful PVC creation response binds the original home request,
provider scope and claim UID. Existing claims and reconciliation reads do not
supply a missing creation acknowledgement. The claim is verified against its
recorded owner, allocation and request before cleanup.

After runtime teardown and the existing owned-capture cleanup, CMA deletes only
that original claim with UID and resource-version preconditions. It journals the
actual response before waiting for the claim to disappear. A delayed deletion
can be observed again using that acknowledgement; a lost acknowledgement cannot
be inferred from a subsequent 404.

The final home receipt proves completion of the original namespaced claim's
lifecycle. Underlying PV and disk reclamation remains the storage provider's
responsibility under the configured storage policy. CMA does not call a missing
PVC proof that the physical disk has been erased, and it does not read or delete
cloud disks directly.

## Publication and retention

Provider receipts remain bound to the original allocation, generation and
retirement operation. The final transaction verifies them with the retained
history inventory, seals material writers, clears the canonical runtime/home
attachments and publishes `Retired`. Original attachments and receipts remain
in permanent recovery records.

History, command and reply identities, control outcomes, replay aliases and
archive cleanup tombstones retain their existing policy. No generic purge
deadline is introduced. Legacy or ambiguous dispatches without the required
original acknowledgements remain explicitly held; absence alone does not grant
new cleanup authority.
