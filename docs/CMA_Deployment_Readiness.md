# C-001 deployment handoff

For exact deployed digests, CI/image provenance, measured flows, historical
failures and receipt paths, see
[CMA native Sandbox production verification](CMA_Native_Production_Verification.md).
This document keeps operational instructions and dated infra diagnostics.

The current Agent follow-up candidate `03cbd332` was Ready at **16:26:14 SGT on
2026-10-14**: Agent Manager generation 15, Web generation 91, and retained Gateway
generation 6 each had one updated Ready replica, exact published imageIDs, and
zero restarts. The default runtime now uses the verified `a955564d` digest for
new allocations. PR #1007 merged into `develop` as `ab789bde`; CI run
[38528498008](https://github.com/ChronoAIProject/cma/actions/runs/38528498008),
attempt 1, passed all 15 jobs. Image Manager activation remains deferred.

Exact digests, actual image source revision/tree, and receipt hashes are in the
[current follow-up report](CMA_Native_Production_Verification.md#current-agent-follow-up--162614-sgt).
Publication: `/tmp/cma-agent-followup-publish-04/publication-report.json`.
Readiness: `/tmp/cma-agent-followup-rollout-01/readiness.json`.
The separate `/tmp/cma-agent-followup-pool-01/ready.json` receipt confirmed two
Ready pool replicas at 16:26:50 SGT. Fresh Agent acceptance and timing remain
pending: a subsequent owner attempt reported **Needs attention** during Sandbox
preparation and is under investigation.

The earlier cd37968d/ac837/cabfe baseline passed cold Gateway acceptance with
one actual UI request, one completed turn, and visible exact acknowledgment in
42.624 seconds; cleanup completed at 11:45:14 SGT. That earlier result is retained
under its tested versions and does not measure the current follow-up.

The three portals ship from one web image: Agent Manager (`/agent-manager`),
Image Manager (`/image-manager`) and Event Gateway (`/event-gateway`). Their
backends remain separate services with their own collection schemas. This handoff
defines application deployment against the infra-owned native runtime cluster.
Infra continues to own its controllers, CRDs, RBAC, storage and network policy;
CMA's completed application rollout did not change those resources.

The current follow-up updates Agent Manager, the shared web image, and the
system default CMA runtime while retaining the verified Event Gateway. Image
Manager is deferred: its Deployment is absent; keep it absent or at zero replicas and do
not require its Cloud Build identity, OAuth client, migrations or builder digest to activate Agent
Manager. Default-image Agent, Trigger and consumer creation does not call Image
Manager. Its catalog may be unavailable while the system default remains
selectable; custom image selection and builds require the later Image Manager
activation described below.

| Authority | Kubernetes context | Namespace |
|---|---|---|
| CMA application, ConfigMap and retained OpenSandbox resources | `gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke` | `chrono-bot` |
| Native Sandbox, pool, claim, runtime Pod and home resources | `gke_chronoai-501608_asia-southeast1-b_chronoai-prod-asia-southeast1-gke` | Infra-assigned native namespace, set in `CHRONO_AGENT_SANDBOX_NAMESPACE` |

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
exact `http://cma-api.chrono-bot.svc` authority, so `CMAEG_CMA_BASE_URL` retains
that origin. The CMA provider registry uses the same origin so outbound binding
management receives the dedicated service credential under its exact-origin
authentication rule. Other internal references use canonical names. The `cmaeg` alias preserves retained CMA configurations during the staged
rename. Do not rewrite stored pointer identities to match a cosmetic name.

The canonical-name cutover is already deployed. The following first-cutover
procedure applies only when migrating another deployment; do not replay its
Secret or record transfer against this running release.

Before a first deployment under canonical names, copy the existing Secret
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

This table reflects the completed native deployment and remaining measured
limits on 2026-10-14. Historical failures are retained with dates and outcomes,
without treating superseded findings as current activation blockers.

| Prerequisite | Verified deployed state | Continuing obligation or coverage limit |
|---|---|---|
| Native Kubernetes authentication | CMA uses the `chrono-cma@chronoai-501608.iam.gserviceaccount.com` metadata identity and verified remote CA; actual native allocation and runtime operations passed through Agent Manager | Preserve renewable metadata authentication and CA/server binding; no dedicated long-duration credential-expiry campaign is claimed |
| Native target | `chrono-bot/chrono-pod-configmap` supplies `https://35.240.157.32`, namespace `chrono-pod` and the remote CA | Read current fields, not old last-applied annotations or the superseded `cma-test` namespace |
| Native authorization | Actual metadata principal can read Sandboxes/claims/PVCs and use `pods/exec`; new native homes delegate PVC creation to controllers | Direct PVC create/patch remains unnecessary for new empty VCT homes; preserve exact Pod/PVC identity reads and retained legacy permissions |
| Native persistent home | Final runtime 37d passed 64 GiB `home-a` initialization, assignment/replay, same-PVC suspend/resume and controller cleanup; real Agent creation and later same-home revive also passed | Retained Sandboxes keep their recorded home and runtime identity; a default-image update does not migrate them |
| Runtime image residue | Historical candidate `2bc6fe1c` failed startup with a build-test `install-v2.json`; final 37d scan found zero matching residue across all 15 logical layers | Keep isolated build-test state and exact final-image scan/canary checks for future runtime changes |
| Runtime transport | Deployed `kubernetes-exec` carries runtime HTTP/WebSocket traffic through the authenticated remote API; real model turns and complete history exports passed | Earlier direct Pod-IP and CRI port-forward diagnostics did not establish a working microVM path. They do not require a network change for the selected exec transport |
| Runtime ServiceAccount/storage/class | New runtime templates select unannotated remote `default`, disable Pod token automount and use `microvm` with 64 GiB RWOP `home-a` volumes | Preserve infra ownership. Historical `standard-rwo`/`pd-balanced` attachment failed on N4; use the verified `home-a` policy |
| Runtime control key | Existing `chrono-bot/cma-agent-sandbox-control` supplies the mounted application signing key | Preserve the key across restarts; preserve it during application updates |
| Canonical application workloads | `cma-agent-manager`, `cma-event-gateway` and `cma-web` are deployed; latest readback confirms 1/1 updated Ready, current generations, matching image IDs and zero restarts | Final AM cd37968d, Web ac837 and Gateway cabfe were Ready at 11:31:31 SGT; final Web visual acceptance passed. Keep one Recreate Agent Manager and preserve canonical/compatibility Service routing |
| Shared MongoDB | Running Agent Manager/Gateway use `chrono-bot` with separate collection ownership; retained durable identities and complete history prefixes survived the AM rollout | Run only each service's migrations. Shared-schema mode is collection ownership, not credential-level isolation |
| Retained Gateway state | Gateway uses `eg_*` collections and preserves exact `http://cma-api.chrono-bot.svc` authority. Earlier real owner edits, selected-Agent management tests and public custom ingress passed | Do not repeat the historical first-cutover export/import. Preserve original record identities and sealed pointers; final Gateway/browser-specific acceptance remains separately recorded |
| Gateway cold inline-Agent acceptance | Final measured result and the earlier 5ce/AM 37d failures are recorded in the verification report | Preserve the 90-second startup and fresh 30-second acceptance/completion budgets; only explicit Busy may retry the same body/key |
| Gateway feature coverage | Integrated source and complete CI include Console/Media/STT code and authenticated Mongo contracts | Selected live routing/channel/model checks do not establish full Console/media/STT, external reply sinks or Trigger acceptance; preserve existing configuration and Secrets |
| Application image registry | The selected AM cd37968d, Web ac837 and Gateway cabfe digests are published in the established application registry; the verification report lists exact pins | Final AM cd37968d and Gateway cabfe passed full CI 15/15; final Web ac837 passed its four scoped jobs, exact publication and visual acceptance. All selected images are published and Ready; original release receipts remain independently attributable |
| Native runtime registry | Final 37d runtime is published under `asia-southeast1-docker.pkg.dev/chronoai-501608/chrono-pod/cma-runtime`; native canary and real Agent runtime image-ID checks passed | Preserve this default and retained images during Web/Gateway-only changes |
| Deferred Image Manager identity | The 2026-10-13 account lookup returned 404 and operator IAM lacked provisioning/build rights; Image Manager has no active Deployment | This is a separate later activation. Infra must provide the documented build identity/permissions before Image Manager migration, scale-up and live build acceptance |

## C-001 implementation map

The Journey Book is the audit baseline; the current implementation follows the
requested C-001 changes below. Implementation is not a claim that every live
acceptance item passed. The verification report separates completed native/API
flows and final visual checks from deferred Cloud Build/OAuth coverage.

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

The non-CRD runtime dependency is `pods/exec`: CMA uses it for authenticated
initial assignment and to carry runtime HTTP/WebSocket traffic to the microVM
guest. Pod/PVC reads retain exact incarnation, home and cleanup checks. Native
Sandbox status does not provide those UIDs. Direct Pod-IP connectivity and CRI
port-forward did not establish a working production transport; changing from
exec requires a separately tested transport and bootstrap contract.

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
do not confer authority in the remote cluster. The deployed remote metadata
identity has separately provided namespaced authorization. A new deployment
must obtain its own reviewed remote identity and grants from infra.
Validate the actual remote principal's grants; do not apply the northeast
application manifests to southeast as a substitute. The table below describes
the **preflight grant set**, not the minimum permissions intrinsic to
agent-sandbox. Native empty homes do not require the legacy PVC/snapshot writes;
exec is the selected production transport. This table is not a request to extend
infra's Role:

| API/resource | Checked verbs | Purpose |
|---|---|---|
| `agents.x-k8s.io/sandboxes` | get, list, create, patch, delete | Provision, observe, suspend, resume and retire |
| `extensions.agents.x-k8s.io/{sandboxtemplates,sandboxwarmpools,sandboxclaims}` | get, list, create, patch, delete | Native pool administration and claims |
| core `persistentvolumeclaims` | get, list for new native homes; create, patch, delete only for legacy detached-home paths | Exact owned-volume receipt and cleanup verification; controllers own new VCT volumes |
| core `pods` | get, list | Verify exact Pod identity and deletion finality |
| core `pods/exec` | get, create | kube-rs WebSocket assignment over stdin and selected runtime HTTP/WebSocket transport |
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
control-plane Pod in another cluster. A direct-TCP transport would require
infra to configure and verify cross-cluster ingress for both cold and warm
Pods. The deployed exec transport instead uses authenticated remote Kubernetes
requests against the exact runtime Pod. Successful exec traffic does not prove
Pod-network isolation; any NetworkPolicy enforcement claim requires its own
independent evidence.
On 2026-10-13, connection attempts from northeast CMA to the ready diagnostic
Pod at `10.57.0.97` timed out after four seconds on both TCP 9090 and TCP 8080.
That diagnostic used a pinned Python image and an Unmanaged Template. The
subsequent Managed Template diagnostic described below also timed out after
the controller reconciled an explicit TCP 9090 allow rule. Neither diagnostic
used the final CMA runtime. These historical direct-TCP failures did not isolate
policy from routing or other enforcement. The selected exec transport does not
depend on this direct cross-cluster path; the diagnostics are not a current
requirement to alter infra networking.
The policy intentionally does not rely on `cma/runtime-name`, which claimed
warm Pods do not carry. Workload network egress and runtime scheduling remain
infra policy; check the selected RuntimeClass can schedule the requested image,
CPU/memory and disk in the target node pool.

The production application configuration explicitly selects
`CHRONO_AGENT_SANDBOX_RUNTIME_CLASS=microvm`. On 2026-10-13, the southeast
`chrono-pod-sandbox` Kyverno policy was confirmed in `Enforce` mode: Pods in
`chrono-pod` must name either `microvm` or `gvisor`. An isolated Sandbox probe
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
GKE PD CSI. That check alone does not prove live RWOP provisioning or scheduling;
actual home-a canary and Agent-flow receipts recorded here establish those checks. The Pod/PVC templates require RWOP,
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
substrate. The diagnostic itself did not prove CMA initialization or Agent behavior.
Those are covered separately by the final 37d native runtime canary and the real
Agent creation/resume receipts using the selected exec transport.
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
`chrono-bot/chrono-pod-configmap` ConfigMap keys `CHRONO_POD_API_SERVER` and
`CHRONO_POD_NAMESPACE`, through
explicit optional `configMapKeyRef` environment inputs. It projects only the
`ca.crt` key under `/etc/chrono-pod/ca.crt` and maps `CHRONO_POD_CA_FILE` to
`CHRONO_AGENT_SANDBOX_CA_FILE`. These inputs override
`CHRONO_AGENT_SANDBOX_API_SERVER` and `CHRONO_AGENT_SANDBOX_NAMESPACE` from the
application ConfigMap when present; preflight reads the actual Deployment
overrides. The ConfigMap and its namespace are infra-owned, with no copy of
credentials in the repository.

The production remote client uses
`CHRONO_AGENT_SANDBOX_AUTH_MODE=google-metadata`,
`CHRONO_AGENT_SANDBOX_CA_FILE=/etc/chrono-pod/ca.crt` and an empty
`CHRONO_AGENT_SANDBOX_TOKEN_FILE`. KSA `chrono-bot-ksa` retains its Workload
Identity annotation for `chrono-cma@chronoai-501608.iam.gserviceaccount.com`.
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
Sandbox and claim resources in `chrono-pod` through `https://35.240.157.32`,
with CA validation. Both reads returned HTTP 200. The ConfigMap's current
namespace is `chrono-pod`; the older last-applied annotation is not current
configuration and must not determine the target. The private API endpoint was
unreachable from CMA. No static token handoff is needed for metadata auth.
The ConfigMap also names warm pool `python-a`; CMA does not adopt it automatically.
Its ownership, immutable runtime contract and retained-home policy have not
been verified as a CMA pool binding.

The later 2026-10-13 recheck used a fresh token obtained from the actual northeast
CMA Pod's metadata server and remote TLS CA to perform SelfSubjectAccessReviews.
The principal `chrono-cma@chronoai-501608.iam.gserviceaccount.com` is allowed
Sandbox get, Claim create, PVC get and Pod exec get/create. PVC create/patch
remain denied. This supersedes the earlier missing-exec observation; no Role or
RoleBinding was changed by CMA. New native allocation delegates empty VCT
creation to the controller, so those denied PVC writes are not an infra blocker.
Exec provides authenticated initial assignment and the selected runtime
HTTP/WebSocket transport. Authorization checks alone do not prove lifecycle
behavior; the later final-runtime and real Agent receipts supply that separate
evidence.

The earlier prospective checked-in application configuration passed all 37
operator preflight checks against the live infra ConfigMap, remote schemas, exact
impersonated authorization reviews, runtime account and StorageClass. That
version did not check RuntimeClass or Pod admission; the later isolated probe
found the missing class described above. The expanded preflight adds the
selected class and known production class requirement. The revised prospective
`microvm`/`home-a` application configuration passed all 38 checks against live
infrastructure on 2026-10-13. These checks substitute the checked-in application
ConfigMap and Deployment for the candidate that was not yet deployed at the
time; the remaining reads and authorization reviews used the actual clusters. Neither the earlier
37 checks nor the expanded checks attest a deployed candidate or live runtime
behavior. Earlier
`kubectl auth can-i` failures arose from discovery and subresource parsing;
direct SelfSubjectAccessReviews confirm the actual grants without requesting
additional permissions.

## Build and configure the selected application images

The final selected image pins are already published, deployed and synchronized
with the four repository manifests. The verification report records exact
per-component source/run/digest evidence. The following workflow applies to a
future selected release; publication itself does not deploy workloads.

The complete release workflow publishes all five components when selected;
publication does not deploy workloads. Image Manager remains absent or at zero
replicas. Replace its `:unreleased` image only for its later, separately verified
activation. Pin application manifests only to actual verified registry digests.

For an authorized direct application rollout, the CI `ci-native-image-*`
artifacts provide the already tested Linux amd64 images. Require passing checks
for the complete application candidate, except the explicitly scoped AM cd37968d
and Web ac837 gates recorded in the verification report, and verify each artifact's archive
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
| `api` | `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api` | `cma-agent-manager` Deployment |
| `cmaeg` | `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-cmaeg` | `cma-event-gateway` Deployment |
| `web` | `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-web` | Shared portal Deployment |
| `runtime` | `asia-southeast1-docker.pkg.dev/chronoai-501608/chrono-pod/cma-runtime` | `CHRONO_SANDBOX_IMAGE` system default and rollout annotation |

The exact artifact names are
`ci-native-image-COMPONENT-SOURCE_REVISION-RUN_ID-RUN_ATTEMPT`. Obtain them from
the successful candidate run's artifact inventory, rather than constructing
`SOURCE_REVISION` from the PR head. Each contains `metadata.json` and
`cma-COMPONENT-linux-amd64.docker.tar.gz`; download each into its own directory
with `gh run download RUN_ID --repo ChronoAIProject/cma --name ARTIFACT_NAME
--dir COMPONENT_DIRECTORY`. Download the same run's
`runtime-image-evidence-RUN_ID-RUN_ATTEMPT` as well. Native image artifacts expire
after one day, so preserve verified bundles with the deployment evidence.

Before a registry write, require the selected release gate and confirm its head
matches the selected candidate. Full CI was required for cabfe; the scoped AM cd37968d gate required Backend,
Images(api), Repository structure, Secret scanning and Manifests; final Web ac837
required Images(web), Frontend, Repository structure and Secret scanning. Each scoped gate binds the exact current attempt and separately
records actual whole-run status. Check the artifact run ID/attempt, component,
repository, source revision/tree, Linux amd64 platform, archive size and SHA-256
against `metadata.json`. Inspect the Docker archive's config and require its
SHA-256 to equal `imageId`; verify the source/revision/version labels and both
runtime labels above. The runtime evidence must bind the same source revision
and immutable image ID and report passing execution checks. Artifacts within a selected
publication must agree on that candidate's verified merge revision and tree.
Each independently selected component publication has separate provenance. Confirm the reviewed follow-up file/blob scope and preserve
the other deployed components. Outside these explicitly authorized scoped gates, a failed full candidate
run is not a deployment source even if its selected image job succeeded; retained successful baseline images
remain identified by their own receipts.

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

For a new deployment, create `cma-agent-sandbox-control` and its `control-key`
using the approved secret workflow. Production already has this Secret; preserve
its existing value. Its mounted file is required when the native backend is
enabled; it must contain 32–8192 bytes. Do not copy a token into a ConfigMap,
runbook, PR or shell command. Keep the key stable across restarts so retained
runtime control tokens continue to match. The control-plane Secret volume is
optional only while the native backend is disabled.

When a verified runtime is intentionally replaced, set its digest in
`CHRONO_SANDBOX_IMAGE` and the matching rollout annotation. The current follow-up
selects the verified `a955564d` runtime digest for new allocations; retained
Sandboxes keep their recorded images. `CHRONO_AGENT_SANDBOX_RUNTIME_CLASS`, runtime
ServiceAccount, pull secrets, home policy and deployment ID must match the reviewed production
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
Gateway rollout. Keep Image Manager absent or at zero replicas until its
prerequisites are available. Its absence does not block the system-default runtime path.

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

Cloud Build is fixed to project `chronoai-501608`, location `asia-northeast1`
and destination
`asia-southeast1-docker.pkg.dev/chronoai-501608/chrono-pod`. Enable Cloud
Build, Artifact Registry and Cloud Logging APIs. Before scaling Image Manager:

1. Bind KSA `chrono-bot/cma-image-manager` to
   `cma-image-manager@chronoai-501608.iam.gserviceaccount.com` through GKE Workload
   Identity. The API obtains short-lived access through the metadata service.
2. Grant that same identity Cloud Build create/get/list, Cloud Logging log-entry
   read permissions and `iam.serviceAccounts.actAs` on itself. Use custom roles or
   the corresponding reviewed predefined roles.
3. Grant that same identity Artifact Registry writer on only `chrono-pod` and
   Cloud Logging writer. Image Manager submits and reconciles the job while the
   Cloud Build runner executes the Docker steps under this identity; do not give
   it application or production-secret access. Check the Cloud Build service
   agent's required service-account token permissions for that identity.
4. Set `CMA_IM_BUILD_SERVICE_ACCOUNT` to
   `projects/chronoai-501608/serviceAccounts/cma-image-manager@chronoai-501608.iam.gserviceaccount.com`
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

The production native path is enabled. Preserve these provider and transport
settings during application updates:

```text
CHRONO_SANDBOX_PROVIDER=agent-sandbox
CHRONO_RETAIN_OPENSANDBOX=true
CHRONO_AGENT_SANDBOX_ENABLED=true
CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT=kubernetes-exec
```

For a future runtime change, select the new default only after exact publication,
residue scan and native canary pass, and apply that default before restarting
its consuming Agent Manager. Keep AM at one replica with Recreate strategy.
Web/Gateway-only changes preserve the runtime configuration and AM Pod. Retained
Sandboxes keep their recorded images and lifecycle authority.

Using the reviewed native namespace and remote principal, set `NATIVE_NAMESPACE`
and `NATIVE_PRINCIPAL` to those nonsecret identities
and run from the repository root:

```sh
node .github/scripts/agent-sandbox-preflight.mjs \
  --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke \
  --namespace chrono-bot \
  --service-account chrono-bot-ksa \
  --runtime-context gke_chronoai-501608_asia-southeast1-b_chronoai-prod-asia-southeast1-gke \
  --runtime-namespace "${NATIVE_NAMESPACE:?set the infra-assigned native namespace}" \
  --runtime-principal "${NATIVE_PRINCIPAL:?set the native client's Kubernetes principal}"
```

This script reads `cma-agent-manager` configuration and Deployment overrides
only from the CMA context, then checks native API
schemas, the configured runtime ServiceAccount's existence and non-deletion,
the selected RuntimeClass and the known southeast `chrono-pod` class requirement,
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
`chrono-cma@chronoai-501608.iam.gserviceaccount.com`. A permission result for an
impersonated principal does not prove the deployed client represents it; verify
actual authenticated requests from CMA.
Each API command has a 10-second server request bound and every command has a
15-second local timeout. A passing result does not prove mounted credential
identity, controller image provenance, OCI runtime compatibility, network
enforcement or live provisioning.

Execute only the selected, verified application patches and record actual Pod
image IDs, readiness and retained-resource continuity. Keep native and retained
OpenSandbox support enabled while their resources exist. Agent Manager remains
one Recreate replica because process admission quotas lack shared coordination.
Warm-pool acceptance starts with one member of the selected immutable runtime
and must preserve claimed members when replenishment changes.

## Deployment acceptance and coverage

The [verification report](CMA_Native_Production_Verification.md) is the measured
acceptance record, including exact tested versions, final cold result, normal
cleanup, protected-resource preservation and relevant historical failures.
The checklist below remains the wider acceptance contract for future changes;
it is not a claim that every feature received production coverage.

Deferred coverage includes Image Manager/Cloud Build, fresh missing-service
OAuth, NyxID frontend navigation/binding-key delivery, external reply sinks,
broad failover, full Console/media/STT, Trigger and cross-user sharing. Native
exec connectivity does not establish direct Pod-network isolation.

The repository image pins and source/CI annotations are now synchronized with
the actual deployed receipts. The four synchronized manifests are
`infra/api/configmap.yaml`, `infra/api/deployment.yaml`,
`infra/cmaeg/deployment.yaml` and `infra/web/deployment.yaml`.
The applied patch is `/tmp/cma-final-repo-sync-01/final-manifests.patch`. Their current hashes match
`/tmp/cma-final-repo-sync-01/resolved-receipts.json` after hashes. This pins AM
cd37968d, Web ac837, Gateway cabfe and default runtime 37d with their actual image
sources and CI runs. YAML, configuration key/documentation, native contract and
route ownership checks passed. The legacy storage seed example and deferred
Image Manager are preserved. This manifest/document finish does not redefine or
rebuild the independently identified deployed application images.

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

Record each selected component's candidate, actual image source revision/tree,
full CI run and registry manifest digest, together with controller provenance,
native API preflight, home mode and Claim/Sandbox/Pod/PVC UIDs. Record a seed UID
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
- Verify runtime Pods have no Kubernetes API token and the selected exec
  transport authenticates operations against the exact owned Pod. Record any
  direct-network isolation test with its own scope; exec connectivity does not
  demonstrate Pod-network isolation. Retire owned acceptance resources and
  record exact Claim/Sandbox/Pod/PVC cleanup without affecting user resources.

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

The configured template-policy value remains empty. Native controllers retain
network policy authority. The selected exec transport does not require direct
northeast-to-runtime TCP ingress. A future direct-TCP transport must independently
prove authenticated access with the actual runtime image and controller-owned
policy, including DNS and approved egress. A source CIDR does not establish CMA
identity. The following diagnostic is historical and does not block exec.

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
`10.41.0.0/16` on TCP 9090. After the same generated NetworkPolicy advanced to
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
read because the diagnostic account lacked `compute.firewalls.list` and
`compute.instances.get`; its narrow firewall-flow log query also lacked access.
Missing cross-cluster firewall rules were therefore unconfirmed. Any future
direct-TCP transport would require infra to verify effective ingress and egress.
This historical diagnostic does not block the selected exec transport or
authorize a networking change.

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
