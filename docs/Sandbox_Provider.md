# Sandbox provider: observed production contract

## Current Runtime Contract

### Machine Included prototype

The lifecycle request accepts `machine_included: true` for a new Sandbox. CMA
resolves `CHRONO_MACHINE_SANDBOX_IMAGE`, records the immutable image and the
`CMA_MACHINE_INCLUDED=1` marker in the Sandbox profile, and keeps the existing
2,000 millicore / 8,192 MiB reservation. The option remains disabled when that
image setting is empty. Only the image built with `RUNTIME_DESKTOP=1` installs
Chromium, Xvfb, Openbox, x11vnc, websockify, scrot, wmctrl and xdotool. Its rootless desktop
starts on display `:99` before runtime readiness. Ports 5900 and 6080 bind to
loopback; `/runtime/desktop` uses the supervisor's control authentication.
The local WebSocket listener also requires a fresh boot token supplied by the
supervisor in its Host header, so a page in Chromium cannot bypass CMA through
localhost. The credential is absent from request paths and child browser env.
Closing Chromium keeps the desktop alive; its right-click menu reopens the same
browser profile at `~/.cma/chrome`.

The public owner-authenticated `GET /api/v1/agents/{id}/desktop` returns the
WebSocket URL. `/desktop/ws` opens a bounded runtime connection before upgrading,
refuses a second browser connection to the same Sandbox, checks browser Origin,
and rechecks owner lineage and runtime identity every 30 seconds. Connections
expire after 30 minutes and can reconnect. CMA bundles the pinned noVNC 1.7.0
client with the web application. The viewer offers full screen, explicit user
input and reconnect. It pauses user input while the current Agent is working;
this UI behavior is not an input lock against commands from other Agents in the
same Sandbox. Isolated Agents cannot access the shared desktop.

Codex runs beside Chromium and uses its existing shell and image tools. A
computer-use feature flag does not prove a Linux screenshot/click/type driver.
A clean CLI 0.165.0 tool-inventory check exposed shell and image tools but no
computer-use tool. Official OpenAI documentation describes the desktop Computer
Use plugin on macOS and Windows, not a bundled Linux CLI driver:
https://learn.chatgpt.com/docs/computer-use . The desktop variant pins CLI 0.165.0
(the current stable registry version checked on 2026-10-29), while the ordinary
headless runtime keeps CLI 0.147.0. Both use the source-pinned ACP 1.4.0 bridge.
The desktop image enables `features.computer_use` in its system configuration
and uses the verified global CLI instead of ACP's older nested dependency.
The feature flag is checked separately from actual desktop execution. An Agent can take a screenshot with
`scrot /tmp/desktop.png`, inspect it with `view_image`, and click/type through
`xdotool` on `DISPLAY=:99`; no additional computer-use MCP server is involved.

CI builds a separate desktop image and executes its shared display contract
using a loopback model fixture: real Codex shell click/type, `view_image` delivery,
authenticated framebuffer transfer, and subsequent user input in the same
browser. This is tool execution evidence, not a live model task or production
provider acceptance. The image must pass that check and a disposable production
Sandbox demonstration before enabling its immutable digest. Audio and video
playback are outside this prototype. The desktop launcher owns a separate
session and is a Linux child subreaper, so detached browser helpers stay in its
process tree. Recovery admits only the source-pinned launcher directly under
the runtime and its descendants, rechecks the launcher identity, and continues
to reject unrelated processes. Unit checks cover replacement/refusal cases;
the Linux image must also exercise this against real Chromium processes.
Acceptance closes the browser without losing the display, restarts the runtime
with its retained home, and checks Chromium localStorage through the browser.

Actions run `40737301164` established the earlier startup failure: Docker's
default seccomp policy denied Chromium's namespace sandbox. A disposable probe
in the production `kata-clh` runtime successfully created nested user and network
namespaces with the production non-root policy; that probe was removed. CI uses
a hash-pinned Moby default profile with only Chromium's user/PID/network namespace
creation and chroot added. Chromium's sandbox stays enabled; the acceptance
requires a renderer with PID namespace and seccomp isolation. The latest Linux
image qualification and shared-display acceptance are still pending. The
deployment image setting remains empty until those checks pass.

