# C-001 deployment handoff

The three portals ship from one web image: Agent Manager (`/agent-manager`),
Image Manager (`/image-manager`) and Event Gateway (`/event-gateway`). Their
backends remain separate services with their own collection schemas. This handoff prepares
application deployment against the infra-owned native runtime cluster. It does
not record a successful production rollout or authorize changes to that cluster's
controllers, RBAC, storage or network policy.

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

## Observed deployment prerequisites

These observations were recorded on 2026-10-13. They distinguish current access
from the remaining application activation steps.

| Prerequisite | Observed state | Remaining work |
|---|---|---|
| Native Kubernetes authentication | Actual CMA Pod received the `chrono-cma@chronoai-501608.iam.gserviceaccount.com` metadata identity; Sandbox and claim reads returned HTTP 200 with the remote CA verified | Exercise the application token refresh path and native lifecycle after required grants are present |
| Native target | `chrono-bot/chrono-pod-configmap` currently supplies `https://35.240.157.32` and namespace `chrono-pod` | Infra confirms this formal workload target; old last-applied annotations are not authoritative |
| Native authorization | Remote Role `chrono-cma` permits native resource operations but lacks PVC `create`/`patch`, `pods/exec` `get` and VolumeSnapshot `get` | Infra adds the required namespaced permissions and the explicit-principal preflight passes |
| Native retained-home seed | No seed snapshot was observed in the intended remote target during the earlier inspection | Infra supplies an initialized ready format-1 seed, its exact UID, StorageClass and capacity; configure the four native home overrides |
| Shared MongoDB | Existing `chrono-bot` credentials have been provisioned through the current application Secrets | Copy those values to the canonical Secret names, select each service's shared-schema mode and run only its own migration/startup initialization |
| Existing Event Gateway feature settings | Live `cmaeg` has Console and Media enabled, `cma_codex` non-human/media providers and `gpt-transcribe` STT settings, with optional STT/console Secret refs and a 32 MiB in-memory media volume | The current source candidate has no corresponding Console, Media or STT configuration modules. Keep the old Gateway deployment and Secrets intact; do not activate the canonical replacement until those feature settings are implemented and their behavior is accepted, or an explicit retirement decision is recorded |
| Image Manager build identity | Lookups for the single `cma-image-manager` Google service account returned 404; current operator IAM tests did not grant account creation, policy changes or Cloud Build submission | Infra provisions that one identity and grants it the Image Manager API, Cloud Build, logging and registry permissions described below before builds are activated |
| Application image registry | Upload/download authority was observed on `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry` | Import the tested release images, verify the immutable digests and update application manifests before deployment |

## C-001 implementation map

The Journey Book is the audit baseline; the current implementation follows the
requested C-001 changes below. Native lifecycle, Cloud Build and OAuth behavior
still require the deployment acceptance in this document.

| User flows | Prepared application behavior |
|---|---|
| UF-01, UF-13, UF-19, UF-20 | Agent Manager landing with three service descriptions and shared service navigation, image plus inline Agent settings, durable three-stage provisioning and a NyxID connection modal |
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

CMA relies on the upstream `agents.x-k8s.io/sandbox-name-hash` Pod label for
discovery and verifies the Sandbox owner UID separately. Home adoption converts
the claimed Sandbox from a volume template to an explicit mount of the same
PVC, waits for the controller to observe that generation, then removes the PVC
owner reference. Verify this conversion preserves the existing Pod. Removing
the owner reference alone is unsafe because upstream can adopt the PVC again.

## CMA identities, storage and network

The Agent Manager ServiceAccount remains `chrono-bot/chrono-bot-ksa` in the
northeast application cluster. `infra/api/serviceaccount.yaml` declares its
local Role/RoleBinding and the same-cluster native policy. Those local grants
do not confer authority in the remote cluster. Infra must provide a separate
remote identity and namespaced authorization in the assigned native namespace.
Validate the actual remote principal's grants; do not apply the northeast
application manifests to southeast as a substitute. The native path requires:

| API/resource | Required verbs | Purpose |
|---|---|---|
| `agents.x-k8s.io/sandboxes` | get, list, create, patch, delete | Provision, observe, suspend, resume and retire |
| `extensions.agents.x-k8s.io/{sandboxtemplates,sandboxwarmpools,sandboxclaims}` | get, list, create, patch, delete | Native pool administration and claims |
| core `persistentvolumeclaims` | get, list, create, patch, delete | Retained homes, original-PVC adoption and cleanup |
| core `pods` | get, list | Verify exact Pod identity and deletion finality |
| core `pods/exec` | get, create | kube-rs WebSocket assignment over stdin |
| `snapshot.storage.k8s.io/volumesnapshots` | get | Verify the pinned initialized seed |

