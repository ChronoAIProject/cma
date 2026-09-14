# Unified service and Codex connection flow

The active delivery goal is the complete production flow, including deployment
and end-to-end verification. The earlier public-LLM default change does not
complete this goal.

## Required behavior and acceptance evidence

| Requirement | Required evidence |
| --- | --- |
| Separate NyxID services from the Codex provider | Profile editor and sandbox information show the two groups with readable names; model providers are absent from the NyxID service picker. |
| Select Chrono LLM (public) or Codex subscription per sandbox | Published profile and created sandbox preserve the selected model-access mode. Managed calls use the NyxID public LLM route; subscription calls use native OpenAI authentication and endpoints. |
| Complete subscription sign-in before admission | A missing subscription leaves the launch or manual creation intent waiting with no sandbox or warm reservation assigned. Completing the supported OpenAI OAuth ceremony makes the same intent eligible. |
| Guide all missing connections in sequence | One Connect all action opens the first missing service, verifies it on the server, and advances through the remaining services, with progress, cancellation, retry, expiry and popup-blocked handling. |
| Apply the gate to every entry path | Trigger fresh/warm assignment and reuse, New Session creation, existing sandbox open/revival, and agent reopen all enforce the required NyxID and model-provider connections. |
| Handle expiry and re-login | Normal renewable credentials refresh without disruption. An invalid or revoked subscription exposes Re-login with Codex subscription on the affected agent and resumes the existing conversation after successful reauthentication. |
| Preserve warm preparation and ownership isolation | Both provider modes prepare workspaces, worktrees and idle agent sessions before assignment without user credentials. Credentials are bound only to the authenticated owner and assigned sandbox. |
| Deploy and verify the complete feature | Immutable API/web/runtime images, healthy rollout, real production truretruing managed-provider journey, subscription OAuth and inference journey, missing-service sequence, renewal/re-login, and preservation of existing conversations. |
| Monitor local storage | Record free space and remove only identified stale build outputs, worktrees, images and caches; retain recoverable source and release evidence. |

## Current evidence and implementation direction

The starting production release is API source `d239e15c`, with manifest and
verification record committed in `bb55b991`. Three public-LLM warm environments
were verified. The existing `model_access` contract already has two values:
`chronoai_managed` and `own_credentials`; retain these persisted values while
giving them the precise user-facing provider names.

Existing subscription authentication starts inside the ACP handshake after
sandbox assignment. It cannot provide the requested admission gate, and warm
subscription preparation must not start an interactive owner login. Existing
NyxID connection handoffs are tied only to Trigger Launches and require an
explicit Verify click. Both restrictions need to change for this goal.

Use the official Codex device-code OAuth flow for hosted sign-in. OpenAI's
ordinary browser flow returns to a localhost callback; the documented hosted
alternative is `account/login/start` with `chatgptDeviceCode`. The UI displays
the supplied verification URL and one-time code. This is an OAuth sign-in,
not an API-key entry flow. Device-code availability can depend on the user's
ChatGPT security or workspace settings, which must have an actionable error.

The model-provider credential must have an owner-bound, durable connection
intent before assignment. Native account authentication, encrypted credential
custody, refresh ownership, runtime binding and re-login must be implemented
together; a checkbox or successful unassigned login is insufficient evidence.
The public and subscription provider must use the same admission-check boundary
as the required NyxID services, without granting a subscription marker to NyxID.

Official sources fetched on 2026-09-11:

- <https://developers.openai.com/codex/auth/>: supported subscription login,
  device-code flow on headless devices, credential storage and automatic refresh.
- <https://developers.openai.com/codex/app-server/>: account/read,
  chatgptDeviceCode login, login completion and cancellation, and externally
  managed ChatGPT tokens with explicit refresh requests.

The runtime uses the documented experimental `chatgptAuthTokens` interface,
with native Codex 0.147.0 and ACP 1.4.0 pinned to verified bytes. Its synthetic
native contract test passes: a credential-free warm conversation rejects
prompts, accepts owner access-token binding, and loads the same conversation.
Neither the refresh token nor a runtime `auth.json` is installed in the sandbox.