The Sandbox/Agent implementation requires runtime contract **3**, declared by
the image label `dev.chrono.cma.runtime-contract`. The control plane provisions
one isolated runtime per Sandbox, with Workspaces, dedicated Worktrees and
per-Agent homes. It does not create one pod per Agent or run Agents in a main
checkout. Software installation completes as `cma` before credential delivery
or activation of the credential-bearing loopback proxy.

The final image must demonstrate operation receipts, credential generations,
per-Agent configuration and exact conversation resumption. Retained OpenSandbox
runtimes must still support releasing writer holds left by earlier deployments.
CMA no longer captures or restores whole-home archives. Changing
`CHRONO_SANDBOX_IMAGE` requires an immutable digest and selects the image for new
allocations; existing Sandboxes retain their pinned image across pause, resume
and control-plane upgrades. A new tag or label is not runtime evidence.
See [CMA Model](CMA_Model.md) for the current contract and
[Operations](CMA_Operations.md) for the final verification register.

The additive conversation acknowledgement protocol and its rollout limits are
documented in [Runtime output acknowledgement](CMA_Runtime_Output_Recovery.md).

For the `agent-sandbox` provider, the attached home persistent volume is the
persistence authority. Its runtime transport has no CMA checkpoint archive
store. Checkpoint capture, writer holds, writer-resume commands and archive
restore/bootstrap are refused as unsupported before Kubernetes discovery or
runtime HTTP requests. Runtime ownership bindings, subscriptions, normal
control operations and Agent history export remain independent of archive
storage. Production also constructs the retained OpenSandbox transport without
archive authority. Its only remaining checkpoint command is release of an existing
writer hold, which requires no archive access. Startup no longer reads checkpoint
bucket or storage machine credentials, and the storage proxy HTTP implementation
has been removed. Historical manifests and their object-key validation remain
readable; blob upload, download and deletion return Unsupported. CMA does not
claim historical objects were deleted, and archive-only paused Sandboxes need an
explicit migration outside CMA before revival.

Native pause patches the admitted `agents.x-k8s.io/v1beta1` Sandbox to
`operatingMode: Suspended` and waits for all Pods owned by that CRD UID to
disappear, including terminating Pods. CMA persists the retained runtime binding
separately from a live runtime and preserves it during reconciliation and
retirement. Resume patches that same CRD name and UID to `Running` with a newly
reserved runtime generation. The pinned image, original PVC UID and configuration
remain exact; the replacement process must report a different boot identity.
Changing the default image does not change a suspended Sandbox on resume.

The native adapter accepts a retry after a lost pause or resume response only
when the provider still has the exact old suspended or exact next running spec.
It rejects replacement UIDs, image changes, incomplete Pod inventories and stale
resource versions. Retirement deletes the retained CRD and proves Pod absence
before releasing the home. It can also finish when deletion preempts a pause or
resume whose response was lost, without contacting the stopped runtime process.

Local HTTP transport tests cover native operation without archive storage,
cross-Sandbox binding refusal, rejection of archive operations before I/O and
legacy writer-release recovery without an archive store.
They do not establish production PV pause/revive/delete acceptance or native
warm-pool readiness.

Images supporting native pool initialization additionally declare
`dev.chrono.cma.runtime-bootstrap=1`. Their `--wait-for-assignment` mode requires
the Pod UID from the downward API and starts only a mode-0600 Unix socket outside
the tenant home; it has no tenant identity, control token or TCP listener. The
control plane delivers a bounded configuration through Kubernetes exec stdin to
the fixed `/usr/local/bin/layer4-runtime --assign-runtime` command. The waiting
process checks the exact Pod UID and admits only the first valid configuration.
An identical request can replay after acknowledgement loss; a different request
cannot replace it. The control token remains in memory and is excluded from
child environments. Normal authenticated runtime startup follows assignment.

