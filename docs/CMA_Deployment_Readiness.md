# C-001 deployment handoff

The three portals ship from one web image: Agent Manager (`/agent-manager`),
Image Manager (`/image-manager`) and Event Gateway (`/event-gateway`). Their
backends remain separate services with their own collection schemas. This handoff prepares
application deployment against the infra-owned native runtime cluster. It does
not record a successful production rollout or authorize changes to that cluster's
controllers, RBAC, storage or network policy.

The current rollout scope is Agent Manager, Event Gateway and their shared web
image, with the tested CMA runtime as the system default. Image Manager is
deferred: leave its Deployment at zero replicas and do not require its Cloud
Build identity, OAuth client, migrations or builder digest to activate Agent
Manager. Default-image Agent, Trigger and consumer creation does not call Image
Manager. Its catalog may be unavailable while the system default remains
selectable; custom image selection and builds require the later Image Manager
activation described below.

| Authority | Kubernetes context | Namespace |
|---|---|---|
| CMA application, ConfigMap and retained OpenSandbox resources | `gke_example-project-02_asia-northeast1_example-cluster-02` | `chrono-bot` |
| Native Sandbox, pool, claim, runtime Pod and home resources | `gke_example-project-02_asia-southeast1-b_example-cluster-01` | Infra-assigned native namespace, set in `CHRONO_AGENT_SANDBOX_NAMESPACE` |

CMA deployment ID remains `chrono-bot-prod`. Always pass the intended context
explicitly. CMA stays in the northeast cluster and calls the southeast
Kubernetes API directly through a separately configured client. Existing
production OpenSandbox resources retain their recorded provider and home
bindings until normal retirement.

The Kubernetes Deployment, Service and ConfigMap names are
`cma-agent-manager`, `cma-event-gateway` and `cma-image-manager`, with matching
application labels and internal service DNS. The source directories `infra/api`
and `infra/cmaeg`, image repositories `cma-api` and `cma-cmaeg`, and existing
container/binary aliases remain compatible. Agent Manager keeps KSA
`chrono-bot-ksa`; Event Gateway keeps `cmaeg-ksa`. Existing external hostnames,
NyxID service identities and protocol paths remain valid.

Compatibility Services `cma-api` and `cmaeg` select the corresponding canonical
workloads. Existing Nyx ChannelPointers and sealed channel records bind the
exact `http://example-host-01.example-namespace.svc` authority, so `CMAEG_CMA_BASE_URL` retains
that origin while provider dispatch and new internal references use canonical
names. The `cmaeg` alias preserves retained CMA configurations during the staged
rename. Do not rewrite stored pointer identities to match a cosmetic name.

Before the first deployment under canonical names, copy the existing Secret
contents through the approved secret workflow to `cma-agent-manager` and the
`cma-event-gateway-{auth,master-keys,cma-service-token}` Secrets. Preserve the
values and key names; renaming a resource is not key rotation. The native
control Secret remains `cma-agent-sandbox-control`. Capture old Deployment,
Service, route and Secret identities before the cutover. Stop the old control
plane before enabling its replacement so there is only one Agent Manager;
verify routing and retained-runtime recovery before retiring old resources.
Applying a new name alone does not perform this migration.

Create the three canonical Service objects before starting the new web image.
Nginx resolves their static upstream names at startup, including
`cma-image-manager`; its Service must exist even while its Deployment remains
absent or at zero replicas. This creates no Image Manager worker. The API and
Gateway Service files also contain compatibility aliases: prepare the new
canonical Services first, and retarget the aliases only when the replacement
Pods are ready to receive traffic. Do not apply the complete bundled files
early and strand the old workloads behind selectors they do not match.

Attach the canonical health/backend policies before sending traffic to their
Services. Retain the old routes and policies while they use the compatibility
aliases; creating a differently named HTTPRoute does not replace an older
route with the same matches. During the rename, verify both the canonical and
alias endpoint sets, route conditions and stream timeout behavior. Stop and
drain the old writers before the Gateway record transfer and replacement
startup; the new Agent Manager's `Recreate` strategy cannot stop an independently
named old Deployment.

## Observed deployment prerequisites

These observations were recorded on 2026-10-13. They distinguish current access
from the remaining application activation steps.