## Implemented authority and browser flow

The API image includes the same pinned native Codex binary. CMA runs it in a
private temporary home for device-code OAuth and `account/read` renewal. The
native OAuth cache and pending device information are encrypted in the
`codex_subscription_connections` MongoDB collection. Authenticated encryption
binds both fields to the owner and deployment. Compare-and-swap revisions and
expiring refresh leases prevent a stale replica from replacing newer authority.

The runtime requests short-lived access tokens through its authenticated CMA
control connection. The control plane validates the sandbox, runtime handle,
agent and owner before releasing access authority. Unassigned warm environments
receive only permission to prepare idle conversations. Native OpenAI routing is
retained in subscription mode; public managed mode uses the existing scoped
NyxID public LLM route.

The browser uses authenticated, non-cacheable POST endpoints at
`/auth/connections/check`, `/auth/connections/start` and
`/auth/connections/cancel`. Targets are an exact sandbox profile revision,
sandbox, workspace, agent or launch. NyxID and Codex requirements are separated
in the UI. Connect all reuses a popup, verifies each service on the server and
advances automatically. Recovery checks the current owner before restoring a
saved handoff; cancelled or superseded async responses cannot release an action.
Forced subscription re-login stays pending even if an older credential works.

The same browser wizard covers trigger review and recovery, manual resource
creation, direct sandbox opening/resume and agent opening/messaging. Resource
creation and reopening also enforce the gate on the server. Read-only admin
inspection does not connect the administrator's credentials to another owner.
Subscription agents expose Re-login with Codex subscription.

## Validation before deployment

- Rust regression: 584 CMA, 182 public adapter, 643 core and 171 runtime adapter
  tests pass. The public adapter suite replays 1,376 shared HTTP contract cases.
  The documented 32 MiB Rust test-thread stack is required for the existing
  large worker lifecycle tests. External integration tests are run separately.
- Frontend: all 1,206 tests pass, including seven new wizard scenarios; typecheck,
  lint and the Vite build pass.
- MongoDB replica-set test proves encrypted custody, owner/deployment binding,
  exclusive refresh CAS and rejection of stale writes.
- Native synthetic subscription contract passes with the pinned binaries.
  Linux API authentication image builds and verifies the native binary digest.
- Removed 31 stale Cargo cache directories/test binaries, reclaiming about
  5.5 GiB. A later cleanup removed another 11 idle incremental caches, about
  3 GB. Retained source can regenerate these artifacts.

## Production verification on 2026-09-11

The unified API and runtime from `1120f745` were deployed successfully. The
browser journey exposed an over-wide polling/cancellation request: it sent the
whole returned handoff to an API that accepts only the service and intent IDs.
Web revision `20c24796` fixes that boundary. Its production browser recovery
reuses the original pending login and receives HTTP 200 for the exact active
intent. All eight connection tests, TypeScript checking and lint pass.

The real GitHub README links to the named Truretruing trigger through its
`/api/v1/triggers/public/.../open/...` entry endpoint. Verification used GitHub's
rendered README and real badge/entry requests without modifying the README.

| Production observation | Result |
| --- | --- |
| Truretruing provider and required integrations | Chrono LLM (public), GitHub OAuth and Ornn; all server checks satisfied for the test owner. |
| Availability configuration | Three warm sandboxes; always keep alive enabled. |
| Preparation before the click | Three ready, unassigned environments, each with workspace, worktree, skills and an idle native conversation; no owner credentials. |
| README launch | `lch_55dc259f0c3b4011a53f18d92a8127b5`; sandbox `sbx_2dce2a5e65a54eed84bb083e9024ee11`. |
| Message box usable | 9,054 ms after clicking the README badge. |
| Launch readiness observation | 10,718 ms; the prepared workspace and native conversation were preserved. |
| Configured first task | Exactly one first prompt, real model output and tool execution, completed in 224,879 ms. This is task execution time, separate from sandbox availability. |
| Owner binding | GitHub login `chronoai-shining`; matching Git author identity in workspace and worktree. |
| Browser errors during managed journey | None. |
| Missing subscription gate | HTTP 409 `connections_required`; zero sandbox assignments. |
| Production subscription button | Opens native OpenAI device OAuth; server keeps the launch unassigned while login is pending. |
| Native model compatibility | Pinned native model catalog includes `gpt-5.6-sol` as its default. |
| Global snapshot interval | 1,800 seconds. |
| Stale resource cleanup | Two failed/stale owned sandboxes retired, plus superseded unowned warm runtimes. Calvin's paused `test2` and its sealed snapshot were preserved. Retirement history remains in CMA. |

