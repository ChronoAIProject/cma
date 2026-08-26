# Sandbox provider: observed production contract

This document records what the hosted sandbox provider actually did when it
was driven against production on 2026-08-23 UTC. It is evidence, not a design
for how a provider ought to behave. The complete request, response, Kubernetes
event, and five-second timeline record is attached to issue #539.

The observation target was:

| Surface | Observed value |
|---|---|
| Kubernetes context | `gke_example-project-02_asia-northeast1_example-cluster-02` |
| Control-plane namespace | `chrono-bot` |
| Deployment identity | `chrono-bot-prod` |
| Control-plane image | `chrono-bot-api:v0.6.0` |
| Runtime image | `chrono-bot-runtime:v0.6.0` |
| Runtime digest | `sha256:766a99c01a3f45a57296faaf680f88bd1e7720ff6584cbba46239e94253e3eb6` |
| OpenSandbox app/chart | `0.1.0` / `opensandbox-server-0.1.2` |
| OpenSandbox image | `opensandbox/server@sha256:4b386f107a4222320928b0b4dd38df8dc5154250ea4c90b4d36767a62f69ce7c` |
| OpenSandbox exec helper | `opensandbox/execd:v1.0.20` |
| Sandbox namespace | `chrono-bot` |

The provider credential is not part of this record. Every Kubernetes command
used to establish these facts was read-only and context-pinned. Temporary
sandboxes were created and removed through the provider API, all inside the
`chrono-bot` namespace.

## Decisions established by the probe

| Probe | Observed answer | What code may claim |
|---|---|---|
| Per-sandbox limits | **Honoured.** Pod requests and limits exactly matched three different requested shapes. | A hosted runtime receives the reservation rendered by `SandboxShape::limits_for`. The provider response does not echo that reservation, so code must not claim an independently confirmed applied value from `SandboxStatus`. |
| Over-capacity request | **Asynchronous.** The POST produced no bytes before the 240-second client timeout. The provider object stayed `Pending / BATCHSANDBOX_PENDING`; only a Kubernetes event named the admission failure. | A deployment-side ledger can refuse the predictable case. This provider exposes no specific late capacity signal that `SandboxClient` can safely map to a named error. |
| Guest resource interfaces | **No requested-limit denominator.** The gVisor guest exposed cgroup v1 with unlimited memory and CPU sentinels, no cgroup v2 files, and no `/sys/class/net`. | The supervisor may report the v1 usage counters and may fall back to `/proc/net/dev` where it exists. The hidden limits, v2 pressure and OOM counters remain absent; a requested reservation is not an observed denominator. |
| Death reason | **Does not name the cause.** A kernel-proven OOM became provider `Pending / CREATING`; timeout and delete became immediate not-found responses. | The provider can establish that a runtime is no longer serving. It cannot provide an OOM verdict for this build. |

These answers are specific to the versions and context above. A provider or
runtime upgrade does not inherit them by assumption. Re-run the probes before
changing a capability or mapping on the strength of new behaviour.

## 1. Listing and sandbox correlation

Before any probe sandbox was created, `GET /v1/sandboxes` returned:

```json
{
  "items": [],
  "pagination": {
    "page": 1,
    "pageSize": 20,
    "totalItems": 0,
    "totalPages": 0,
    "hasNextPage": false
  }
}
```

This is the envelope consumed by `SandboxClient::list` in
`api/crates/layer3-adapter/src/sandbox.rs`. A listing is paged in groups of 20;
the `pagination` object, particularly `hasNextPage`, is authoritative about
whether another page exists.

Because the initial listing was empty, the first control sandbox established
the provider-to-Kubernetes mapping:

- sandbox `ID` maps to pod `ID-0`;
- pod label `opensandbox.io/id` carries the complete sandbox ID;
- pod label `batch-sandbox.sandbox.opensandbox.io/name` carries the same ID;
- metadata supplied to the API is copied to pod labels;
- the sandbox is owned by a `sandbox.opensandbox.io/v1alpha1` `BatchSandbox`
  whose name is the sandbox ID.