| Prerequisite | Observed state | Remaining work |
|---|---|---|
| Native Kubernetes authentication | Actual CMA Pod received the `example-sa-01@example-project-02.iam.gserviceaccount.com` metadata identity; Sandbox and claim reads returned HTTP 200 with the remote CA verified | Exercise the application token refresh path and native lifecycle with the final candidate |
| Native target | `chrono-bot/example-namespace-02` currently supplies `https://35.240.157.32` and namespace `example-registry-01` | Use these current values; do not use the earlier `cma-test` namespace or infer configuration from last-applied annotations |
| Native authorization | Fresh CMA Pod metadata token confirms Sandbox get, Claim create, PVC get, and `pods/exec` get/create; PVC create/patch remain denied | VCT integration passes native adapter tests. Verify live allocation with the final image; PVC writes and snapshot reads are not required for new empty native homes |
| Native persistent home | A pinned diagnostic Python image passed controller-owned 64 GiB RWOP allocation/mount on `home-a`, suspend/resume with the same PVC and saved marker, and Claim deletion with Sandbox/PVC GC | This proves the infrastructure substrate. Verify the final CMA runtime initializer, authenticated assignment/bootstrap and lifecycle before activation; a mandatory seed snapshot is a superseded implementation choice |
| Published runtime acceptance | Candidate `2bc6fe1c` passed CI and its imported runtime ran under `microvm`, initialized the controller-owned home and accepted authenticated assignment. It then exited with an installation Sandbox identity mismatch. Inspection of the published image found a build-test installation record at `/tmp/cma-checkpoints/runtime-state/install-v2.json` | Keep native activation disabled until the corrected image passes bootstrap, replay, authenticated status and retained-home restart. Build tests must isolate and clean their temporary state; assignment must honor the provided persistent `TMPDIR` instead of reading the waiting process's ambient directory |
| Runtime network path | `chrono-pod-default` admits TCP 8080 from a single peer combining namespace `sandbox-router` **and** Pod label `app=sandbox-router`; that router is not deployed. Initial northeast connection attempts timed out on TCP 9090 and 8080. A subsequent controller-owned Managed Template policy explicitly allowed the northeast Pod CIDR on TCP 9090, but all 15 allow-phase requests still timed out | Infra must provide and verify the runtime path. Current CMA discovery connects directly to Pod IP TCP 9090 for authenticated HTTP and WebSocket. The measured timeouts do not isolate policy from routing or VPC firewall enforcement; Kubernetes API access alone does not satisfy this requirement. See the Native Template network policy evidence below |
| Runtime ServiceAccount and storage | Remote `default` exists, is active and has no GSA annotation. A `standard-rwo` canary disk could not attach to the selected N4 node because it uses `pd-balanced`; `home-a` (`hyperdisk-balanced`, CMEK) passed the diagnostic 64 GiB RWOP lifecycle | CMA selects `default` with Pod token automount disabled and `home-a` with the existing 64 GiB home default. A new runtime ServiceAccount is not required; final CMA image/bootstrap acceptance remains separate |
| Runtime isolation class | The southeast `example-namespace-01` policy enforces `microvm` or `gvisor`; an isolated Sandbox without `runtimeClassName` was rejected. The diagnostic `microvm`/`home-a` Pod ran successfully | CMA explicitly selects the existing `microvm` RuntimeClass. Class existence and policy compatibility are checked before activation; final CMA image and connection acceptance remain separate |
| CMA runtime signing key | `chrono-bot/cma-agent-sandbox-control` has been created and read-verified in the northeast CMA cluster, without printing or storing its value locally | Mount the existing Secret during activation and preserve its value across restarts; it is separate from infra's Kubernetes access identity |
| Canonical application prerequisites | Three canonical Services, Agent Manager/Event Gateway ConfigMaps, and their four health/backend policies have been created and read-verified in northeast. Original credential values match the canonical Secret copies; new required auth/control keys are present | Deploy the verified application images, observe ready endpoints and finish route/alias cutover. Old `cma-api`, `cmaeg` and `cma-web` remain Ready 1/1; no canonical application worker or Image Manager worker has started |
| Shared MongoDB | Existing `chrono-bot` credentials have been copied to and verified in the canonical application Secrets; original Secrets are retained | Select each service's shared-schema mode and run only its own migration/startup initialization |
| Gateway record transfer | Read-only production audit found all three legacy collections, 11 valid channel records and no connection/interaction records; destination `eg_*` collections are absent. A real local Mongo/mongosh roundtrip proves all three domains, cursor batches, exact bytes/i64 revisions, expiry, replay and conflict refusal | Repeat the live export after draining writers, initialize only Gateway's new schema, import and verify before starting the replacement. The audit did not migrate production data; its nine Sandboxes and sixteen Agents must remain available through the control-plane rollout |
| Existing Event Gateway feature settings | Live `cmaeg` has Console and Media enabled, `cma_codex` non-human/media providers and `gpt-transcribe` STT settings, with optional STT/console Secret refs and a 32 MiB in-memory media volume | The controlled Gateway integration restores live source `976e8773` Console/Media/STT code while retaining independent Mongo and portal authentication. Gateway/Core 1,556 tests, strict Clippy and the authenticated Mongo Console/media/portal composition contract pass locally. Keep the old deployment and Secrets until the final integrated source passes native CI and image gates, all three record domains are migrated, and the replacement passes real Console/media/consumer acceptance; empty STT URL and FFmpeg directory preserve the live disabled transcription/transform settings |
| Image Manager build identity (deferred) | Lookups for the single `cma-image-manager` Google service account returned 404; current operator IAM tests did not grant account creation, policy changes or Cloud Build submission | Does not block the current Agent Manager/Event Gateway rollout. Infra provisions this identity and the permissions below before later Image Manager activation |
| Application image registry | Candidate `2bc6fe1c` API, Gateway and Web images were imported into `us-central1-docker.pkg.dev/example-project-01/example-registry-02` and their immutable bytes reverified | These images have not been deployed. Publish the corrected candidate after its required CI and image gates, then regenerate the deployment bundle from that candidate's receipts |
| Native runtime image registry | Candidate `2bc6fe1c` runtime was imported into `asia-southeast1-docker.pkg.dev/example-project-02/example-registry-01/cma-runtime`; the remote node pulled and ran its exact digest | Publishing and image-pull access passed. The separate startup failure above prevents native acceptance; publish and test the corrected image before activation |

## C-001 implementation map

The Journey Book is the audit baseline; the current implementation follows the
requested C-001 changes below. Native lifecycle, Cloud Build and OAuth behavior
still require the deployment acceptance in this document.

| User flows | Prepared application behavior |
|---|---|
| UF-01, UF-13, UF-19, UF-20 | Agent Manager landing with three service descriptions and shared service navigation; optional repository with new, existing or contextual Sandbox creation; deployment runtime/model/scope defaults; settings in Agent details; durable three-stage provisioning and a NyxID connection modal |
| UF-03, UF-10, UF-11 | User Profile creation and marketplace routes retired; private immutable configuration records remain internal persistence details |
| UF-04 | Add an empty or GitHub workspace through the inline workspace API; contextual Agent creation retains that workspace and inherits its Sandbox image/key scope |
| UF-05 | Agent menu downloads authenticated JSONL history from its recorded runtime provider |
| UF-08, UF-09 | Native suspend/resume retains the same CRD and PVC; deletion waits for claim, CRD and Pod finality before disk/record cleanup; production CMA archive storage and scheduling retired |
| UF-12 | Operations configures native pool capacity; durable worker stages select/claim/adopt the original member and recover after restarts; legacy Profile warmup only retires old reservations |
| UF-14 | Sharing addresses the Sandbox and its Workspace/Agent descendants |
| UF-17, UF-21 | Trigger and Event Gateway consumer setup use image plus inline Agent settings; consumer management is available from the account menu |
| UF-27 | Default owner Sandbox quota is ten |
| UF-31 | Image Manager accepts bounded UTF-8 Dockerfile imports and reconciles the complete Cloud Build job |
| All three portals | Shared dark default, navigation, input/button styling and responsive layouts |

Agent, Trigger and consumer provisioning uses the deployment's configured CPU
and memory defaults; pool forms initialize from those defaults. New Agent
lifecycle operations preserve accepted capacity for
restart/retry, alongside the selected immutable image. Pool administration is
limited to the trusted deployment runtime image; custom images remain available
for ordinary Agent/Trigger/consumer creation.

New inline Sandbox configurations allow up to 16 workspaces and 16 Agents per
workspace, within the existing admission bounds. Previously accepted lifecycle
operations keep their original immutable limits and resource reservation when
replayed. Existing-workspace Agent creation uses the durable lifecycle endpoint
with `workspace_id`; it cannot replace the parent image/repository or expand its
connected-service scope.

## Infra-owned native API prerequisites