Sustained OAuth polling also exposed the generic POST creation rate limit.
Revision `d176d5c0` classifies connection checks as reads and gives the three
authenticated connection routes per-owner operation budgets. An actual
middleware test sends 30 polls, verifies login creation remains available,
enforces its creation limit, and proves a second owner retains an independent
login allowance. All 11 rate-limit tests pass. Its API rollout is healthy.

The final deployed image pins are:

| Component | Source revision | Image digest |
| --- | --- | --- |
| API | `d176d5c0` | `sha256:8eba12c107b5823573b129b4fb00b99114da731141ebb116078e47acf0c48d87` |
| Web | `20c24796` | `sha256:812174585e1cec37d55e2313c28f8523295a9e7b903a3e025eda33777aa725b0` |
| Runtime | `1120f745` | `sha256:5541f5a711d90c61824fe6a55b4c762d21a37cb2a63574f552390520eabf5ee4` |

All images were scanned with zero fixable HIGH/CRITICAL vulnerabilities and
their immutable remote digests were checked before deployment. The managed
test's pause sealed a snapshot at `2026-09-11T11:02:43Z`, preserving its exact
conversation, and the warm pool replenished to three fully prepared entries.

After the API correction, all 35 connection checks over 94 seconds returned
HTTP 200; a NyxID sign-in handoff and cancellation then both returned HTTP 200.
No external service connection was changed by that read/handoff check.
The same Truretruing sandbox was revived through the trigger in 66,154 ms,
returning to the original workspace and agent with the exact saved transcript
and no replayed first prompt. Its browser connection check succeeded and no
page errors occurred. This measured revival is distinct from the 9-second
fresh warm assignment.

Another 12 superseded local OCI archives were removed, reclaiming 1.65 GB.
Their verified images remain in the registry. Current release archives,
source, and evidence were retained; local free space was about 10 GiB.

An additional isolated production check prepared a subscription-mode warm
sandbox with a directory workspace, worktree and idle native conversation.
The runtime remained in `installed` (unassigned) phase, had no agent `auth.json`,
no managed model-provider configuration, and zero user prompts. Its only NyxID
grant requirement was Ornn. A create request while the owner's subscription
was missing returned HTTP 409 `connections_required`; the warm sandbox remained
absent from the owned sandbox API, with the same pod UID and unchanged prepared
conversation. Evidence: `subscription-warm-result.json`. This proves native
subscription preparation before credentials in production; it does not prove
authenticated subscription assignment or inference.
The temporary runtime and its home PVC were then removed through normal warm
pool retirement, and its disposable profiles/trigger were retired. The original
subscription login test remains available. Evidence:
`subscription-warm-cleanup.json`.

Real subscription OAuth completion, subscription inference and renewable-token
acceptance still require the owner's OpenAI sign-in. No existing local OpenAI
credential was copied into production. Missing-service sequencing has automated
coverage; a real new user's disconnected NyxID services have not been connected
in this verification. These pending checks must not be reported as completed.

Detailed local evidence is retained in
`/tmp/cma-provider-connections.h1FNhc/`, including `managed-result.json`,
`managed-pool-before.json`, `subscription-test.json`, and
`subscription-poll-recovery.json`, plus `managed-revival.json`,
`poll-rate-result.json`, and `final-evidence.json`. Do not publish the temporary OAuth handoff
or authenticated browser files.
