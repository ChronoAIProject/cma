# CMA native Sandbox production verification — 2026-10-14

CMA remains in `chronoai-prod-asia-northeast1-gke`, namespace `chrono-bot`.
Its runtime Sandboxes run in the independent `chronoai-prod-asia-southeast1-gke`,
namespace `chrono-pod`. Only CMA application code/configuration and CMA-owned
lifecycle resources were changed. Infra-owned agent-sandbox controllers, CRDs,
RBAC, network policies and storage configuration were not changed.

Agent Manager uses the `chrono-bot-ksa` workload identity and the infra-provided
`chrono-pod-configmap` CA/server/namespace. Authentication uses renewable Google
metadata credentials. Runtime HTTP and WebSocket traffic travels through the
remote Kubernetes `pods/exec` API; CRI port-forward could not reach this
`microvm` runtime's network namespace. Native `SandboxTemplate` volume claim
templates own the empty 64 GiB `home-a` PVCs. CMA does not need PVC create/patch
or seed snapshots for this allocation path.

## Running artifacts

Agent Manager source `80d27862d3ae4d39fdba968f68d234c550cbeec3` is deployed and Ready.
This includes the scope-change resume, repeated creation and borrowed-binding
conversation lookup corrections. Live Agent creation, retained conversation
recovery, owner-approved access revision and real Gateway event delivery have
passed. Agent Manager, Event Gateway and Web are Ready;
Image Manager has no Deployment. The shared MongoDB is `chrono-bot`, with service-owned
collection prefixes and startup migrations kept separate.

| Component | Immutable image digest |
| --- | --- |
| Agent Manager | `sha256:fec55c01d66aeb9ff24439867d198f679496f4d797ccd3c33c55609ed7307e1e` |
| Event Gateway | `sha256:540c5e73f923e9379b99eec76223c3d9a5fc84119c9bac5d8b24ec6d9dbc11a1` |
| Web | `sha256:d62695c668be11213ff608287455b69f9a530c2491d8d977db5a28bdb0e8e26b` |
| Default runtime | `sha256:028aca56ea54349300433d0bcf0d45b48a9ce83f7e6697207fb5dd8b4184bc5b` |

Web source `412e046e24fb8dbc778ca9129f1adf51b2a7d8aa` was published from the
passed CI image artifact of merge revision
`7a8ca05dfc23a7339d896884403a2c7b713b5a89`; the Web, TypeScript SDK and Web Dockerfile
inputs are identical between those revisions. A real owner UI edit returned
HTTP 200, advanced the conditional version from 2 to 3, and preserved the selected existing Agent
through persistence and reload; unrelated channel policy and Agent history
remained unchanged.

The runtime is published in
`asia-southeast1-docker.pkg.dev/chronoai-501608/chrono-pod/cma-runtime`.
Agent Manager is packaged as an amd64 musl release binary overlay on the existing
application image, retaining its runtime dependencies, nonroot user and
entrypoint. This local overlay is not a CI-published image.

The final Deployment and Pod readback confirmed one updated Ready replica for
each application and zero container restarts. Actual Pod image IDs matched all
three application digests above. Public `/`, `/agent-manager`, `/event-gateway`
and authenticated `/auth/session` returned HTTP 200. The Image Manager Deployment
was independently confirmed absent. The recorded API ConfigMap and all three
application Deployment manifests passed a server-side dry-run against the
explicit northeast context and `chrono-bot` namespace.

## Actual user flows

- API creation `f034959d722543838d1d753ad8883f9a` completed native allocation,
  empty Workspace creation, Agent startup, the automatic initial prompt and a
  real completed Codex response.
- Existing-Sandbox creation `bb1fc8543ce34794aa4f028670da9924` omitted the profile,
  image and repository. It created a distinct empty Workspace/Agent in the same
  Sandbox and applied `gpt-6`, `high`, `autonomous` and exactly
  `chrono-llm-public,ornn-api`. Its first model turn completed.
- Browser creation `e78ca958585246c68192216ce463671a` independently passed the
  revised minimal form, omitted-field defaults, fixed baseline scope checkboxes,
  exact initial prompt and completed assistant reply, including reload.
- Optional repository creation `a1baaf5d944641eb8d0820fd3488ce2f` preserved
  `source.type=empty` and put the public example repository URL in the first
  durable prompt. Codex ran three tool calls and completed the initial turn.
  CMA did not programmatically clone the selected repository.
- Pause/resume physically replaced the runtime Pod while retaining the same
  Sandbox and PVC UID `643a2b90-2bc1-43a6-b4c0-c9321d24b021`. Both original Agents
  retained their Workspace IDs, native conversation identities and complete
  prior transcript/event prefixes. A subsequent model turn returned the exact
  `CMA_NATIVE_RESUME_OK` marker in the original conversation.
- A second ordinary browser "New agent" creation, operation
  `de84f6e9bf314e2aa4b04788ffc176c0`, completed on source `80d27862` with the same
  default title as an earlier creation. The Sandbox name uses the durable
  operation ID; the visible Agent title and minimal form remain unchanged.
- Live Codex called `agent_key_scope_revision`; owner approval returned HTTP 200
  for the exact baseline scopes plus `cma-trigger-github-observer-staging`.
  Request `aar_a8cb7464d8630a8dd051bfc3daf7c05c66cbc1ad53e508f57c4145006b7c2f0e`
  reached `succeeded`. The additive grant was recorded once, its credential
  generation floor was 2 and delivered generation was 3. The original native
  conversation received one automatic notification and a completed Codex
  acknowledgment, without a manually submitted follow-up message. Reload and
  exact same-decision replay retained that single notification.
