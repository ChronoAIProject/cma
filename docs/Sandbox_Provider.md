# Sandbox provider: observed production contract

This document records what the hosted sandbox provider actually did when it
was driven against production on 2026-08-23 UTC. It is evidence, not a design
for how a provider ought to behave. The complete request, response, Kubernetes
event, and five-second timeline record is attached to issue #539.

The observation target was:

| Surface | Observed value |
|---|---|
| Kubernetes context | `gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke` |
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

The observed listing was paged in groups of 20. The `pagination` object,
particularly `hasNextPage`, is authoritative about whether another page
exists.

Because the initial listing was empty, the first control sandbox established
the provider-to-Kubernetes mapping:

- sandbox `ID` maps to pod `ID-0`;
- pod label `opensandbox.io/id` carries the complete sandbox ID;
- pod label `batch-sandbox.sandbox.opensandbox.io/name` carries the same ID;
- metadata supplied to the API is copied to pod labels;
- the sandbox is owned by a `sandbox.opensandbox.io/v1alpha1` `BatchSandbox`
  whose name is the sandbox ID.

Every observation sandbox used the metadata value `sizing-probe`, distinct
from the production deployment identity. The provider copied that metadata to
pod labels, which made the temporary objects independently identifiable.

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

The answer is **honoured**, not clamped or ignored: each requested value was
rendered into the resulting pod specification without adjustment.

The evidence boundary matters for metering: the service knows what it asked
for, and the observed pod spec matched it. The provider's public sandbox
response does not independently echo what was applied. Callers must not
present a provider response field as independent confirmation of the applied
reservation.

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
The observation client timed out after 240 seconds. It received no identifier
that could be used to poll lifecycle state.

`BATCHSANDBOX_PENDING` is an allocation state, not a capacity verdict. Mapping
that word itself to a capacity error would turn an ordinary slow allocation
into a false refusal. No code may make that mapping from this record.

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

This is cgroup v1. The following cgroup v2 paths were absent:

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
`memory.usage_in_bytes` and `cpuacct.usage` existed. The v2 pressure and OOM
event counters were absent.

`/sys/class/net` was absent, so interface names and their `rx_bytes` and
`tx_bytes` counters could not be listed. The observation did not record
`/proc/net/dev`.

The top-level `/sys/fs/cgroup` tmpfs was mounted read-only; controller mounts
were readable. The pod spec did not set `readOnlyRootFilesystem` (the
Kubernetes default is false), dropped `NET_RAW`, used RuntimeDefault seccomp,
and ran the application as uid 10001.

These observations do not establish that every runtime image or provider
version exposes the same counters. Missing values must remain distinguishable
from zero values.

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

The response establishes only that the sandbox is non-serving. It cannot
support an OOM verdict. This reproduces the production incident recorded on
#464: the kernel proves OOM while Kubernetes presents exit 128 / `Error`, and
the provider does not name memory as the cause.

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
- the runtime image's user or resource sampler;
- the namespace `LimitRange` or `ResourceQuota`;
- provider status or error response vocabulary.

Do not turn a new provider word into a capability, capacity refusal, or loss
reason until its request and lifecycle have been driven and recorded again.

## 7. Snapshot and file transport

A second production observation on 2026-08-26 established the provider's
storage transport surface. The deployed OpenAPI document advertised:

- `POST /v1/sandboxes/{sandbox_id}/snapshots`;
- `GET /v1/snapshots` and `GET`/`DELETE /v1/snapshots/{snapshot_id}`; and
- `CreateSandboxRequest.snapshotId` for restore.

The routes existed, but a native snapshot finished in `Failed` state with
reason `RegistryNotConfigured`. The snapshot registry was not configured in
the controller manager, so native snapshot and restore were not operational.

The provider-authenticated Execd proxy accepted command, ranged file download,
and multipart file upload requests. An idle-fixture round trip preserved text
and binary hashes, executable mode, and a relative symlink. The download route
returned a correct HTTP 206 response for a byte range. The upload route
returned no `ETag`, `Digest`, `Content-MD5`, or other integrity header, so the
destination had to verify the complete SHA-256 value before extraction.

This observation proves byte transport only. It does not prove resumable
upload, a maximum request size, or a consistent capture while processes are
writing. The provider proxy used a 180-second HTTP client timeout and exposed
no filesystem pause, freeze, or atomic-volume operation. Any future use must
define its own size limit, deadline, integrity check, and writer-consistency
boundary.