CMA's initial native integration was implemented against
[`kubernetes-sigs/agent-sandbox` at `86ad94178a85aa26788077a55894cdc234c5ea6a`](https://github.com/kubernetes-sigs/agent-sandbox/tree/86ad94178a85aa26788077a55894cdc234c5ea6a).
The southeast installation was subsequently inspected at release `v1.0.2`,
source revision
[`9a85153590e54cb980f3241f9e7a9228449412c9`](https://github.com/kubernetes-sigs/agent-sandbox/tree/9a85153590e54cb980f3241f9e7a9228449412c9),
controller digest
`sha256:dc700a37ecf4e680a146098a30996f420507e76e83a27dbd293f2b8114b14daa`.
Its source and all four served CRD schemas are compatible with the required
native contract. Keep both development baseline and actual installed provenance
in rollout evidence; a passing schema check is not live lifecycle acceptance.

Infra owns the matching core and extension controllers and CRDs. The absence
of a CMA ConfigMap or northeast ServiceAccount in the southeast cluster does
not mean its native controller is missing or incompatible. CMA application
configuration is read from northeast; remote namespace, identity and storage
prerequisites are checked independently in southeast.

| Namespaced resource | Served version | Behavior CMA requires |
|---|---|---|
| `sandboxes.agents.x-k8s.io` | `v1beta1` | `podTemplate`, `volumeClaimTemplates`, `operatingMode: Running/Suspended`, Ready condition with observed generation, owner-linked Pods |
| `sandboxtemplates.extensions.agents.x-k8s.io` | `v1beta1` | Per-member volume templates, `volumeClaimTemplatesPolicy: Disallowed`, exact Pod template |
| `sandboxwarmpools.extensions.agents.x-k8s.io` | `v1beta1` | `sandboxTemplateRef`, `replicas`, `updateStrategy: OnReplenish`, `status.observedGeneration/readyReplicas` |
| `sandboxclaims.extensions.agents.x-k8s.io` | `v1beta1` | `warmPoolRef`, assigned `status.sandbox.name`, transfer of member ownership to the claim, foreground cascading deletion |

Claims contain only a warm-pool reference. Claim-level environment variables or
volume templates force a cold allocation in the pinned upstream version. CMA
delivers the owner configuration over Kubernetes exec after durably recording
claim and disk identity. Template labels include
`cma/control-plane: chrono-bot-prod`; the controller preserves these labels on
claimed Pods. No change to the default additional-Pod-label allowlist is needed.

The installed controller creates and owns PVCs from `volumeClaimTemplates`.
Its `reconcilePVCs` creates `<template-name>-<sandbox-name>` and sets the Sandbox
controller reference. `operatingMode: Suspended` removes the owned Pod while
retaining the Sandbox and its PVCs; `Running` can mount those same volumes again.
CMA does not inherently need PVC/PV write access for this lifecycle. New homes
use the runtime's credential-free empty-volume initializer; a seed VolumeSnapshot
is only a retained legacy mode, not an agent-sandbox API requirement.
Claim `lifecycle.shutdownPolicy: Retain` retains the **claim**
after expiry and still deletes its Sandbox; it must not be used as a promise of
home retention. Suspension without expiring/deleting the claim is the applicable
pause operation.

The current CMA implementation discovers Pods with the upstream
`agents.x-k8s.io/sandbox-name-hash` label and verifies their owner UID. New native
homes preserve the volume template and the PVC's Sandbox owner reference. Their
receipt pins the original Claim, Sandbox and PVC UIDs; controller GC owns disk
deletion. Conversion to detached PVC mounts exists only for already-admitted
legacy homes and must not expand the new flow's permission requirements.

The other current non-CRD dependency is `pods/exec` for authenticated assignment
of an unclaimed runtime. Ordinary runtime operations and conversations already
use the runtime's HTTP/WebSocket interface. A CRD-only client would need a tested
replacement for bootstrap authentication, Pod-incarnation verification and
deletion finality. The served Sandbox status includes Pod IPs, readiness and
suspension conditions, but no Pod/PVC UID. Access to the remote Kubernetes API
alone does not prove that the CMA cluster can reach runtime Pod IPs or that a
remote Service FQDN resolves locally. Keep these as explicit acceptance checks;
do not remove working identity checks or assume an IP establishes identity.

The installed API consists of the core `Sandbox` plus the three extension
resources `SandboxTemplate`, `SandboxWarmPool` and `SandboxClaim`. The three
extension resources alone do not expose suspension: CMA must patch the bound
Sandbox's `spec.operatingMode` and observe its conditions. This remains within
the agent-sandbox API and does not require direct Pod mutation.

For the currently requested product flows, there is no unavoidable need for
CMA to write Pod, PVC, PV, Secret or VolumeSnapshot resources directly. Templates
declare the runtime image, init container, mounts and volume claims; controllers
own allocation and cleanup. Conversations, model/settings changes, history
export and scoped-key delivery are CMA runtime HTTP/WebSocket operations, not
Kubernetes exec operations. A future requirement to resize an existing disk,
manage independent snapshots or retain a disk after deleting its Sandbox would
need capabilities beyond this lifecycle, but those are outside the current
requested CMA flows.

The current exec bootstrap avoids putting credentials into CRD specs: it sends
the bounded assignment over authenticated Kubernetes stdin before the runtime
starts its ordinary control listener. Replacing it with HTTP requires an
authenticated first-assignment protocol with replay and runtime-incarnation
checks; the later control token cannot authenticate the request that first
installs that same token. A shared warm-pool secret or an unauthenticated Pod IP
is not an equivalent replacement. Pod/PVC reads currently verify exact UID
ownership and cleanup independently of controller status; treat removal of
these reads as a separate, tested contract change rather than a storage blocker.

## CMA identities, storage and network

The Agent Manager ServiceAccount remains `chrono-bot/chrono-bot-ksa` in the
northeast application cluster. `infra/api/serviceaccount.yaml` declares its
local Role/RoleBinding and the same-cluster native policy. Those local grants
do not confer authority in the remote cluster. Infra must provide a separate
remote identity and namespaced authorization in the assigned native namespace.
Validate the actual remote principal's grants; do not apply the northeast
application manifests to southeast as a substitute. The table below describes
the **current preflight grant set**, not the minimum permissions intrinsic to
agent-sandbox. PVC/snapshot and exec rows are under CMA-side simplification;
they are not an outstanding request for infra to extend its Role:

| API/resource | Checked verbs | Purpose |
|---|---|---|
| `agents.x-k8s.io/sandboxes` | get, list, create, patch, delete | Provision, observe, suspend, resume and retire |
| `extensions.agents.x-k8s.io/{sandboxtemplates,sandboxwarmpools,sandboxclaims}` | get, list, create, patch, delete | Native pool administration and claims |
| core `persistentvolumeclaims` | get, list for new native homes; create, patch, delete only for legacy detached-home paths | Exact owned-volume receipt and cleanup verification; controllers own new VCT volumes |
| core `pods` | get, list | Verify exact Pod identity and deletion finality |
| core `pods/exec` | get, create | kube-rs WebSocket assignment over stdin |
| `snapshot.storage.k8s.io/volumesnapshots` | get only for legacy snapshot-seeded home mode | Verify a historically pinned initialized seed; not used for new empty native homes |

For new empty native homes, the adapter currently emits PVC get/list, Pod list,
and a WebSocket GET to `pods/exec`. Pod get and exec create are conservative
preflight grants beyond those emitted requests; the pinned kube-rs client uses
GET for assignment. The operator preflight also reads the configured
ServiceAccount, selected RuntimeClass and StorageClass, but CMA's runtime
adapter does not need those read permissions. No direct PV, Secret or ConfigMap
calls are part of this path.

The checked-in local Role retains permissions needed by previous OpenSandbox and
storage cleanup paths. Native controllers own runtime Pod creation/deletion;
runtime workloads use `CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT` (default
`cma-runtime` in code; explicitly `default` in the production configuration) in
the remote namespace. The production choice uses the existing unannotated
runtime account; it does not assign the control plane's `chrono-cma` identity
to user workloads. CMA sets
`spec.automountServiceAccountToken: false` in every runtime Pod template; this
overrides the ServiceAccount default, so an unset or true ServiceAccount field
does not fail preflight. Verify the admitted Pod retains that explicit false
setting during activation. Registry pull identity needs reader access to each
selected immutable image; this is
separate from the Image Manager workload/build identity. Configure
`CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS` only if the node's registry identity
does not supply that access. Neither registry nor Kubernetes credentials belong
in an Agent's environment.

For same-cluster deployments, `infra/api/serviceaccount.yaml` restricts ingress
to every native CMA runtime Pod, including unclaimed warm Pods, by the stable
control-plane and upstream Sandbox tracking labels. The tracking selector
excludes retained OpenSandbox Pods. Only Agent Manager Pods in the namespace
may connect to TCP 9090. That namespace-local selector does not authorize a
control-plane Pod in another cluster. Infra must configure and verify the
remote runtime network path for the northeast CMA source, covering both cold
and warm Pods. Reaching the Kubernetes API alone does not prove runtime
connectivity. Verify the cluster CNI enforces NetworkPolicy and that an
unrelated Pod cannot reach the runtime port.
On 2026-10-13, connection attempts from northeast CMA to the ready diagnostic
Pod at `10.57.0.97` timed out after four seconds on both TCP 9090 and TCP 8080.
That diagnostic used a pinned Python image and an Unmanaged Template. The
subsequent Managed Template diagnostic described below also timed out after
the controller reconciled an explicit TCP 9090 allow rule. Neither diagnostic
used the final CMA runtime. The remote network blocker remains unresolved;
these measurements do not distinguish NetworkPolicy rejection from routing or
other network enforcement.
The policy intentionally does not rely on `cma/runtime-name`, which claimed
warm Pods do not carry. Workload network egress and runtime scheduling remain
infra policy; check the selected RuntimeClass can schedule the requested image,
CPU/memory and disk in the target node pool.

The production application configuration explicitly selects
`CHRONO_AGENT_SANDBOX_RUNTIME_CLASS=microvm`. On 2026-10-13, the southeast
`example-namespace-01` Kyverno policy was confirmed in `Enforce` mode: Pods in
`example-registry-01` must name either `microvm` or `gvisor`. An isolated Sandbox probe
without `runtimeClassName` was rejected, so an empty setting cannot activate
native CMA there. The existing `microvm` RuntimeClass uses handler `kata-clh`,
adds node selector `sandbox.gke.io/runtime=microvm` and its matching
`NoSchedule` toleration, and charges `250m` CPU plus `130Mi` memory Pod overhead.
These are infra-owned RuntimeClass settings; CMA selects the class without
modifying them. The preflight requires an allowed class only for this known
production cluster/namespace and checks any explicitly selected class exists,
is not deleting and has a handler. Other deployments may omit the setting.
Class existence and this specific admission check do not prove complete policy
acceptance, node capacity, image compatibility or live RWOP scheduling.

Keep the legacy `CHRONO_SANDBOX_HOME_*` settings pinned to the northeast seed
and retained allocations. For new native homes in southeast, set
`CHRONO_AGENT_SANDBOX_HOME_MODE=native_vct_empty_v1`,
`CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS`, and
`CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB` (1–1024 GiB). Leave
`CHRONO_AGENT_SANDBOX_HOME_TEMPLATE` and
`CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID` empty. A complete native storage class
and capacity pair without snapshot fields also selects this mode. An explicit
mode may inherit storage class/capacity from the legacy configuration, but using
an explicit southeast storage pair makes the cluster boundary clear.

The native Sandbox controller creates one PVC per Sandbox from the volume claim
template. The pinned runtime image supplies the `--initialize-home` init command,
which mounts the PVC root as UID/GID 10001 and initializes the `home`, `state`,
and `transfer` layout before the runtime starts. It uses a completion marker to
make Pod recreation safe. The runtime receives credentials only after CMA
records and verifies the original Claim, Sandbox, and PVC UIDs. A zero-replica
CMA-owned pool supplies the controller's cold SandboxClaim fallback when no
warm pool exists. Pool scaling and cold fallback use the same template contract.

New native allocation does not require VolumeSnapshot discovery or PVC
create/patch/delete permissions for CMA. It requires PVC get/list to verify the
recorded disk identity, Pod get/list to establish readiness and deletion finality,
and Pod exec get/create to deliver runtime assignment. Pause preserves the
Sandbox and PVC. Delete retires the Claim/Sandbox and waits for controller GC
of the original PVC and Pod; CMA never issues a direct PVC delete for this mode.
CSI must support `ReadWriteOncePod`. Verify the runtime image pin includes the
initializer as well as runtime pool assignment support before rollout.
The production preflight checks that the chosen StorageClass exists and uses
GKE PD CSI. That check does not prove live RWOP provisioning or scheduling;
those remain activation acceptance checks. The Pod/PVC templates require RWOP,
while the GKE provisioner choice is a production check rather than an inherent
requirement of the agent-sandbox API.

The production native storage choice is `home-a`, retaining the 64 GiB request.
The 2026-10-13 isolated canary using `standard-rwo` provisioned a PVC but failed
disk attachment: Compute rejected its `pd-balanced` disk on the selected
`n4-standard-16` microvm node. The infra-owned `home-a` class uses
`pd.csi.storage.gke.io` with `type=hyperdisk-balanced`, a regional CMEK key,
`WaitForFirstConsumer`, `Delete` reclaim policy and volume expansion enabled.
Checking the CSI provisioner alone had missed this disk/node incompatibility.
The replacement canary with a pinned diagnostic Python image passed real
64 GiB `ReadWriteOncePod` allocation and mounting on `home-a`. Suspending the
Sandbox removed its Pod while retaining the same PVC UID; resuming created a
new Pod UID, retained that PVC and read the saved marker with its boot count
incremented to two. Foreground Claim deletion was followed by controller GC of
the Sandbox and PVC. This proves the infrastructure lifecycle and persistence
substrate. It does not prove the final CMA runtime's home initializer,
authenticated assignment, bootstrap or end-to-end Agent lifecycle, which still
require the final image and a working remote runtime network path.
The legacy `CHRONO_SANDBOX_HOME_STORAGE_CLASS=cma-home-v1` remains pinned for
retained northeast allocations.

Historical four-field native home overrides still select `legacy_snapshot`;
`CHRONO_AGENT_SANDBOX_HOME_MODE=legacy_snapshot` makes that choice explicit.
Those allocations retain their original snapshot, scope, request digest, and
PVC cleanup behavior and require their historical storage permissions. When
all native overrides and mode are empty, configuration inherits the legacy
policy for compatibility. That inheritance never copies a snapshot across
clusters. A partial policy, unknown mode, or an empty-VCT mode mixed with
snapshot fields fails startup. The old seed manifests remain operator inputs
for retained legacy allocations, and their pinned names/UIDs must remain intact.

The application Deployment takes the remote API origin and namespace from
`chrono-bot/example-namespace-02` ConfigMap keys `CHRONO_POD_API_SERVER` and
`CHRONO_POD_NAMESPACE`, through
explicit optional `configMapKeyRef` environment inputs. It projects only the
`ca.crt` key under `/etc/example-registry-01/ca.crt` and maps `CHRONO_POD_CA_FILE` to
`CHRONO_AGENT_SANDBOX_CA_FILE`. These inputs override
`CHRONO_AGENT_SANDBOX_API_SERVER` and `CHRONO_AGENT_SANDBOX_NAMESPACE` from the
application ConfigMap when present; preflight reads the actual Deployment
overrides. The ConfigMap and its namespace are infra-owned, with no copy of
credentials in the repository.

The production remote client uses
`CHRONO_AGENT_SANDBOX_AUTH_MODE=google-metadata`,
`CHRONO_AGENT_SANDBOX_CA_FILE=/etc/example-registry-01/ca.crt` and an empty
`CHRONO_AGENT_SANDBOX_TOKEN_FILE`. KSA `chrono-bot-ksa` retains its Workload
Identity annotation for `example-sa-01@example-project-02.iam.gserviceaccount.com`.
The client obtains and refreshes short-lived access tokens through the metadata
service and verifies the remote API's TLS certificate against the projected CA.
It does not execute gcloud, run kubeconfig plugins or replace the local legacy
client's credentials.

For explicit file authentication, select `token-file` (the parser default) and
configure the API origin, CA file and token file together. File contents are
refreshed approximately every minute, and the credential owner rotates the
token before expiry. Metadata mode rejects a token-file input. Missing or
invalid credentials fail the native client rather than falling back to local
Kubernetes authority. These credentials stay in the CMA control plane.
Native home records persist an authority derived from the API origin, namespace
and deployment identity before allocation. Keep that authority stable while
its allocations exist; changing to another URL spelling or endpoint does not
silently migrate retained homes to a different client or cluster.

On 2026-10-13, the live northeast CMA Pod was verified to obtain the expected
`chrono-cma` metadata identity and use its short-lived access token to read
Sandbox and claim resources in `example-registry-01` through `https://35.240.157.32`,
with CA validation. Both reads returned HTTP 200. The ConfigMap's current
namespace is `example-registry-01`; the older last-applied annotation is not current
configuration and must not determine the target. The private API endpoint was
unreachable from CMA. No static token handoff is needed for metadata auth.
The ConfigMap also names warm pool `python-a`; CMA does not adopt it automatically.
Its ownership, immutable runtime contract and retained-home policy have not
been verified as a CMA pool binding.

The later 2026-10-13 recheck used a fresh token obtained from the actual northeast
CMA Pod's metadata server and remote TLS CA to perform SelfSubjectAccessReviews.
The principal `example-sa-01@example-project-02.iam.gserviceaccount.com` is allowed
Sandbox get, Claim create, PVC get and Pod exec get/create. PVC create/patch
remain denied. This supersedes the earlier missing-exec observation; no Role or
RoleBinding was changed by CMA. New native allocation delegates empty VCT
creation to the controller, so those denied PVC writes are not an infra blocker.
Exec currently provides the authenticated initial assignment channel. Successful
authorization checks alone do not prove allocation, assignment, remote runtime
connectivity or persistent-home acceptance.

The earlier prospective checked-in application configuration passed all 37
operator preflight checks against the live infra ConfigMap, remote schemas, exact
impersonated authorization reviews, runtime account and StorageClass. That
version did not check RuntimeClass or Pod admission; the later isolated probe
found the missing class described above. The expanded preflight adds the
selected class and known production class requirement. The revised prospective
`microvm`/`home-a` application configuration passed all 38 checks against live
infrastructure on 2026-10-13. These checks substitute the checked-in application
ConfigMap and Deployment for the not-yet-deployed candidate; the remaining
reads and authorization reviews use the actual clusters. Neither the earlier
37 checks nor the expanded checks attest a deployed candidate or live runtime
behavior. Earlier
`kubectl auth can-i` failures arose from discovery and subresource parsing;
direct SelfSubjectAccessReviews confirm the actual grants without requesting
additional permissions.

## Build and configure the selected application images

For the current rollout, import the tested `api`, `web`, `runtime` and `cmaeg`
images from the same reviewed source tree. The existing release workflow still
publishes all five components when used for a complete release; its publication
does not deploy to Kubernetes. Pin the final registry digests in the selected
application manifests. Image Manager remains at `replicas: 0`; its `:unreleased`
placeholder must be replaced only before its later activation.

For an authorized direct application rollout, the CI `ci-native-image-*`
artifacts provide the already tested Linux amd64 images. Require passing checks
for the complete application candidate and verify each artifact's archive
checksum, `sourceRevision`, `sourceTree`, image ID and workflow run identity.
A pull-request artifact records GitHub's actual merge checkout: its source tree
must equal the reviewed deployment candidate's tree, and the image revision
label must match that recorded checkout. Import those exact bytes and verify
the destination digests before updating manifests. Images from the separate
CI tooling fix or an earlier passing portal tree do not validate later
authentication, database, service-name or registry changes.

Use these destinations for the current direct rollout:

| CI component | Destination repository | Deployment use |
|---|---|---|
| `api` | `us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api` | `cma-agent-manager` Deployment |
| `cmaeg` | `us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-cmaeg` | `cma-event-gateway` Deployment |
| `web` | `us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-web` | Shared portal Deployment |
| `runtime` | `asia-southeast1-docker.pkg.dev/example-project-02/example-registry-01/cma-runtime` | `CHRONO_SANDBOX_IMAGE` system default and rollout annotation |

The exact artifact names are
`ci-native-image-COMPONENT-SOURCE_REVISION-RUN_ID-RUN_ATTEMPT`. Obtain them from
the successful candidate run's artifact inventory, rather than constructing
`SOURCE_REVISION` from the PR head. Each contains `metadata.json` and
`cma-COMPONENT-linux-amd64.docker.tar.gz`; download each into its own directory
with `gh run download RUN_ID --repo ChronoAIProject/cma --name ARTIFACT_NAME
--dir COMPONENT_DIRECTORY`. Download the same run's
`runtime-image-evidence-RUN_ID-RUN_ATTEMPT` as well. Native image artifacts expire
after one day, so preserve verified bundles with the deployment evidence.

Before any registry write, require the full run to pass and confirm its head
matches the selected candidate. Check the artifact run ID/attempt, component,
repository, source revision/tree, Linux amd64 platform, archive size and SHA-256
against `metadata.json`. Inspect the Docker archive's config and require its
SHA-256 to equal `imageId`; verify the source/revision/version labels and both
runtime labels above. The runtime evidence must bind the same source revision
and immutable image ID and report passing execution checks. The four artifacts
must agree on source revision and tree. A failed or superseded run is not a
deployment source even if one image job succeeded.

Decompress each verified archive into a private evidence directory and import
with `skopeo copy --dest-authfile AUTH_FILE --digestfile DIGEST_FILE
docker-archive:ARCHIVE docker://REPOSITORY:UNIQUE_CANDIDATE_TAG`. `AUTH_FILE` is an
operator-managed private file; never include bearer values in command arguments
or the journal. The tag should include the candidate SHA, run ID and attempt.
Then read the destination by the returned digest, verify its manifest/config
identity and source/platform/runtime labels, and record the immutable
`REPOSITORY@sha256:...` reference. Docker-save transfer establishes a registry
manifest digest at import; the Docker image ID is its config identity, not the
registry manifest digest. Keep both in the evidence. The release OCI importer
`.github/import-release-images.sh` expects a different five-component release
bundle and must not be used for these four native Docker archives.

Every native pool runtime image must actually implement runtime contract 3 and
bootstrap assignment version 1, with these final-image labels:

```text
dev.chrono.cma.runtime-contract=3
dev.chrono.cma.runtime-bootstrap=1
```

The release evidence gate checks both labels, exact source/image identity and
the existing runtime executable probes. Labels alone do not prove assignment:
verify that `--wait-for-assignment` creates its Unix socket with no TCP listener
and no tenant credentials, and that an exact Pod UID assignment starts the
authenticated runtime. Imported custom Dockerfiles should extend a tested CMA
runtime digest and preserve its executable, non-root identity and labels.

Create the `cma-agent-sandbox-control` Secret's `control-key` using the approved
secret workflow. Its mounted file is required when the native backend is
enabled; it must contain 32–8192 bytes. Do not copy a token into a ConfigMap,
runbook, PR or shell command. Keep the key stable across restarts so retained
runtime control tokens continue to match. The control-plane Secret volume is
optional only while the native backend is disabled.

Set the new runtime digest in `CHRONO_SANDBOX_IMAGE` and the matching deployment
rollout annotation. `CHRONO_AGENT_SANDBOX_RUNTIME_CLASS`, runtime ServiceAccount,
pull secrets, home policy and deployment ID must match the reviewed production
configuration. Existing pool bindings retain their original Pod policy when a
later release changes those defaults.

The requested production topology uses the existing `chrono-bot` MongoDB
database for all three services, with strictly separate service-owned collections
and schema migrations. Configure the explicit shared-schema authority mode
described in [service isolation](CMA_Service_Isolation.md); the default isolated
mode continues to require collection-scoped credentials. Reusing a database
does not permit one service's persistence implementation to read or mutate
another service's collections.

| Service | Database setting | Explicit production authority mode |
|---|---|---|
| Agent Manager | `CHRONO_MONGODB_DATABASE=chrono-bot` | `CHRONO_MONGODB_AUTHORITY_MODE=shared-schema` |
| Event Gateway | `CMAEG_MONGODB_DATABASE=chrono-bot` | `CMAEG_MONGODB_AUTHORITY_MODE=shared-schema` |
| Image Manager | `CMA_IM_MONGODB_DATABASE=chrono-bot` | `CMA_IM_MONGODB_AUTHORITY_MODE=shared-schema` |

Each service still receives its own URI configuration from its Secret. The
shared-schema mode verifies that the authenticated account has every required
operation in the selected database; it does not grant missing rights or migrate
other services' collections. It allows broader collection grants within that
database and does not claim credential-level isolation between the services.

Before upgrading Agent Manager, inspect the new `cma --print-runtime-role`
output for its required collection grants. It includes `native_warm_pool_targets`
for durable pool plans, revisions, leases and original resource bindings.
Agent Manager initializes its own collections and indexes and has no separate
migration role. Every selected credential mode must still supply the required
operations for those collections.

## Deferred Image Manager / Cloud Build activation

This is a separate later activation, outside the current Agent Manager/Event
Gateway rollout. Keep Image Manager at zero replicas until its prerequisites
are available. Its absence does not block the system-default runtime path.

When Image Manager is activated, select the existing `chrono-bot`
database and explicit shared-schema authority mode for Image Manager as well
as Agent Manager and Event Gateway. Run the released
`cma-image-manager --migrate` before starting the service; schema marker 2 is
required. Migrations and runtime persistence operate only on Image Manager's
declared collections. In the default isolated mode, retain a separate database
and exact collection-scoped runtime/migrator credentials; broader credentials
are accepted only by the explicitly selected shared-schema mode. Use
`--print-runtime-role` and `--print-migration-role` to generate required
collection grants without printing credentials.

Provision the dedicated NyxID client and browser cookie key described in
`infra/image-manager/secret.yaml`. Register the callback at
`https://bot.chrono-ai.fun/api/image-manager/auth/callback`, the service resource
`cma-image-manager`, and the documented `cma-im:` permissions. Event Gateway and
Agent Manager keep their own clients and session keys; see
[service isolation](CMA_Service_Isolation.md).

Cloud Build is fixed to project `example-project-02`, location `asia-northeast1`
and destination
`asia-southeast1-docker.pkg.dev/example-project-02/example-registry-01`. Enable Cloud
Build, Artifact Registry and Cloud Logging APIs. Before scaling Image Manager:

1. Bind KSA `chrono-bot/cma-image-manager` to
   `cma-image-manager@example-project-02.iam.gserviceaccount.com` through GKE Workload
   Identity. The API obtains short-lived access through the metadata service.
2. Grant that same identity Cloud Build create/get/list, Cloud Logging log-entry
   read permissions and `iam.serviceAccounts.actAs` on itself. Use custom roles or
   the corresponding reviewed predefined roles.
3. Grant that same identity Artifact Registry writer on only `example-registry-01` and
   Cloud Logging writer. Image Manager submits and reconciles the job while the
   Cloud Build runner executes the Docker steps under this identity; do not give
   it application or production-secret access. Check the Cloud Build service
   agent's required service-account token permissions for that identity.
4. Set `CMA_IM_BUILD_SERVICE_ACCOUNT` to
   `projects/example-project-02/serviceAccounts/cma-image-manager@example-project-02.iam.gserviceaccount.com`
   and
   `CMA_IM_BUILDER_IMAGE` to a verified
   `gcr.io/cloud-builders/docker@sha256:...` digest. The empty checked-in builder
   digest deliberately prevents accidental activation.
5. Keep `CMA_IM_GOOGLE_TOKEN_FILE` empty in production, replace the unreleased
   service image, migrate the database, then scale the service and verify
   `/readyz`. Confirm the same-origin `/api/image-manager` Gateway route works.

Build input accepts a public GitHub repository, full lowercase commit SHA,
repository-contained context/Dockerfile paths and optional UTF-8 imported
`dockerfile_content` up to 16 KiB. Private source credentials and source-archive
uploads are not implemented. Imported content is written into a dedicated
Cloud Build volume, outside repository-controlled paths, then built against the
selected checkout context. The dispatcher observes both import and build steps,
their exact executable arguments/volume bindings and destination before
publishing a successful immutable digest. A lost submission response is
reconciled using its original unique build tag without submitting a second job.

## Read-only preflight and activation order

Keep the checked-in values until infra and image prerequisites are met:

```text
CHRONO_SANDBOX_PROVIDER=opensandbox
CHRONO_RETAIN_OPENSANDBOX=true
CHRONO_AGENT_SANDBOX_ENABLED=false
```

After infra provides the native namespace, remote principal and storage, set
`NATIVE_NAMESPACE` and `NATIVE_PRINCIPAL` to those reviewed nonsecret identities
and run from the repository root:

```sh
node .github/scripts/agent-sandbox-preflight.mjs \
  --context gke_example-project-02_asia-northeast1_example-cluster-02 \
  --namespace chrono-bot \
  --service-account chrono-bot-ksa \
  --runtime-context gke_example-project-02_asia-southeast1-b_example-cluster-01 \
  --runtime-namespace "${NATIVE_NAMESPACE:?set the infra-assigned native namespace}" \
  --runtime-principal "${NATIVE_PRINCIPAL:?set the native client's Kubernetes principal}"
```

This script reads `cma-agent-manager` configuration and Deployment overrides
only from the CMA context, then checks native API
schemas, the configured runtime ServiceAccount's existence and non-deletion,
the selected RuntimeClass and the known southeast `example-registry-01` class requirement,
the effective seed and storage class
in the runtime context. It checks the configured native namespace and remote
API origin against the selected runtime cluster and performs namespaced
authorization checks as the explicit remote principal. These checks submit
non-persistent `SelfSubjectAccessReview` requests to the exact authorization API
endpoint, with API group, resource and subresource supplied in the JSON body.
They bypass kubectl resource discovery and do not interpret `pods/exec` as a
Pod named `exec`. The script does not read Secrets
or modify workloads. A missing/incompatible CRD, denied right, mismatched target,
incomplete policy or replaced/unready remote seed returns nonzero. Omitting the
runtime context preserves the same-cluster mode; a distinct runtime context
requires an explicit remote principal.

The caller must be allowed to impersonate that remote principal for authorization
checks. For the current metadata identity, the principal is
`example-sa-01@example-project-02.iam.gserviceaccount.com`. A permission result for an
impersonated principal does not prove the deployed client represents it; verify
actual authenticated requests from CMA.
Each API command has a 10-second server request bound and every command has a
15-second local timeout. A passing result does not prove mounted credential
identity, controller image provenance, OCI runtime compatibility, network
enforcement or live provisioning.

Then deploy the application release with the native backend enabled and retained
OpenSandbox support still enabled. Confirm existing runtime recovery before
selecting `CHRONO_SANDBOX_PROVIDER=agent-sandbox` for new allocations. Keep only
one Agent Manager replica and its existing Recreate rollout strategy; process
admission quotas do not yet have shared coordination. Native warm-pool
administration should start with one member of the verified immutable runtime
image and grow only after the checks below pass.

## Deployment acceptance still required

Agent access-request tools use the existing auth encryption key to derive a
separate, deployment-scoped `cma_at_` capability. The `agent_access_tools`
collection stores only its hash, exact Agent creation/runtime identity and
confirmed native conversation. This capability can request owner consent; it
cannot approve a request or grant a NyxID service scope. Keep the encryption key
and public HTTPS origin stable across an ordinary API rollout so the same live
conversation keeps its installed bearer. The collection belongs only to Agent
Manager and is included in its collection authority contract.

`agent_access_requests` stores the owner's exact approved scope, personal
sign-in incarnation, original conversation and resumable workflow progress. The
popup callback verifies every approved service connection. The worker then
waits until every live Agent in the Sandbox is quiet before atomically admitting
the consented pause. Scope grant and revival have separate leased transactions
that preserve their exact lifecycle operation identities. The request reaches
Succeeded only after replacement key delivery and confirmed delivery of the
idempotent continuation command to the original conversation. A closed popup,
an OAuth redirect, or a queued command alone is insufficient. Revoked owner
authority, a changed Agent identity or a later incompatible manual lifecycle
must not be borrowed as authorization to continue the workflow.

New Agents negotiate explicit ACP HTTP MCP support before tool installation.
Retained Agents without a confirmed tool-open receipt continue without the new
tool; deploying an API image does not modify their live conversations. A worker
takeover for a previously enrolled Agent preserves its bearer and exact native
conversation. A replacement runtime receives a distinct bearer through a fresh
ACP handshake; an existing bridge cannot claim installation without its exact
durable receipt. Old runtime, changed creation and retired-Agent credentials
must fail authentication.

Record source SHA, the four current final image digests, controller digest/revision, native
API preflight, native home mode and Claim/Sandbox/Pod/PVC UIDs. Record a seed UID
only when testing the retained legacy mode. Execute these checks after
deployment; local fixtures do not substitute for them:

- Sign into Agent Manager and Event Gateway with their own NyxID sessions.
  Verify the unified dark UI, service navigation, connection popup, the
  selectable system default while Image Manager is unavailable, and
  permission denial for an unconnected or unauthorized service.
- Prove direct authenticated native API access from the northeast CMA Pod into
  the assigned southeast namespace, with CA validation and credential rotation.
  Record both cluster contexts and verify native CRD/Pod/PVC creation remains in
  southeast while retained OpenSandbox recovery and cleanup remain in northeast.
- Deferred Image Manager acceptance: sign into its own NyxID session and verify
  private/public image selection. Run both a repository Dockerfile build and an imported Dockerfile build.
  Observe running/success, owner-only bounded logs, exact Artifact Registry
  digest, authorized resolution, private/share/public visibility and a failed
  build that never publishes a version. Check unknown-response recovery without
  duplicate Cloud Build submissions.
- Create an Agent with the tested system default image; repeat with an
  authorized custom image after Image Manager activation. For the default,
  verify the three stages (Sandbox, Agent/workspace, scoped key), exact CRD/Pod/
  PVC identities, empty workspace, gpt-6/high/autonomous defaults and required
  chrono-llm-public/ornn-api scope. With an optional GitHub URL, verify it reaches
  the first prompt and repository setup is performed by the Agent. Verify a
  real turn and export JSONL from the Agent menu.
- Add empty and GitHub workspaces to that Sandbox, then create an Agent from
  an existing Workspace. Verify the original Sandbox/Workspace identities and
  inherited service scope, with no new Sandbox allocation.
- From an enrolled Agent, request a missing NyxID service through its MCP tool.
  Verify explicit owner approval, additive scopes, callback recovery and a
  follow-up to the same native conversation. Repeat across an Agent Manager
  restart; reject a stale runtime bearer and prove that an unrelated or
  previously unenrolled retained Agent continues its existing conversation.
- Create a native pool through Operations, observe controller readiness, resize
  at the current revision, and set its target to zero. Verify stale revisions
  are refused and claimed Agents retain their runtime and disk when the pool
  stops replenishing.
- Claim a ready native member, prove its prewarmed Pod survives PVC adoption,
  and verify owner configuration is assigned exactly once. Restart Agent
  Manager between durable provisioning stages and verify recovery of the same
  claim, member and disk. Test cleanup before runtime admission.
- Pause and resume with the same Sandbox CRD/PVC and a new Pod/boot identity;
  verify user files and conversation continuity. Delete active and paused
  Sandboxes, including a retry after lost response: claim first, then CRD and
  every owned/terminating Pod, then PVC and CMA records.
- Launch a Trigger and Event Gateway consumer using image + inline Agent
  configuration; verify real routing, a model reply and consumer controls.
  Test Sandbox sharing with another authorized user and confirm no legacy
  Profile/snapshot management UI is required.
- Verify unrelated Pods cannot access native runtime TCP 9090 and runtime Pods
  have no Kubernetes API token. Retire all acceptance resources and record exact
  claim/CRD/Pod/PVC cleanup without touching other users' resources.

Reverting the default provider changes only new allocation selection. Keep the
native backend enabled while any native runtime, suspended binding, pending
claim or retained cleanup debt exists. Roll back only to an application image
that understands the persisted native lifecycle stages and runtime protocol;
changing an image tag cannot migrate or safely erase that state.


### Native Template network policy

`CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY` is an operator-only JSON object
with explicit `ingress` and `egress` arrays. CMA submits these rules only through
its owned SandboxTemplate, with `networkPolicyManagement: Managed`. The native
controller owns the resulting NetworkPolicy, its pod selector and policy types;
CMA needs no NetworkPolicy write permission. CMA requires both directions because
the controller replaces its complete default policy with the custom rules. An
empty array grants no traffic in that direction from this policy. NetworkPolicy
allows are additive: other policies selecting the same Pod can still allow
traffic. CMA rejects an omitted direction. Named ports are not accepted; use
explicit TCP/UDP protocols and numeric ports.

The checked-in value remains empty pending an actual cross-cluster network proof.
The controller's managed default ingress admits its local sandbox-router only,
so an empty value does not establish direct CMA control-plane connectivity. A
reviewed custom policy must admit the observed control-plane source on TCP 9090,
retain the required DNS path and allow the runtime's approved public egress. A
source CIDR identifies addresses, not an individual CMA deployment; runtime
authentication remains required. Use the actual runtime image and
controller-created managed policy in the rollout canary.

On 2026-10-13, diagnostic `cma-managed-net-1aa0c364` tested the pinned Python
runtime using a controller-owned Managed Template policy. The runner verified
the Template owner UID, exact Pod selector, both policy types and complete rules
before each phase. The initial custom policy had no ingress allowances. Its
egress matched `chrono-pod-default`: UDP/TCP 53 to Pods labeled
`k8s-app=kube-dns` **in** namespace `kube-system`, plus public IPv4 excluding
`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10` and
`169.254.0.0/16`. No IPv6 allowance was added.

The baseline requests from northeast Pod `10.41.0.104` to ready southeast Pod
`10.57.0.99:9090` timed out. The diagnostic then added ingress from
`10.254.0.0/16` on TCP 9090. After the same generated NetworkPolicy advanced to
generation 2 and its exact rules were verified, all 15 allow-phase requests
still timed out after four seconds each. The post-allow TCP 8080 check was not
reached. The source Pod, Claim, Pool and Template were removed with UID
preconditions, the generated policy was garbage-collected, and the infra policy
UID and spec were unchanged. This proves custom-policy reconciliation and
cleanup, but does not establish runtime connectivity or production acceptance.

Both clusters use `ADVANCED_DATAPATH`. [Google's Dataplane V2
documentation](https://docs.cloud.google.com/kubernetes-engine/docs/how-to/network-policy#ipblock-dpv2)
states that Pod traffic is never covered by an `ipBlock` rule. This is a material
limitation of using a Pod CIDR as the source selector; it does not establish how
the southeast dataplane classified this particular remote-cluster source or
prove the cause of these timeouts. Effective VPC firewall rules could not be
read because the current account lacks `compute.firewalls.list` and
`compute.instances.get`; the narrow firewall-flow log query also lacked access.
Missing cross-cluster firewall rules therefore remain unconfirmed. Infra must
verify effective ingress to destination node
`gke-chronoai-prod-asia-south-egress-a-bd6f4de5-bhd9` in
`asia-southeast1-b` and effective egress from the northeast source for this
TCP 9090 path before rollout.

Policy is frozen in durable pool plans, bindings and claim/home authority. A
changed deployment policy selects a new pool identity; existing reservations
retain their original policy. Historical definitions without this field keep
the exact previous digest and serialization. CMA refuses drift in an explicit
Template policy and verifies its original controller template-ref hash on the
assigned Pod template during adoption and resume. Template-only fields are never
copied into the core Sandbox spec, and resume preserves the selector label.

The pinned agent-sandbox v1.0.2 controller does not inject public DNS when custom
rules are configured. Explicitly review cluster DNS reachability in the selected
egress policy; do not copy only the ingress exception and accidentally drop DNS
or public endpoint access.