- This run used an already satisfied NyxID service connection. It exercised real
  owner/session/exact-scope checks and automatic key-delivery notification; it
  did not require or exercise a new third-party service OAuth callback. The
  independent Gateway login did exercise a real NyxID OAuth callback.

Earlier live acceptance exposed and corrected retained-owner cookie attribution,
native resume using newly granted scope as old identity, repeated default-name
creation, and borrowed conversation alias lookup. The original failed permission
request remains preserved in acceptance evidence; it was not reset or rewritten
in MongoDB. Its disposable Sandbox was retired through the CMA API and its exact
Claim, Sandbox and PVC were confirmed gone from the remote cluster.

The persisted owner Sandbox limit was updated through the conditional admin API
from three to ten, revision 6 to 7; the other platform settings were read back
and verified unchanged.

Gateway provider management now uses the same `cma-api.chrono-bot.svc` origin as
its preserved ChannelPointers. This restores the exact-origin service credential
selection after the service rename. The original isolated channel creation
retry returned HTTP 201 with the original record and binding. After correcting
borrowed-conversation alias resolution, the real owner UI's **Send test event**
returned HTTP 200 and rendered actual model text. Exactly one completed turn was
added to the selected Agent's original native conversation. A subsequent browser
reload verified the retained target and channel version without repeating the
model request.

The same isolated channel then accepted one authenticated structured CloudEvent
at its advertised public endpoint with HTTP 202. The event produced one completed
model acknowledgment in the original conversation while preserving prior history,
profile and Workspace. The owner UI deleted this disposable channel with a
version-guarded request (HTTP 204); lookup returned 404, and the borrowed Agent
and completed transcript remained intact. The original Telegram channel stayed
at version 6. These tests configured no external reply sinks and sent no external
messages.

## Validation and evidence limits

The scope-resume correction passed five native lifecycle tests, one retained
legacy service-access test, four new adapter regressions, seven suspension
checks and one native pool resume check. These groups overlap and are not
added into a total. Naming corrections passed four lifecycle admission tests,
including concurrent creation and pre-update lost-response recovery. Binding
resolution passed 19 core tests and all 34 Responses v2 tests, including the
actual Gateway management-test request shape without event context. Strict
all-target/all-feature Clippy passed for CMA, its adapter, core and public API.

Merged native transport: 342 adapter tests and 31 focused application tests passed.
The approval fix passed two nonignored regressions using the real cookie resolver
and one integration test on an isolated authenticated local MongoDB replica set.
Strict application Clippy passed after simplifying one existing optional lookup.
The Gateway pending-media regression passed with 300 overlapping provider waits,
256 free ingress permits and all 301 callbacks accepted. Its bounded admission
batches avoid falsely requiring unlimited simultaneous HTTP admission.

The production UI transport flag remains false, so owner Agent conversations use
Agent Manager's AG-UI path. Gateway readiness, routing, independent OIDC login,
owner session, channel editing, management test and authenticated custom-event
delivery passed. Gateway acceptance used supported caller-provided binding and
channel bearer credentials; it does not establish NyxID binding-key issuance or
delivery through third-party reply sinks. Automatic scope-update notification
uniqueness was checked through successful delivery, reload and exact approval
replay, not a process-crash/failover campaign.

Browser automation occasionally timed out while attaching or reloading. The
successful management model request was not repeated: its reload assertion was
recovered independently against the exact task-owned tab. The combined evidence
preserves both the original successful business assertions and that reload.

## Evidence index

The following local reports contain redacted acceptance receipts rather than
credentials. They are execution artifacts on the operator workstation, not
repository files.

| Acceptance | Local report |
| --- | --- |
| Minimal browser creation on deployed source | `/tmp/cma-real-portal-owner-proof-j9MtV6/report.json` |
| Actual MCP request, approval and automatic continuation | `/tmp/cma-real-permission-revision-9mPk6y/report.json` |
| Native allocation and retained Agent continuity | `/tmp/cma-native-live-accept-f034/` |
| Optional repository first prompt | `/tmp/cma-optional-repo-f034/status-redacted.json` |
| Gateway independent OIDC callback | `/tmp/cma-real-gateway-oidc-KQHAo4/report.json` |
| Gateway target-preserving owner edit | `/tmp/cma-real-gateway-edit-ZRzlk6/report.json` |
| Gateway management event and recovered reload | `/tmp/cma-real-gateway-business-reload-r0ThOP/report.json` |
| Real custom ingress, model response and channel cleanup | `/tmp/cma-real-gateway-ingress-Vk2ucc/report.json` |
| Failed browser Sandbox cleanup | `/tmp/cma-failed-browser-cleanup-80d.json` |
| Successful permission Sandbox cleanup | `/tmp/cma-successful-permission-cleanup-80d.json` |
| Main native/Gateway Sandbox cleanup | `/tmp/cma-main-disposable-cleanup-80d.json` |

The successful permission-test Sandbox reached `retired` revision 67 after a
single CMA DELETE accepted with HTTP 202. Its exact Claim, Sandbox, Pod and PVC
were confirmed absent. Native identities in that cleanup receipt were observed
after the permission flow succeeded; they are not evidence of pre-grant identity.

After Gateway acceptance, the main disposable Sandbox
`sbx_dae93c090e9f4bf38e25c353ff981b13` was also retired through one CMA API
DELETE (HTTP 202), reaching revision 220. Before deletion the receipt preserved
the selected ready Agent's 57 events and three completed turns, with sensitive
content excluded. The original Claim, Sandbox, final Pod and PVC were all
confirmed absent by captured names and UIDs. Cleanup used normal CMA lifecycle
operations; the user's original Sandbox was not touched.