The native exec sender checks CRD/Pod identity and persisted Sandbox, generation
and home authority before and after delivery. It requires a successful exec
exit status and an exact request-digest receipt, bounds both output streams,
and does not propagate remote output in errors. Local tests exercise kube-rs
WebSocket exec and real runtime processes, including Pod replacement refusal,
bounded output without EOF, authenticated startup and retained-home restart.
The native pool client configures immutable `SandboxTemplate` and
`SandboxWarmPool` resources keyed by image digest, CPU/memory, initialized home
policy and Pod policy. It pins seed/template/pool UIDs and counts ready replicas
only after the controller observes the pool generation. Each template provisions
an independent PVC; claims carry only `warmPoolRef`, because claim-level env or
volume templates force upstream cold allocation. Persist a claim request before
calling `claim`, and its returned UID before `observe_claim` or `adopt_home`.

Home adoption records the original claim, Sandbox and PVC UIDs on the claim,
converts the same Sandbox from a volume template to an explicit reference to
the same PVC, and waits for the controller to observe that generation. Only then
does it remove the PVC owner reference and record retained-home metadata. This
prevents the controller from re-adopting a detached PVC. No Pod operation is
issued during conversion. Lost mutation replies replay against the same UIDs.
The attachment must be persisted before runtime assignment. `recover_home`
recovers an adopted PVC against the original claim receipt even after claim or
seed removal; storage cleanup without an attachment also checks a bounded,
complete inventory, rejecting duplicate or inconsistent adoption records.

Local Kubernetes HTTP fixtures cover pool/claim operations, controller-observed
home adoption, original Pod policy retention and recovery of lost replies.
Pod discovery and pause/delete observation use the upstream controller's
`agents.x-k8s.io/sandbox-name-hash` label, then validate the original Sandbox
owner UID. This supports claimed warm Pods without claim-specific CMA labels
that upstream metadata reconciliation would remove. The hash is only a search
hint; matching labels alone never grant runtime ownership. CRD deletion
observation uses name and UID without filtering on a mutable management label.
The production native backend recognizes an adopted home and admits its original
claim-owned Sandbox with a metadata-only runtime binding. It validates the
original image/resources/mounts and the complete request fingerprint, preserves
the waiting Pod template and sends assignment before binding transport. Retries
and resume keep the original CRD/PVC and Pod policy; resumed Pods receive the
reserved newer generation through exec. Claim ownership is checked again around
credential delivery. Runtime deletion first deletes the original claim, then
waits for the CRD, owned Pods and claim to disappear; lost deletion replies can
recover claim identity from the retained PVC.

The lifecycle worker selects its provider before home allocation and records
the selected pool request, original claim UID, assigned Sandbox and home receipt
in separate durable transitions. Retries preserve the selected image and Pod
policy. A reservation already committed to ordinary storage stays on that path.
Native reservations cannot allocate an ordinary replacement PVC on retry.

Lifecycle publication retries confirmed aborted Mongo transactions with bounded
backoff. If contention still exhausts that budget, the worker rereads the exact
lease, phase, runtime and credential before scheduling a retry and releasing its
own unchanged lease. It never releases a successor's lease or repeats a commit
whose result is unknown; unknown results use the existing reconciliation path.
This prevents transient queue-completion contention from waiting for the long
lifecycle lease to expire. Persistent database unavailability can still defer
recovery. Disposable replica-set tests cover repeated write conflicts, ambiguous
commits, preserved runtime identity and a single completed lifecycle event.

Retirement also handles claims that never reached runtime admission. It records
discovered claim/member identities before requesting claim-first deletion, then
observes claim, owned Sandbox and Pod absence before disk release. Cleanup does
not recreate missing claims. A late creation after database retirement is found
by native claim reconciliation; orphan cleanup records its plan on the original
claim. Retained-home cleanup checks native finality even if the member CRD has
already disappeared. Incomplete inventories or unknown orphan Pod parents keep
cleanup pending.

