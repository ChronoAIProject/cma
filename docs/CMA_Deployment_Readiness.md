# C-001 deployment handoff

The three portals ship from one web image: Agent Manager (`/agent-manager`),
Image Manager (`/image-manager`) and Event Gateway (`/event-gateway`). Their
backends remain separate services and database identities. This handoff prepares
deployment after infra installs agent-sandbox. It does not record a successful
production rollout or authorize installation of a controller.

The deployment context is
`gke_example-project-02_asia-northeast1_example-cluster-02`, namespace
`chrono-bot`, CMA deployment ID `chrono-bot-prod`. Always pass that context
explicitly. Existing production OpenSandbox resources retain their recorded
provider bindings until normal retirement.

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

CMA's native integration was implemented against
[`kubernetes-sigs/agent-sandbox` at `86ad94178a85aa26788077a55894cdc234c5ea6a`](https://github.com/kubernetes-sigs/agent-sandbox/tree/86ad94178a85aa26788077a55894cdc234c5ea6a).
Install the matching core and extension controllers and CRDs together. A
controller image tag or the existence of only the core Sandbox CRD is
insufficient. Record the installed immutable controller image digest and its
source revision in the rollout evidence; this repository does not guess an
unpublished upstream image digest.

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

Apply the reviewed namespaced Role/RoleBinding from
`infra/api/serviceaccount.yaml` with the application release. The Agent Manager
ServiceAccount is `chrono-bot-ksa`. The native path requires:

| API/resource | Required verbs | Purpose |
|---|---|---|
| `agents.x-k8s.io/sandboxes` | get, list, create, patch, delete | Provision, observe, suspend, resume and retire |
| `extensions.agents.x-k8s.io/{sandboxtemplates,sandboxwarmpools,sandboxclaims}` | get, list, create, patch, delete | Native pool administration and claims |
| core `persistentvolumeclaims` | get, list, create, patch, delete | Retained homes, original-PVC adoption and cleanup |
| core `pods` | get, list | Verify exact Pod identity and deletion finality |
| core `pods/exec` | get, create | kube-rs WebSocket assignment over stdin |
| `snapshot.storage.k8s.io/volumesnapshots` | get | Verify the pinned initialized seed |

The checked-in Role also retains permissions needed by previous OpenSandbox and
storage cleanup paths. Native controllers own runtime Pod creation/deletion;
runtime workloads use `cma-runtime` with automatic API tokens disabled. Registry
pull identity needs reader access to each selected immutable image; this is
separate from Image Manager's build identity. Configure
`CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS` only if the node's registry identity
does not supply that access. Neither registry nor Kubernetes credentials belong
in an Agent's environment.

`infra/api/serviceaccount.yaml` restricts ingress to every native CMA runtime
Pod, including unclaimed warm Pods, by the stable control-plane and upstream
Sandbox tracking labels. The tracking selector excludes retained OpenSandbox
Pods. Only
Agent Manager Pods in the namespace may connect to TCP 9090. Verify the cluster
CNI enforces NetworkPolicy and that an unrelated Pod cannot reach this port.
The policy intentionally does not rely on `cma/runtime-name`, which claimed
warm Pods do not carry. Workload network egress and gVisor scheduling remain
infra policy; check the selected RuntimeClass can schedule the requested image,
CPU/memory and disk in the target node pool.

Keep `CHRONO_SANDBOX_HOME_*` pinned to a ready, format-1 initialized
VolumeSnapshot and its exact Kubernetes UID. The checked-in seed manifests show
the layout (`home`, `state`, `transfer`) and are operator initialization inputs,
not per-Agent backup scheduling. Use CSI support for `ReadWriteOncePod` and
restoring the seed to a fresh PVC. Each pool member has its own disk. Never
reinitialize or replace a seed name/UID that retained allocations reference.
The existing seed and runtime pins are historical production pins, not evidence
that the newly built bootstrap runtime has been deployed.

## Build and configure all application images

Publish the tested `api`, `web`, `runtime`, `cmaeg` and `image-manager` images
from the same reviewed source revision using the existing release workflow.
Transfer them to the approved registry if required by the cluster. Pin the final
registry digests in the release manifests; Image Manager currently has
`replicas: 0` and an `:unreleased` placeholder, which must be replaced before
activation. The publication workflow does not deploy to Kubernetes.

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

Before upgrading Agent Manager, regenerate its collection-scoped role with the
new `cma --print-runtime-role` binary. It includes `native_warm_pool_targets` for
durable pool plans, revisions, leases and original resource bindings. The role's
create/index privileges initialize that collection; Agent Manager has no
separate migration role. Existing credentials missing these grants fail startup.

## Image Manager / Cloud Build activation

Use the `cma_image_manager` database with separate runtime and migrator MongoDB
credentials. Run the released `cma-image-manager --migrate` with the migrator
identity before starting the service; schema marker 2 is required. Runtime
startup verifies exact collection grants and refuses broader shared database
credentials. Use `--print-runtime-role` and `--print-migration-role` to generate
the collection-scoped role definitions without printing credentials.

Provision the dedicated NyxID client and browser cookie key described in
`infra/image-manager/secret.yaml`. Register the callback at
`https://bot.chrono-ai.fun/api/image-manager/auth/callback`, the service resource
`cma-image-manager`, and the documented `cma-im:` permissions. Event Gateway and
Agent Manager keep their own clients and session keys; see
[service isolation](CMA_Service_Isolation.md).

Cloud Build is fixed to project `example-project-02`, location `asia-northeast1`
and destination
`asia-northeast1-docker.pkg.dev/example-project-02/chronoai-registry`. Enable Cloud
Build, Artifact Registry and Cloud Logging APIs. Before scaling Image Manager:

1. Bind KSA `chrono-bot/cma-image-manager` to
   `cma-image-manager@example-project-02.iam.gserviceaccount.com` through GKE Workload
   Identity. The API obtains short-lived access through the metadata service.
2. Grant that API identity Cloud Build create/get/list, Cloud Logging log-entry
   read permissions, and `iam.serviceAccounts.actAs` on only the selected build
   identity. Use custom roles or the corresponding reviewed predefined roles.
3. Give build identity
   `cma-image-build@example-project-02.iam.gserviceaccount.com` Artifact Registry
   writer on only `chronoai-registry` and Cloud Logging writer. User Dockerfiles
   execute under this identity; do not give it application or production-secret
   access. Check the Cloud Build service agent's required service-account token
   permissions for the project's configured build identity.
4. Set `CMA_IM_BUILD_SERVICE_ACCOUNT` to its fully qualified resource name and
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

After infra installs the native APIs, run from the repository root:

```sh
node .github/scripts/agent-sandbox-preflight.mjs \
  --context gke_example-project-02_asia-northeast1_example-cluster-02 \
  --namespace chrono-bot \
  --service-account chrono-bot-ksa
```

This script only reads API schemas, ConfigMap storage settings, ServiceAccounts
and storage resources, plus namespaced authorization checks. It does not read
Secrets or modify workloads. It returns nonzero on missing/incompatible CRDs,
denied rights, a replaced/unready seed or missing storage prerequisite. The
caller must be allowed to impersonate the application ServiceAccount for the
authorization checks. Each command has a 10-second server request bound and a
15-second local timeout. A passing result does not prove controller image
provenance, OCI runtime compatibility, network enforcement or live provisioning.

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