At the time of this probe, the historical metadata key
`chrono-bot/control-plane` was consumed by `HostedKernel::list_runtimes` and
reconciliation. Every probe used the value `sizing-probe`, not
`chrono-bot-prod`, so the production reconciler was not entitled to adopt or
reclaim it.

Current provisioning writes `cma/control-plane`, `cma/session`, and
`cma/kernel` as the canonical keys and duplicates the same values under their
historical `chrono-bot/*` counterparts. Reconciliation prefers each canonical
ownership value and falls back to the historical value when it is absent. The
fallback cannot be removed until a complete provider listing proves that no
live pre-rename sandbox still depends on it.

## 2. Per-sandbox reservations

Three otherwise identical requests carried these `resourceLimits` values:

| Control | Requested | Pod requests | Pod limits |
|---|---|---|---|
| A | `cpu=1`, `memory=1Gi` | `1`, `1Gi` | `1`, `1Gi` |
| B | `cpu=2`, `memory=2Gi` | `2`, `2Gi` | `2`, `2Gi` |
| C | `cpu=500m`, `memory=512Mi` | `500m`, `512Mi` | `500m`, `512Mi` |

Each POST returned HTTP 202 after the sandbox reached `Running`; each raw GET
returned HTTP 200. The raw provider object contained its image, entrypoint,
metadata, lifecycle status, and timestamps. It did **not** contain requested or
applied resources.

The answer is **honoured**, not clamped or ignored. `HostedKernel::provision`
in `api/crates/layer3-adapter/src/hosted/mod.rs` may therefore continue to
render each `RuntimeCapacity` through `SandboxShape::limits_for` into the
request's `resourceLimits` map.

The evidence boundary matters for metering: the service knows what it asked
for, and the observed pod spec matched it. The provider's public sandbox
response does not independently echo what was applied. Price is consequently
based on the pinned reservation, never on a field claimed to have come back
from the provider.

### Namespace policy in force

At the start of the probe, namespace `chrono-bot` declared:

```yaml
ResourceQuota:
  limits.cpu: "40"
  limits.memory: 80Gi
  pods: "50"
  requests.cpu: "20"
  requests.memory: 40Gi

LimitRange:
  default:
    cpu: "1"
    memory: 1Gi
  defaultRequest:
    cpu: 100m
    memory: 256Mi
  max:
    cpu: "4"
    memory: 8Gi
```

No second `LimitRange` applied to this namespace. The three controls were all
inside the range, and the observed pod values matched the request rather than
a default.

## 3. An impossible reservation

The over-capacity control requested `cpu=1000` and `memory=1000Gi`, while the
namespace `LimitRange` allowed no more than 4 CPU and 8Gi for one container.
Its POST behaved as follows:

1. OpenSandbox created a `BatchSandbox` but returned no HTTP headers or body.
2. Provider listing and GET reported `Pending`, reason
   `BATCHSANDBOX_PENDING`, message `BatchSandbox is pending allocation`.
3. No pod was ever created. The `BatchSandbox` stayed at `allocated=0`,
   `ready=0`, and `replicas=0`.
4. Kubernetes repeated a `FailedCreate` event naming both `LimitRange`
   violations.
5. After 240,005 milliseconds, the client timed out with no received bytes and
   no HTTP status. Ending the request context removed the `BatchSandbox`.

The decisive Kubernetes event was:

```text
maximum cpu usage per Container is 4, but limit is 1k
maximum memory usage per Container is 8Gi, but limit is 1000Gi
```

The provider exposed none of that text and no machine-readable capacity code.
This timeout occurs inside `SandboxClient::create`, whose `PROVIDER_TIMEOUT` is
240 seconds. It happens before `HostedKernel::await_running` receives an ID and
can poll lifecycle state.

`BATCHSANDBOX_PENDING` is an allocation state, not a capacity verdict. Mapping
that word itself to `AdapterError::AtCapacity` would turn an ordinary slow
allocation into a false refusal. No code may make that mapping from this
record.