Operators manage native capacity through `/api/v1/admin/warm-pools` and the Warm
pools tab. Targets persist the configured system image, resources and original
Pod policy before provider I/O. Initial pool creation uses zero replicas until
the pool/template UIDs are acknowledged in Mongo; later scaling always uses
that original binding. Changes to deployment defaults never retarget an existing
pool. The former profile prewarming worker now only retires unowned legacy
reservations; new creation no longer claims legacy prepared environments.

Kube-rs HTTP/WebSocket fixtures exercise this backend with failed exec, exact
replay, a restarted control-plane client and a new Pod on resume. Worker tests
cover durable stages, response loss, partial retirement and late claims. These
fixtures emulate controller observations and assignment reception. Production
controller compatibility and real lifecycle acceptance remain deployment gates;
see [deployment handoff](CMA_Deployment_Readiness.md).

The historical provider observations in sections 1-7 below remain unchanged.
They establish only behavior on their recorded versions. Section 8 records
Agent-home experiments on a contract-2 image with Codex 0.147.0 and codex-acp
1.4.0. Its G-A3 result was corrected by #835: the bridge **does** discover skills
added after startup. Do not cite those historical experiments as proof that the
final contract-3 image or production rollout passed. Re-run G-A1-G-A5 and the
Sandbox/restart/rotation gates against that final digest before public readiness.

## Historical Provider Evidence

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

## 8. Agent home evidence

### Untouched-conversation persistence correction

On 2026-09-08 a production default Agent with no submitted prompt exposed an
incomplete case in the original capture evidence: Codex 0.147.0 keeps a newly
started conversation in memory until its first message or a metadata write.
Whole-home capture succeeded, but the separate conversation capture correctly
refused the absent rollout and manual pause failed before publication.

The provider-specific compatibility patch in
`api/crates/layer3-adapter/src/codex/persist_conversations.cjs` names each new
conversation `New Agent` through `thread/name/set` before ACP reports success.
This lets Codex itself persist its original metadata and conversation identity;
it neither sends a prompt nor manufactures transcript entries. The upstream
ACP 1.4.0 distribution hash remains
`ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf`.
The patched executable hash is
`c3cb7e54fb6a48f3091b97bed9f5243eaf7c3f3293fbd1b021e0faf41067cd2c`.
Both are verified in the runtime build. Unexpected upstream bytes, repeated
patching and ambiguous source matches fail closed.

An offline Linux/amd64 test against the exact production runtime archive
verified a single `session_meta` record after `session/new`, zero turns, a full
provider-process restart, and `session/load` of the original identity. The
runtime build repeats this proof. This local proof does not by itself claim
production pause/resume acceptance; that is recorded separately in
`CMA_Unattended_Acceptance.md` after deployment.

The official app-server reference documents `thread/name/set` and the persisted
thread lifecycle: <https://learn.chatgpt.com/docs/app-server.md>. The pinned
binary and ACP process tests establish the exact empty-conversation behavior.

### Original agent home evidence

This record was captured on 2026-09-05 Asia/Singapore against the contract-2
runtime. It establishes the per-agent Codex home capability; it does not rely
on the older production-provider observations above.

| Component | Pinned value |
|---|---|
| Runtime image | `cma-runtime:m16-arm64` |
| Runtime image ID | `sha256:0fc5383fa451ca16f024951d29813f6fa06a7ddcb368ebc4dd54bbae48dd5152` |
| Architecture | `arm64` |
| Runtime contract label | `2` |
| Codex | `@openai/codex` 0.147.0 |
| ACP bridge | `@agentclientprotocol/codex-acp` 1.4.0 |
| Codex launcher SHA-256 | `134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477` |
| ACP distribution SHA-256 | `ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf` |

The runtime's revision label was
`08b0ddae7b69cfe086da3e1be6eea72b9b2a77d5`, which did not resolve to a
commit in the local repository used for this audit. This record therefore
pins the immutable image ID and executable hashes rather than claiming source
provenance from that label. The final release evidence must pin the rebuilt
release image digest and final CMA commit independently.

