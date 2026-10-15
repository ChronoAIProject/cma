# C-001 deployment handoff

For exact deployed digests, CI/image provenance, measured flows, historical
failures and receipt paths, see
[CMA native Sandbox production verification](CMA_Native_Production_Verification.md).
This document keeps operational instructions and dated infra diagnostics.

The current deployment is recorded in the linked verification document and
immutable manifests. On 15 October at 21:03 SGT, Web `0785d328` (generation 119)
is live with smooth route/message/menu transitions and immediate visible draft
handoff. Agent Manager remains `d21ba8aa` (generation 41) while the new captured
GitHub API instructions are being built. The default runtime is `d97121b5`;
existing owner runtimes keep their pinned images and full-home volumes. The
accepted repository workflow is authenticated `gh api` source download plus a
local Git workspace, independently verified on a production testing Agent.
Image Manager remains deferred.

The following 18:51 checkpoint is historical. At **18:51 SGT on 15 October 2026**,
production ran Agent Manager
`edaeca82` (generation 38), Web `fa1e3f13` (generation 116), and Event Gateway
`477bf801` (generation 7). All three exact image digests and Ready Pod ownership
were verified. The default runtime is `ff9834c4`, digest
`sha256:b75343d07995b93adb792857fb6a6cc43dff4e3a94b8468becfe41e4a7c11eae`.
The manifests now pin this observed deployment. Image Manager remains deferred;
CMA remains in Tokyo and the native runtime remains in its separate cluster.

This release adds live key replacement and optional service revocation with a
confirmed callback to the original Agent. Runtime GitHub transport is prepared
before the first grant. Web adds sender identities, compact/upward-expanding
drafts and explicitly approximate live output tokens alongside actual usage.
Attachment upload, quieter reconnection, fast message admission and the shared
Event Gateway browser login remain active. Public Web assets match the running
Pod bytes. The combined Web suite passed 205 files / 1,932 tests, type checking,
lint, formatting and build.

The final Agent Manager patch releases an obsolete scope request when an owner
pause or runtime change invalidates a callback that was never written. A fenced
transaction proves the absence or pre-write failure before ending that request;
already admitted or uncertain callbacks retain their exact command and recovery.
This prevents a failed notification from indefinitely blocking a later grant or
revocation. Eleven focused tests and 34 authenticated MongoDB contracts passed,
including later revocation and uncertain-delivery regressions. The exact published
image passed vulnerability scanning and production readiness checks.

Full-home runtime and native Linux attachment upload/replay/recreation proofs
passed before activation. Existing Sandboxes retain their recorded runtime
images and original volumes. New allocations use the new default policy; its
CMA-created warm target had one observed Ready replica. Whole-`/home` mounting
and `OnRootMismatch` remain in the policy. Actual production browser creation
reached an enabled composer in 3.373 seconds; this excludes first model response
completion. Pasted images and dragged files were consumed by the Agent, with a
381 ms message receipt after attachment upload. Actual runtime pause/resume
still took about 9 seconds / 21–26 seconds despite prompt admission, so the
complete five-second lifecycle goal is **not accepted**. Fresh external OAuth
consent remains unmeasured.

Production hot key updates on final Agent Manager `edaeca82` passed quiet and
busy-Agent callbacks in 2.152 and 1.804 seconds respectively, with key delivery
and old-key revocation completed first. Actual optional GitHub grant/private
repository read/removal on the preceding `8cad5bcd` also
passed, with callbacks at 3.399 and 1.844 seconds and the original Pod, boot,
conversation and PVC/PV retained. The owner's existing GitHub OAuth connection
was used. Native Git clone/push is separate from this verified GitHub API path.
After these hot writes, every rollback reader must support the new scope fields,
events and persistent callback intent described below, as well as attachment
capability and whole-home policy records. `26438942` alone is no longer a valid
rollback floor. Retain the compatible `8cad5bcd` implementation; changing only
the default runtime image cannot make an older reader compatible.

The following checkpoints are historical and do not describe the current pins.

At 12:26 SGT, API `6571e110` remains deployed. Web `53bbe2f7` replaced
`47b56be7` at 12:25:35 SGT with verified public assets. Live Stop, exact interruption
and one completed follow-up now pass; original-volume resume also passes identity
checks, with 505 ms acceptance and 26.353 s actual readiness. The first API6571
creation sample completed functionally but exceeded five seconds (5.224 s to
source-flow Send eligibility). Fresh optimized-Web timing and actual browser
acceptance remain pending. The following 12:16 snapshot is historical.