The practical contract is therefore:

- a declared deployment ledger is required to refuse predictable exhaustion
  before an identifier, record, credential, or sandbox is created;
- a successful ledger check is not proof that the shared provider has room;
- this provider build offers no sound late mapping from its public API to a
  typed capacity error;
- a late failure remains an availability failure until the provider exposes a
  specific signal.

## 4. Kernel resource interfaces

Controls A and C were read from inside their running containers as the image's
declared user, uid and gid 10001. Kubernetes applied their different limits,
but the gVisor guest presented this cgroup membership:

```text
7:pids:/<guest-cgroup>
6:memory:/<guest-cgroup>
5:job:/<guest-cgroup>
4:devices:/<guest-cgroup>
3:cpuset:/<guest-cgroup>
2:cpuacct:/<guest-cgroup>
1:cpu:/<guest-cgroup>
```

This is cgroup v1. Every cgroup v2 path read by
`layer4_runtime::resources::Sampler` was absent:

```text
/sys/fs/cgroup/memory.max
/sys/fs/cgroup/memory.current
/sys/fs/cgroup/memory.peak
/sys/fs/cgroup/memory.events
/sys/fs/cgroup/memory.pressure
/sys/fs/cgroup/cpu.max
/sys/fs/cgroup/cpu.stat
```

The available v1 denominator files did not reflect the pod reservation:

```text
/sys/fs/cgroup/memory/memory.limit_in_bytes = 9223372036854775807
/sys/fs/cgroup/cpu/cpu.cfs_quota_us         = -1
/sys/fs/cgroup/cpu/cpu.cfs_period_us        = 100000
```

Both different-size controls returned the same unlimited sentinels. Numeric
`memory.usage_in_bytes` and `cpuacct.usage` existed. The supervisor reads those
usage counters but treats the sentinels as absent limits, so it does not draw a
misleading percentage. The v2 pressure and OOM event counters remain absent.

`/sys/class/net` was absent, so interface names and their `rx_bytes` and
`tx_bytes` counters could not be listed. The probe did not record
`/proc/net/dev`; the sampler uses its non-loopback counters only as a fallback
when that proc file is present and sysfs has no usable interface.

The top-level `/sys/fs/cgroup` tmpfs was mounted read-only; controller mounts
were readable. The pod spec did not set `readOnlyRootFilesystem` (the
Kubernetes default is false), dropped `NET_RAW`, used RuntimeDefault seccomp,
and ran the application as uid 10001.

The supervisor represents missing reads as absent fields and advertises the
resource lane in readiness only when a sample contains an observation beyond
its monotonic clock. An uptime-only frame is neither sent nor forwarded by the
control plane, so the route's existence alone cannot imply useful reporting.

## 5. Death and reclamation

Three provider lifecycle paths were observed independently. Each was sampled
every five seconds for at least three minutes. The full 37-sample timelines
are on #539.

### OOM kill

A sandbox was started with a 256Mi reservation. After a five-second delay, its
process touched 16Mi allocations until the memory cgroup killed it. The POST
first returned HTTP 202 with `Running / RUNNING`.

At 2026-08-23T16:56:31Z, Kubernetes recorded container exit code 128 and reason
`Error`. The read-only node journal tied the exact pod UID and container ID to:

```text
Memory cgroup out of memory: Killed process 274807 (exe)
A process of this unit has been killed by the OOM killer.
TaskOOM event
received container exit event ... exit_status:128
```

After the death, every provider sample said:

```json
{
  "state": "Pending",
  "reason": "CREATING",
  "message": "Sandbox is being created"
}
```

`Sandbox::is_running` correctly returns false, so reconciliation may treat the
runtime as non-serving. `hosted::loss::loss_reason` cannot derive an OOM verdict
from this response. This reproduces the production incident recorded on #464:
the kernel proves OOM while Kubernetes presents exit 128 / `Error`, and the
provider does not name memory as the cause.

### Provider timeout