The probe spoke newline-delimited ACP JSON-RPC over bridge stdio, correlated
request IDs, enforced a deadline, and redacted session IDs and credential
values. The redacted source report and harness have these hashes:

```text
report SHA-256:  e1a75eb6fe0fad6c71f936d84c40d3a8e04ae6333ba3f166259f61094db217a5
harness SHA-256: 09cb4a6aef1fb8e8faac893a57cc6a3ba61d863caa9b6ce8e4fcf2e23f36fc7f
```

No API key, authorization-header value, or unredacted conversation ID is in
this record.

### Gate verdicts

| Gate | Verdict | Demonstrated contract |
|---|---|---|
| G-A1 | Pass | Two concurrent bridges in one sandbox use different `CODEX_HOME` configuration. |
| G-A2 | Pass | Home `AGENTS.md` and repository `AGENTS.md` are both applied, in that order. |
| G-A3 | Corrected by #835 | Initial skills are discovered, and added skills are hot-reloaded in the same bridge. |
| G-A4 | Pass | The shared auth symlink and the managed loopback-provider path both work. |
| G-A5 | Pass | Conversation data lives in the agent home and survives exact capture and restore. |

The adapter advertises `per_agent_home` for runtime contract 2 on the strength
of these results. A future Codex, ACP, or runtime-contract version does not
inherit this evidence automatically.

### G-A1: distinct homes

Two bridges ran concurrently in one container. Alpha used model `gpt-5.2`
with low effort; beta used `gpt-5.5` with high effort. Each process received a
different home beneath `/home/cma/.cma/agents`.

```text
CODEX_HOME=/home/cma/.cma/agents/alpha/home
CODEX_HOME=/home/cma/.cma/agents/beta/home
```

The redacted `session/new` results were:

```json
{
  "bridgesConcurrentInOneContainer": true,
  "agentA": {
    "sessionId": "<session sha256:90e2e0bfae28 length:36>",
    "model": "gpt-5.2[low]",
    "configOptions": {"model":"gpt-5.2","reasoning_effort":"low"}
  },
  "agentB": {
    "sessionId": "<session sha256:a77464c25107 length:36>",
    "model": "gpt-5.5[high]",
    "configOptions": {"model":"gpt-5.5","reasoning_effort":"high"}
  }
}
```

This proves that `CODEX_HOME` selects process-local configuration for
simultaneous agents in one sandbox.

### G-A2: profile and repository instructions

The selected home and git worktree carried independent sentinels:

```text
<CODEX_HOME>/AGENTS.md: CMA808_GLOBAL_PROFILE_SENTINEL
<worktree>/AGENTS.md:  CMA808_REPOSITORY_INSTRUCTION_SENTINEL
```

`codex debug prompt-input` rendered this model-visible order:

```text
# AGENTS.md instructions for /workspace

<INSTRUCTIONS>
CMA808_GLOBAL_PROFILE_SENTINEL

The global agent profile instruction must be presented before repository instructions.

--- project-doc ---

CMA808_REPOSITORY_INSTRUCTION_SENTINEL

The repository instruction must be presented after the global profile instruction.
</INSTRUCTIONS>
```

The contract is therefore "profile prompt + repository instructions." CMA
writes the profile prompt to the agent home and does not edit repository
files. The fallback worktree `.cma/AGENTS.md` was neither required nor used.

### G-A3: skill discovery and hot reload

`cma-808-start-skill` existed under `<CODEX_HOME>/skills` before bridge start.
Only after the first `session/new` completed, the harness created
`cma-808-late-skill/SKILL.md`, invoked `/skills` in that session, and then
opened another session on the same still-running bridge.