PR #1027 is being tested through authorized local production builds. As of
12:16 SGT on 2026-10-15, Agent Manager source `6571e110` and Web source
`47b56be7` are deployed in the existing CMA cluster. The combined API image
`sha256:99044a2876b0919c680118d55e57883571b208bcffacd69aad689d6a932ea98d`
was published after Google authentication was restored. Generation 31 and its
exact imageID were Ready at 12:16:26 SGT; authenticated owner access and retained
fixture identities, settings and history passed continuity checks. Branch head
`0c16330e` passed all 15 CI jobs; it is separate from the deployed image source.
Runtime `cad2058`, Event Gateway and the shared Web image remain unchanged,
and Image Manager remains undeployed. The Deployment keeps prepared target one
and singleton/Recreate operation. Strict kubeconform validated all 48 resources
without skips; kube-linter and HTTPRoute ownership checks passed.

The API748 optional-GitHub existing-Sandbox diagnostic passed, and the 12:16
read-only follow-up confirmed the original Claim, Sandbox, PVC and PV identities
and full-home template mount. Two retired test fixtures also passed exact native
resource absence checks. These diagnostic and cleanup results do not establish
final API6571 acceptance. Browser/model, fresh warm creation and live Stop
verification are still pending. The five-second creation goal is not yet
consistently met; source-ordered
measurements are not browser measurements. Current candidate evidence and
remaining acceptance gaps are recorded in the
[PR #1027 verification](CMA_Native_Production_Verification.md#local-production-candidates--pr-1027).
The compatible rollback floor below applies to these production records.

The preceding merged production deployment is PR #1022, candidate `d9a6d440`,
merged as `4dc1325f`, with all 15 CI jobs passed. Agent Manager/Web were Ready
at 23:13:31 SGT on 2026-10-14. Warm creation reached the exact Agent surface
in 10.179 seconds and enabled Send in 11.696 seconds, so the strict five-second
creation target remains unmet. Actual early input was durably accepted and
executed once. Pause and resume showed accepted UI state in 0.184 and 1.531
seconds respectively; actual Sandbox completion took 9.109 and 24.422 seconds.
Whole-home contents, modes, ownership, links, dirty Git, the original PVC and
original conversation survived the replacement Pod. Model selection, skill use
and MCP access revision with one automatic notification passed. Fresh external
OAuth remains unexercised; these results do not claim coverage of that path.

The earlier production deployment PR #1019, candidate `3d6885c6`,
merged as `68dbe4b6`, with all 15 CI jobs passed. Agent Manager/Web were Ready
at approximately 20:26 SGT on 2026-10-14. Strict full-home preservation now
passes, including modes, ownership, links, dirty Git, the original PVC and
conversation. Warm creation still took 17.306 seconds and usable resume took
21.649 seconds; the five-second performance bar remains unmet. Immediate
delete feedback/disappearance passed. A transient authorization-check outage
also exposed a UI continuity defect. Exact observations and the next fixes are
recorded in the [PR #1019 verification](CMA_Native_Production_Verification.md#full-home-preservation-and-lifecycle-performance--pr-1019).

The combined performance and full-home candidate `04ccbd6a` merged through
PR #1016 as `fea97549`. CI run
[38539408542](https://github.com/ChronoAIProject/cma/actions/runs/38539408542),
attempt 1, passed all 15 jobs. Exact API/Web/runtime image publication is
complete and the checked-in pins match its receipt. AM/Web and retained Gateway
were Ready at **18:02:50 SGT**, and the new full-home pool reached two observed
Ready replicas at **18:04:03 SGT**. One fresh UI creation succeeded in **21.122
seconds** to the visible Agent surface/enabled composer; the five-second target
was not met. Model, skill, scope revision and Stop acceptance subsequently
passed. Pause/resume retained the exact PVC and conversation, but actual
completion exceeded five seconds and kubelet changed filesystem modes on
remount; strict whole-home persistence therefore failed. Exact digests, identities
and measured boundaries are in the
[combined release report](CMA_Native_Production_Verification.md#combined-performance-and-full-home-release--pr-1016).

The follow-up source introduces a separately identified native pool policy with
`fsGroupChangePolicy: OnRootMismatch`. Existing pool/template/home records keep
their original policy; activate fresh capacity only when its `config_sha256`
exactly equals the non-null administration `default_config_sha256`. Matching
only image and resources can select an old policy and is insufficient. PR #1019
proved fresh whole-home mode preservation; subsequent releases still require
their own live continuity and timing verification.

After a new-policy pool or binding is persisted, any API rollback must retain
support for the optional `fs_group_change_policy` field and v3 pool identity.
Older binaries reject that persisted field through strict deserialization; do
not roll Agent Manager back to the pre-policy image after activation. Runtime
`cad2058` and the current full-home layout remain compatible and unchanged.

The next durable bootstrap admission persists `wait_for_workspace` in Agent
work and immutable Agent event logs. After the first such admission, **do not
restore the PR #1022 API image**: its strict enum reader rejects both waiting
Agents and retained events, even after those Agents become Ready. Recovery
requires a compatible forward repair that disables new early lifecycle
reservation while retaining the new enum, event readers, workspace binding,
waiting worker and cleanup support for previously admitted work. Keep all
message receipts and logs intact; do not rewrite or delete records to make an
older image parse them. Runtime and full-home storage are unchanged by this
API/Web-only candidate. Capture retained workload and conversation evidence
before and after any repair, as for an upgrade.

The per-turn model correction in `c8ee3872` adds optional `model` evidence to
durable `TurnStarted` events. The value is captured from the selected Agent
settings in the same fenced transaction that admits the turn; later settings
changes cannot rewrite a completed response's model. The new reader accepts
older events without this field. Older API binaries use strict event
deserialization and reject a new event containing `model`, so compatibility is
one-way: **after the first model-bearing turn is committed, every Agent Manager
reader and rollback image must include `c8ee3872` or an equivalent compatible
reader, as well as all preceding durable-format support**. A pre-`c8ee3872` image
is no longer a valid rollback even after that turn completes or its Agent stops.

Before allowing the new writer to serve traffic, retain an immutable compatible
API image digest and record its source revision and readiness checks in the
rollout evidence. For the next combined performance/lifecycle candidate, the
standalone `c8ee3872` API build is the compatibility floor; keep its digest as the
rollback target instead of the previously deployed API digest. Complete the
transition of old Agent Manager readers before accepting new model-bearing
turns; do not leave an old replica serving retained histories during a mixed
rollout. A rollback changes the application image only and must preserve the
stored events, message receipts, original PVCs and conversations. Verify both a
retained model-bearing turn and a new turn after recovery. Do not remove the
field, rewrite history, or relax MongoDB validation to enable an older image.
This field does not change the runtime protocol or whole-home storage layout.

The preceding `03cbd332` Agent Manager/shared Web release was Ready at
16:26:14 SGT on 2026-10-14, with two Ready default-pool replicas observed at
16:26:50 SGT. Its fresh Agent acceptance was not completed. The earlier
cd37968d/ac837/cabfe Gateway flow passed in 42.624 seconds; that result remains
associated with its original tested versions. Image Manager activation is deferred.

The three portals ship from one web image: Agent Manager (`/agent-manager`),
Image Manager (`/image-manager`) and Event Gateway (`/event-gateway`). Their
backends remain separate services with their own collection schemas. This handoff
defines application deployment against the infra-owned native runtime cluster.
Infra continues to own its controllers, CRDs, RBAC, storage and network policy;
CMA's completed application rollout did not change those resources.

The combined rollout scope is Agent Manager, the shared web image and the
system default runtime, retaining the verified Event Gateway. Image Manager is
deferred: its Deployment is absent; keep it absent or at zero replicas and do
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

This table records native deployment prerequisites and measurements from the
versions named on 2026-10-14. The combined release status above is tracked
separately; earlier runtime and portal measurements do not verify its full-home
layout or creation latency.

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
| Canonical application workloads | Combined 04ccbd6a AM generation 16/Web generation 92 and retained cabfe Gateway generation 6 were Ready at 18:02:50 SGT with exact imageIDs and zero restarts | Keep one Recreate Agent Manager and preserve canonical/compatibility Service routing; record exact candidate/imageIDs for the combined rollout |
| Shared MongoDB | Running Agent Manager/Gateway use `chrono-bot` with separate collection ownership; retained durable identities and complete history prefixes survived the AM rollout | Run only each service's migrations. Shared-schema mode is collection ownership, not credential-level isolation |
| Retained Gateway state | Gateway uses `eg_*` collections and preserves exact `http://cma-api.chrono-bot.svc` authority. Earlier real owner edits, selected-Agent management tests and public custom ingress passed | Do not repeat the historical first-cutover export/import. Preserve original record identities and sealed pointers; final Gateway/browser-specific acceptance remains separately recorded |
| Gateway cold inline-Agent acceptance | Final measured result and the earlier 5ce/AM 37d failures are recorded in the verification report | Preserve the 90-second startup and fresh 30-second acceptance/completion budgets; only explicit Busy may retry the same body/key |
| Gateway feature coverage | Integrated source and complete CI include Console/Media/STT code and authenticated Mongo contracts | Selected live routing/channel/model checks do not establish full Console/media/STT, external reply sinks or Trigger acceptance; preserve existing configuration and Secrets |
| Application image registry | Combined candidate 04ccbd6a passed all 15 CI jobs; API/Web registry digests are published and pinned from the completed receipt | Preserve Gateway cabfe; match immutable pins and provenance to publication and live readiness independently |
| Native runtime registry | Combined cad2058f runtime is published and selected; two full-home v2 pool replicas were Ready at 18:04:03 SGT with exact runtime/mount/initializer proof | Live same-PVC whole-home persistence still needs separate acceptance; retained homes keep their original images |
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

Reaching the owner's Sandbox quota ends a new Agent lifecycle operation with an
explicit limit conflict; it must not remain queued as a retryable dependency
failure. Paused Sandboxes still retain storage and count toward this quota.
Retiring disposable acceptance Sandboxes releases their slot after cleanup;
prepared capacity alone does not bypass owner admission limits.

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
`CHRONO_AGENT_SANDBOX_HOME_MODE=native_vct_full_home_v2`,
`CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS`, and
`CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB` (1–1024 GiB). Leave
`CHRONO_AGENT_SANDBOX_HOME_TEMPLATE` and
`CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID` empty. A complete native storage class
and capacity pair without snapshot fields also selects this mode. An explicit
mode may inherit storage class/capacity from the legacy configuration, but using
an explicit southeast storage pair makes the cluster boundary clear.

The native Sandbox controller creates one PVC per Sandbox from the volume claim
template. The pinned runtime image supplies the `--initialize-home-tree` init
command, which mounts the PVC root as UID/GID 10001. It seeds the public user
home into `home/cma`, creates writable `home/.codex`, and initializes separate
`state` and `transfer` directories before the runtime starts. The runtime mounts
the PVC's `home` at **`/home`**, preserving all contents including `/home/.codex`;
its `HOME` and working directory remain `/home/cma`. Runtime state and transfers
remain mounted at `/var/lib/cma-runtime` and `/var/lib/cma-transfer`. The durable
`.cma-home-layout-v2` completion marker makes Pod recreation safe without
re-seeding or modifying retained owner contents. The runtime receives credentials only after CMA
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
initializer as well as runtime pool assignment support before rollout. V2 pool
admission requires image metadata `dev.chrono.cma.home-layout=2`. CMA declares
this capability for its deployment-owned runtime pin; the runtime image evidence
probe checks the actual OCI label and executes both initializer versions. Roll
out the new runtime pin, CMA binary and v2 default configuration together.

Retained `native_vct_empty_v1` homes keep the original `home` → `/home/cma`
mount and `--initialize-home` initializer with `.cma-home-layout-v1`. Their
policy serialization, request digests, pool identities and native authority
scope remain stable. V2 uses a distinct persisted policy and pool identity;
changing the default never migrates or reinterprets an existing PVC. Either
initializer refuses the other version's completion or staging marker before
changing the volume. The historical native authority scope still identifies
the same cluster/namespace/deployment; the persisted policy identifies layout.
To roll back new allocation defaults, explicitly select `native_vct_empty_v1`
while retaining a CMA version that understands existing v2 records. Do not
roll back the runtime image pin of a retained v2 pool to a v1-only image.
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
`CHRONO_SANDBOX_IMAGE` and the matching rollout annotation. For this combined
release, update the runtime pin and
`CHRONO_AGENT_SANDBOX_HOME_MODE=native_vct_full_home_v2` atomically before the
Agent Manager restart. Retained Sandboxes keep their recorded image and home
mode. `CHRONO_AGENT_SANDBOX_RUNTIME_CLASS`, runtime ServiceAccount, pull secrets,
home policy and deployment ID must match the reviewed production
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

### Prepared default Agent hierarchy

The native pool can additionally prepare one or two complete default hierarchies
using `CHRONO_NATIVE_PREPARED_TARGET=1` or `2`. The checked-in ConfigMap keeps a
safe default of `0`; the production API Deployment explicitly overrides it to
`1`, while environments without that Deployment override remain disabled.
Enable this after a matching native pool is ready, with the same pinned runtime
image, CPU/memory and `native_vct_full_home_v2` home policy. Preparation acquires a
real native Claim and home, initializes the empty workspace and worktree, and
opens the default idle Agent session. It does not issue owner credentials,
resolve owner access tools, clone a user's repository or send a model prompt.

The separate `native_prepared_sandboxes` collection contains the global target
fence and reservation records. Count reservation documents by `state` to verify
`preparing`, `installing`, `ready`, `claimed`, `retiring` and `retired` separately.
Native pool `ready` counts describe unclaimed native members; a prepared
hierarchy has already claimed its member and therefore is not part of that
count. Only a recently verified `ready` reservation with identical canonical
runtime/profile content may be consumed by explicit default Agent Manager
creation. The authenticated owner's quota, name and idempotency checks remain
in the admission transaction. Claimed rows retain the exact Sandbox, Workspace,
Agent, worktree, home and session identity for reconciliation.

For an exactly matched consumed preparation, owned Agent startup replays the
worktree and Agent-home requests in one attempt after a fresh runtime inspection.
It records both ordered cleanup debts before either request, executes each
resource's writer fence, then atomically publishes both matching receipts and
completion logs. Publication writes revision-conditioned auxiliary fences on
the live Sandbox, Workspace, Worktree and claimed preparation. A changed parent,
worker lease or preparation cannot publish success. An interrupted attempt keeps
both debts; recovery can replay the pair or use the ordinary individual steps
when preparation evidence is unavailable. No runtime request is omitted, and
metadata conflict retries contain publication only.

First-message admission backs off bounded transaction conflicts. When all attempts
are known to have aborted, the worker rereads the exact first-message record and
its current unexpired writer lease before releasing that lease for a one-second
retry. An admitted or uncertain dispatch is never reset to queued; ambiguous
commits keep their existing reconciliation path. This prevents an unadmitted
first message from waiting for the full worker lease after database contention.

Connection publication has a separate lease after the opened conversation becomes
Ready. Its Mongo transactions also back off known-aborted conflicts. If bounded
publication attempts are exhausted, recovery rereads that exact connection claim
and releases only its unchanged, unexpired lease for a one-second retry. It may
adopt concurrent title/log metadata and a newly queued initial message; settings,
existing input, runtime generation, work state or successor lease changes refuse
that recovery. The ordinary worker reconnects under a new fence before dispatch.
Unknown commits keep reconciliation and cannot release the lease as an abort.
This path does not reset or replay an admitted user command whose queue deadline
has expired.

Response Stop inspection and reservation share one five-second retry scheduling
budget with bounded backoff and jitter. Credential authority retains its original
deadline. The budget does not cancel an in-flight transaction, resolve an unknown
commit as a failure, or guarantee that the complete HTTP request takes less than
five seconds. An uncertain result retains its exact control key and target for
reconciliation; a new key must not be used to hide the uncertain original request.

For acceptance, record a ready reservation's IDs and native Claim/member/PVC
identity; submit one explicit create; verify that the resulting owned hierarchy
retains those identities and starts its first message only after credential and
access-tool handoff. Measure request-to-Ready latency on that actual create;
local ACP session reuse alone does not establish end-to-end latency. Requests
with unmatched settings or no ready reservation continue through normal
creation. Reservations refresh their session health every five seconds, expire
after thirty minutes, and retain cleanup authority on failures. Retiring rows
count against the two-reservation cap until cleanup completes.

To disable preparation in production, set the explicit Deployment
`CHRONO_NATIVE_PREPARED_TARGET=0` override (or remove it to inherit the ConfigMap
default) and restart Agent Manager. The ConfigMap default remains `0`. This
retires only unclaimed reservations; claimed owned Sandboxes keep their existing
lifecycle. Run the compatible cleanup worker until unclaimed retirement debt is
cleared before rolling back to a binary that does not know the separate
collection.

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

The API/Web/default-runtime pins and source/CI annotations in
`infra/api/configmap.yaml`, `infra/api/deployment.yaml` and
`infra/web/deployment.yaml` match the completed combined publication receipt.
The Gateway manifest retains its verified cabfe image. Repository synchronization
records published identities. The verification report separately records
completed rollout/pool readiness, the 21.122-second creation measurement, and
remaining live acceptance.

The earlier four-manifest synchronization for AM cd37968d, Web ac837, Gateway
cabfe and default runtime 37d is retained in
`/tmp/cma-final-repo-sync-01/resolved-receipts.json` and
`/tmp/cma-final-repo-sync-01/final-manifests.patch`. Those historical hashes do
not describe the new combined pins. The legacy storage seed example and
deferred Image Manager are preserved. Updating manifests does not rebuild the
independently identified application images.

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
The pinned Codex bridge marks the injected `cma_access` server as required, so
new and prepared conversation opens wait for its discovery before accepting the
first task. Failed discovery fails the open instead of silently omitting the
owner permission tool. Discovery authentication uses a read-only transaction
snapshot of the capability receipt, creation lineage, current runtime and
unexpired delivered key; it never writes the Agent being published. Tool calls
and consent admission retain their independent current-authority write fences.
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

### Hot service-access scope compatibility

Owner service-access Save now replaces the exact scope shared by every Agent in
an existing managed-key Sandbox, retaining the runtime and conversations. The
worker rotates the key and completes old-key revocation before confirming
callback delivery. Live acceptance must measure final Save/Approve through
credential delivery, revocation completion and actual callback write; the
requested five-second budget excludes subsequent model response completion.
Local scheduler and runtime continuity tests alone do not establish that live
latency budget.

The first hot update persists `SandboxCredentialState.service_scope`, a
`ServiceAccessRevised` event, access-request `scope_mode` and
`expected_scope_revision`, the `updating` phase, `progress.hot_update`, and a
persistent notification intent. Every control-plane reader and retained rollback
image must understand those fields and events before new writes are enabled.
Legacy additive grants, paused workflows and notification records without the
optional intent remain readable. Never remove scope revisions, rewrite historical
receipts, or restart user runtimes to recover an older reader. The runtime wire
credential operation already supports replacing the in-memory key on the pinned
live runtime; this change does not require a new home file or a Pod replacement.


### Runtime title provenance reader floor

Automatic first-message purpose and runtime description provenance add optional
fields to two strict persisted structures: `FirstMessageRecord.purpose` and
`AgentRuntimeMetadata.description_source` / `description_is_setup_preview`.
The latter is nested both in Agent observations and `RuntimeMetadataChanged`
log records. Current readers accept old records with these fields absent; missing
purpose/source remains unknown and the setup flag defaults to false. Payload
hashes, creation fingerprints, turn identities and history remain unchanged.

Deploy the compatible API reader before the new runtime image and frontend.
Any API rollback target must include these readers before new durable writes
occur: new Agent admissions write purpose even while older runtimes are retained.
A pre-feature API binary uses `deny_unknown_fields` and cannot safely read these
new records. Restoring that binary after feature writes is unsupported.

A safe rollback restores the previous Web and default runtime selection while
retaining the compatible API reader. Existing Sandboxes keep their pinned image,
home, conversation and history. Never strip fields/events, rotate a live Sandbox,
or restore an earlier database snapshot to accommodate an old reader. This reader
floor is a release constraint; old-binary rollback has not been demonstrated.

The title bridge change requires a new runtime image for new allocations. Retained
runtimes emit unknown provenance until they supply positive evidence. The pinned
local provider test proves empty-conversation reload, exact multiline prompt
identity on completion and history reload, native explicit-title priority even
when equal to the preview, and native `thread/settings/update` to metadata on the
same running process/thread. It uses Codex 0.147.0, ACP 1.4.0 and a local Responses
fixture. It does not establish production acceptance or generate semantic titles.


The existing native empty-thread persistence workaround seeds the reserved name
`New Agent`. A fresh provider history load cannot distinguish that seed from an
identical deliberate native name if its explicit change was never durably
observed. Already recorded `provider_title` metadata, including `New Agent`,
remains authoritative over subsequent preview/unknown observations. No title is
rewritten to resolve this retained provider ambiguity, and no provenance is
invented for an unobserved legacy name.
