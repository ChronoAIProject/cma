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
  5.5 GiB. Retained source can regenerate them. No production state was deleted.

Production rollout and real subscription OAuth/inference acceptance remain
pending; local and synthetic checks do not establish those outcomes.