```json
{
  "bridgePidStayedConstant": true,
  "firstSession": {
    "id": "<session sha256:b2596e7a4d99 length:36>",
    "startSkillSeenInInboundFrames": true,
    "lateSkillSeenInInboundFramesAtCreation": false
  },
  "sameSessionSlashSkills": {
    "stopReason": "end_turn",
    "error": null,
    "startSkillSeenInInboundFrames": true,
    "lateSkillSeenInInboundFrames": true,
    "inboundMethods": ["session/update"]
  },
  "secondSessionSameBridge": {
    "id": "<session sha256:4ee65e2f4793 length:36>",
    "lateSkillSeenInInboundFrames": true
  }
}
```

This contradicts #808's original negative assertion that a post-start skill
would remain invisible. [Issue #835](https://github.com/ChronoAIProject/cma/issues/835)
corrects that contract: the pinned bridge discovers the added skill through
same-session `/skills`, and a later `session/new` on the same bridge discovers
it as well. No cache was added to manufacture the old result.

CMA still installs every configured skill beneath the individual agent home
before first use, and an unavailable archive fails creation with typed
`skill_unavailable { id }`. Agent Profile immutability remains a CMA product
and audit rule; it is not justified by a Codex kernel cache boundary.

### G-A4: shared auth and managed loopback

For own-credential access, the literal layout was:

```text
/home/cma/.cma/provider/codex/auth.json
/home/cma/.cma/agents/own/home/auth.json -> ../../../provider/codex/auth.json
```

The provider auth file was mounted read-only from the host into a disposable
container. The status transcript was fully redacted:

```text
../../../provider/codex/auth.json
Logged in using an API key - <fully redacted>
exit 0
```

ACP initialization advertised `api-key`, and `session/new` succeeded with
model `gpt-5.2[low]`.

After the new renderer landed, the managed path was repeated against its exact
configuration rather than relying on the earlier port-18080 probe:

```toml
model_provider = "cma"

[model_providers.cma]
name = "CMA Loopback"
base_url = "http://127.0.0.1:9091"
env_key = "CHRONO_UPSTREAM_PLACEHOLDER_KEY"
wire_api = "responses"
```

The fake loopback server recorded only structural evidence and deliberately
returned an error after receipt:

```json
{
  "method": "POST",
  "path": "/responses",
  "authorizationHeaderPresent": true,
  "contentType": "application/json",
  "bodyBytes": 44391,
  "configuredModel": "gpt-5.2",
  "markerReachedProxy": true
}
```

No header value was logged. This proves both own-credential and
ChronoAI-managed access modes.

### G-A5: home-local conversation capture and restore

A real ACP turn containing marker `CMA808_HISTORY_SENTINEL_20260905` completed
under `/home/cma/.cma/agents/own/home`. The exact-ID inspector found one
rollout below that home's `sessions/` tree:

```json
{
  "sessionId": "<session sha256:c31611975065 length:36>",
  "relativePath": "2026/09/04/rollout-2026-09-04T18-19-45-<session-id>.jsonl",
  "bytes": 46088,
  "lines": 9,
  "sha256": "8ec8485c97268a3f1a8e2e708c2d84ef581a8991e521fed8606d726b6cb6ebcf",
  "openingRecordType": "session_meta",
  "openingCwd": "/workspace"
}
```

Layer 4 captured that exact conversation as a 46,204-byte carry stream with
SHA-256
`af2707696166648670ee56c26d80560599afaf6ba947e97b62dcd6ad24a75ad6`.
Restoring it into a fresh agent home reproduced the same rollout path, byte
count, line count, and JSONL hash. ACP `session/load` then returned without an
RPC error and replayed the unique marker:

```json
{
  "requestedSessionId": "<session sha256:c31611975065 length:36>",
  "loadError": null,
  "markerSeenInReplayedInboundFrames": true,
  "inboundFrameCount": 5,
  "inboundMethods": ["session/update"],
  "loadedConfig": {"model":"gpt-5.2[low]"}
}
```

A second exact-ID capture after restore produced another 46,204-byte carry
stream with the same SHA-256; byte comparison exited zero. This proves that
conversation records live under the selected agent home and remain findable
by conversation ID after whole-home transport.