The checked-in local Role retains permissions needed by previous OpenSandbox and
storage cleanup paths. Native controllers own runtime Pod creation/deletion;
runtime workloads use `CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT` (default
`cma-runtime`) in the remote namespace with automatic API tokens disabled. Registry
pull identity needs reader access to each selected immutable image; this is
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
The policy intentionally does not rely on `cma/runtime-name`, which claimed
warm Pods do not carry. Workload network egress and gVisor scheduling remain
infra policy; check the selected RuntimeClass can schedule the requested image,
CPU/memory and disk in the target node pool.

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

The inspected remote Role still lacks PVC `create`/`patch`, VolumeSnapshot
`get`, and Pod exec `get` permissions required by the table above. Infra must
complete those grants and confirm the formal namespace before native workload
creation. These successful authenticated reads do not prove allocation,
assignment, remote runtime connectivity or persistent-home acceptance.

## Build and configure all application images

Publish the tested `api`, `web`, `runtime`, `cmaeg` and `image-manager` images
from the same reviewed source revision using the existing release workflow.
Transfer them to the approved registry if required by the cluster. Pin the final
registry digests in the release manifests; Image Manager currently has
`replicas: 0` and an `:unreleased` placeholder, which must be replaced before
activation. The publication workflow does not deploy to Kubernetes.

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

## Image Manager / Cloud Build activation

For the requested production deployment, select the existing `chrono-bot`
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
  --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke \
  --namespace chrono-bot \
  --service-account chrono-bot-ksa \
  --runtime-context gke_chronoai-501608_asia-southeast1-b_chronoai-prod-asia-southeast1-gke \
  --runtime-namespace "${NATIVE_NAMESPACE:?set the infra-assigned native namespace}" \
  --runtime-principal "${NATIVE_PRINCIPAL:?set the native client's Kubernetes principal}"
```

This script reads `cma-agent-manager` configuration and Deployment overrides
only from the CMA context, then checks native API
schemas, ServiceAccount token isolation, the effective seed and storage class
in the runtime context. It checks the configured native namespace and remote
API origin against the selected runtime cluster and performs namespaced
authorization checks as the explicit remote principal. It does not read Secrets
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

Then deploy the application release with the native backend enabled and retained
OpenSandbox support still enabled. Confirm existing runtime recovery before
selecting `CHRONO_SANDBOX_PROVIDER=agent-sandbox` for new allocations. Keep only
one Agent Manager replica and its existing Recreate rollout strategy; process
admission quotas do not yet have shared coordination. Native warm-pool
administration should start with one member of the verified immutable runtime
image and grow only after the checks below pass.

## Deployment acceptance still required

Record source SHA, five final image digests, controller digest/revision, native
API preflight, seed UID and test resource identities. Execute these checks after
deployment; local fixtures do not substitute for them:

- Sign into each portal with its own NyxID session. Verify the unified dark UI,
  service navigation, connection popup, private/public image selection and
  permission denial for an unconnected or unauthorized service.
- Prove direct authenticated native API access from the northeast CMA Pod into
  the assigned southeast namespace, with CA validation and credential rotation.
  Record both cluster contexts and verify native CRD/Pod/PVC creation remains in
  southeast while retained OpenSandbox recovery and cleanup remain in northeast.
- Run both a repository Dockerfile build and an imported Dockerfile build.
  Observe running/success, owner-only bounded logs, exact Artifact Registry
  digest, authorized resolution, private/share/public visibility and a failed
  build that never publishes a version. Check unknown-response recovery without
  duplicate Cloud Build submissions.
- Create an Agent with default image and with an authorized custom image;
  verify the three stages (Sandbox, Agent/workspace, scoped key), exact CRD/Pod/
  PVC identities, GitHub workspace, skills/model/reasoning/prompt/permission
  configuration, and a real turn. Export JSONL from the Agent menu.
- Add empty and GitHub workspaces to that Sandbox, then create an Agent from
  an existing Workspace. Verify the original Sandbox/Workspace identities and
  inherited service scope, with no new Sandbox allocation.
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
