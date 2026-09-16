# Original home backing verification

Channel allocation retirement preserves a home until the provider can account
for its original backing storage. The original record binds the home allocation,
Kubernetes provider scope, PVC generation, PV name and UID, CSI driver, exact
volume handle and backing disk generation. Capture happens during original
allocation. Retirement cannot fill in missing original evidence from the disk
currently using the same name.

The GKE Persistent Disk observer supports the `pd.csi.storage.gke.io` driver and
exact zonal or regional handles:

```text
projects/PROJECT/zones/ZONE/disks/DISK
projects/PROJECT/regions/REGION/disks/DISK
```

It reads only that disk through the fixed Google Compute API origin. Original
capture corroborates the disk name, self link, zone or region, and numeric disk
ID. Cleanup requires the separately recorded, UID-fenced claim deletion and an
authoritative absence observation for that original backing. A PVC 404 does not
prove disk deletion. A present disk or a replacement disk with another numeric
ID keeps cleanup held.

The observer uses the existing default workload service account through the
fixed Google metadata token endpoint. It adds no secret or configuration key,
changes no IAM policy and issues no disk-list or disk-delete requests. The
workload must already have permission to read the exact disk. Authorization
failures and unavailable credentials remain unresolved observations.

Both origins use bounded request deadlines and response sizes. Redirects and
HTTP proxy forwarding are disabled. Disk paths reject alternate origins,
traversal, query strings and unsupported CSI drivers. An absence receipt requires
the Compute API's structured 404 `notFound` response; a proxy error page or a
permission error cannot become absence. Receipt correlation IDs identify CMA
read observations and do not claim a Google deletion operation occurred.

The provider adapter composes this observer with original-allocation capture,
runtime shutdown and claim deletion. The standalone observer does not itself
authorize retirement, release capacity, remove history or mutate storage.
Legacy allocations without their original backing pin retain an explicit hold.

The control-plane service-account manifest includes the Kubernetes read access
needed for fresh capture. The existing namespaced cleanup role adds
`batchsandboxes/watch` so an original creation can be observed from its
pre-dispatch resource version. The `cma-home-backing-observer` ClusterRole grants
only `persistentvolumes/get`, bound only to `chrono-bot/chrono-bot-ksa`; the
adapter reads the exact PV named by the owned PVC. It cannot list or modify PVs.
These source-controlled RBAC resources enable complete cleanup proof capture;
they are not prerequisites for Channel messaging or ordinary provisioning. They
neither restart runtimes nor grant cloud disk permissions. Without these reads,
provisioning can continue while missing evidence keeps final retirement held.
Granting access later cannot retroactively establish a missing original identity.

Fresh Channel provisioning writes a dispatch journal before each physical home
or runtime creation attempt. A home creation receipt comes only from the actual
successful PVC POST response. Existing claims and reconciled responses cannot
acquire this proof. The original PVC UID remains pinned while the first runtime
starts; after storage binding, the adapter checks the exact claim, PV claimRef,
PV UID and CSI handle before and after the original disk-generation observation.
Unavailable storage permissions preserve ordinary provisioning but leave final
retirement held. Multiple or unresolved dispatches also remain held.

Retirement persists the actual runtime deletion proof and the actual
UID/resourceVersion-preconditioned claim deletion response in separate fenced
transactions. Retries use those durable acknowledgements while CSI disk deletion
is pending. The home can reach finality only after the exact backing observer
reports authoritative absence. The final transaction retains those provider
receipts and the original home attachment in permanent recovery records, seals
retained history, and clears the canonical home while publishing `Retired`.
The platform does not infer successful deletion from a lost response.