A running sandbox was created with `timeout=60`. Its POST response carried an
exact `expiresAt` timestamp. It remained `Running / RUNNING` through the last
sample before that timestamp. At the expiry second Kubernetes recorded
`Killing: Stopping container sandbox`; the first GET after expiry returned:

```json
{
  "code": "KUBERNETES::SANDBOX_NOT_FOUND",
  "message": "Sandbox '<id>' not found"
}
```

The provider exposed no terminal state or reclaim reason between running and
not found.

### Normal delete

DELETE against a running control returned HTTP 204. The first following GET,
12 seconds later, returned the same `KUBERNETES::SANDBOX_NOT_FOUND` problem,
and all later samples remained not found. Kubernetes recorded the container
stopping one second after DELETE.

For callers, delete is synchronous: a 204 means the sandbox is no longer
addressable. There is no terminal listing interval from which a normal-delete
reason can be recovered.

## 6. Cleanup and revalidation

Every sandbox created by the probe carried
`chrono-bot/control-plane=sizing-probe`. Each was deleted through the provider
or was already removed by the provider and confirmed absent. The final provider
listing was the same empty envelope recorded at the start. Kubernetes reported
zero matching pods and zero matching `BatchSandbox` objects.

At 2026-08-23T17:08:12Z, namespace quota usage was back at the exact baseline:
two pods, 125m requested CPU, and 192Mi requested memory. The API and web
deployments each remained ready and available.

Re-run this record when any of these change:

- the OpenSandbox server or exec helper version;
- the runtime class or gVisor version;
- the runtime image's user or supervisor sampler;
- the namespace `LimitRange` or `ResourceQuota`;
- provider status or error response vocabulary.

Do not turn a new provider word into a capability, capacity refusal, or loss
reason until its request and lifecycle have been driven and recorded again.

## 7. Workspace capture transport discovery

Issue #662 asks whether the pinned provider can capture and restore a complete
repository workspace without adding a new Layer 4 wire lane. This section
records the bounded production discovery result from 2026-08-26. It does not
advertise workspace recovery as an implemented CMA capability.

### Deployed production surface

CMA's production `CHRONO_SANDBOX_BASE_URL` names the `opensandbox-server`
service, not the separately installed `opensandbox-server-codex` service. The
observed deployment is:

| Component | Observed value |
|---|---|
| Helm chart / chart app version | `opensandbox-server-0.1.2` / `0.1.0` |
| Server image | `opensandbox/server@sha256:4b386f107a4222320928b0b4dd38df8dc5154250ea4c90b4d36767a62f69ce7c` |
| Installed Python distribution | editable `opensandbox-server 0.1.0.dev0` from `/app` |
| Controller | `opensandbox/controller:v0.2.0` |
| Execd | `opensandbox/execd:v1.0.20` |
| Provider HTTP proxy timeout | `httpx.AsyncClient(timeout=180.0)` |
| Maximum requested sandbox lifetime | 3,600 seconds |

The chart version is not the server source version. In particular, the live
image's API does not match the previously inspected upstream `server/v0.1.2`
tag, so that tag and its commit cannot be used as evidence for this deployment.
The image digest, live OpenAPI document, installed distribution, and driven
responses above are the production record. No source commit is claimed for an
image that does not publish one in its observed metadata.

The live OpenAPI document advertises all of:

- `POST /v1/sandboxes/{sandbox_id}/snapshots`;
- `GET /v1/snapshots` and `GET`/`DELETE /v1/snapshots/{snapshot_id}`; and
- `CreateSandboxRequest.snapshotId` for restore.

Therefore "snapshot routes are absent" is not a valid explanation for the
production limitation.

### Native snapshot result

A disposable sandbox was snapshotted through the live API. Creation returned
HTTP 202 and snapshot `ed464609-9b9b-4ad4-9269-7f4585ccdbf0`, but its terminal
state was:

```text
state:   Failed
reason:  RegistryNotConfigured
message: snapshot-registry not configured in controller manager
```

The source sandbox subsequently expired and returned HTTP 404. The failed
snapshot was deleted explicitly (HTTP 204) and then returned HTTP 404. The
schema is present, but native snapshot and restore are not operational in this
production installation. CMA must not claim or depend on them unless the
registry is configured and a new round trip is demonstrated.

### Execd archive round trip

The deployed Execd version's upstream source is pinned at
`ae452d4af7b6415b5074fd7e075b97ff7a5ac236`. Its production endpoints were
reachable through the provider-authenticated proxy at:

```text
POST /v1/sandboxes/{id}/proxy/44772/command
GET  /v1/sandboxes/{id}/proxy/44772/files/download?path=...
POST /v1/sandboxes/{id}/proxy/44772/files/upload
```

Two disposable sandboxes tagged `cma-trigger-readiness` drove this sequence:

1. Execd created a nested text file, binary file, executable file with mode
   `755`, and relative symlink, then archived the workspace.
2. The 6,144-byte archive was downloaded with HTTP 200. Its source and local
   SHA-256 values both were
   `f418af1073e79d48df6c4d8b5a92dbf0e0cc7f096a47ae26a331120a336ac726`.
3. A `Range: bytes=0-63` request returned HTTP 206, the correct
   `Content-Range: bytes 0-63/6144`, and bytes matching the full archive's
   prefix. The route supports ranged download.
4. The full archive and a deliberately truncated 64-byte artifact were each
   accepted by multipart upload with HTTP 200. Execd and the provider supplied
   no `ETag`, `Digest`, `Content-MD5`, or checksum header, so the destination
   compared each uploaded SHA-256 with the source manifest before extraction.
   The truncated artifact failed that comparison and was not extracted.
5. After full-archive verification, extraction reproduced the text and binary
   hashes, executable mode `755`, executable size, and symlink target exactly.
6. Both sandboxes were deleted through the provider with HTTP 204 and each
   returned HTTP 404 afterwards. Local archive and header artifacts were also
   removed.

This demonstrates byte transport, ranged reads, multipart writes, and an
application-level integrity check on the exact production provider. It does
not demonstrate a resumable upload. The provider proxy has a 180-second HTTP
client timeout and advertises no request-size or integrity bound for this
route. A production protocol must impose its own maximum archive size,
deadline, manifest hash, and retry rules; it must treat every interrupted or
unknown upload as uncommitted until the complete destination hash is proven.

### Consistency result and transport decision

The round trip deliberately used an idle fixture. Execd can run `tar` and
`sync`, but neither its file API nor the provider exposes a pause, filesystem
freeze, atomic volume snapshot, or proof that the agent and its descendants
have stopped writing. CMA's current `rotating` state refuses new client input,
but an already-running agent turn or background process can continue changing
the workspace. The current Layer 4 wire declares terminal, conversation,
conversation-gap, and telemetry lanes only; none provides a workspace writer
fence or quiescence acknowledgement.

Consequently the provider-only branch fails #662's consistency requirement.
The selected design is the explicit fallback: a versioned Layer 4 workspace
checkpoint capability with a staged runtime-image rollout. Its protocol must:

1. fence the checkpoint with the Launch workspace epoch and refuse concurrent
   captures;
2. stop admitting new work and positively establish that all supported
   workspace writers are quiescent before capture;
3. flush writes, create a bounded archive and versioned manifest, and report
   size plus content hashes before the old runtime may be torn down;
4. commit durable object storage and the checkpoint record atomically from the
   Launch's perspective, leaving the previous checkpoint valid on failure;
5. verify the manifest and materialized tree before starting the agent in a
   replacement runtime; and
6. fail closed to recovery-required, or resume the still-live old runtime,
   whenever quiescence, capture, upload, commit, or restore cannot be proven.

The demonstrated Execd path may be reused only as a bulk relay for an already
sealed, hash-addressed archive. It is not the consistency authority. An old
runtime image that does not advertise the new checkpoint capability is
ineligible for CMA Trigger, and no rollout may rotate an existing live session
until the complete conversation-plus-workspace handover has itself been
demonstrated.
