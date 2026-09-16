# Bridge setup and consent

<!-- nav: 12 Bridge setup -->

`cma.bridges` manages bridge definitions and explicit retained-Agent consent
using current human/API dashboard credentials. CMA rechecks ownership and
organization membership on each request. Callback signatures, channel sender
identity and platform reply credentials do not confer dashboard access.

`importKey(id, revision, key, { retainControlPlaneAuthority: true })` explicitly
authorizes only this Bridge and exact owner to use the current human OAuth
sign-in for 30 days. The option defaults to false. This permission continues
after browser logout; every operation checks current account and organization
admin access. Revoking the imported key ends it. A new sign-in or expiration
requires explicitly reconnecting the disabled Bridge. The grant references
existing encrypted refresh custody and never places a personal token in an
organization Sandbox. New-Sandbox provisioning still requires a supported,
distinct key with the exact selected service scope.

Use `choices({ organization })` to select existing connector bots, keys and
routes. `list`, `read` and `configuration` inspect current definitions and their
safe setup projections. `create` creates a disabled draft; `update`, `control`,
`validate`, `importKey` and `revokeKey` manage its configuration and selected
credential. Configuration contains explicit placement, partition, admission,
reply and bounded capacity policy. Issuer, provider base URL, callback URL and
custody references are server-owned values.

Fresh `existing_agent` placement requires `fixed_shared_agent`. The manager must
also explicitly consent to using that retained Agent's existing conversation
context. Configuration reads preserve historical partition policy so it can be
inspected and corrected. New-Sandbox placement selects exact published profile
revisions and depends on separately provisioned renewable creation authority.

`listInvokeGrants(bridgeId, options?)` returns ordered management-only history,
including revoked grants, with `items` and nullable `next_cursor`. Its default
limit is 50 and maximum is 100; pass a returned cursor unchanged to the next call.
`readInvokeGrant(grantId, options?)` preserves original target lineage and current
state. Grant IDs are `brinv_` followed by 64 lowercase hexadecimal characters.
Grant projections exclude connector configuration, custody, audit records and
execution proofs.

`invokeReadiness(bridgeId, options?)` returns `not_applicable`, `consent_required`
or `consent_present`, plus a nullable candidate. A present candidate must be
active consent for the exact current configuration. It does not prove current
custody, bridge enablement, allocation readiness or authority to execute a
command; CMA checks those conditions at the relevant effect.

```ts
import { createClient, type BridgeAgentTarget } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });
async function issueAcknowledgedConsent(
  bridgeId: string,
  observedBridgeRevision: number,
  exactTarget: BridgeAgentTarget,
) {
  // Call after the manager explicitly acknowledges sharing retained context.
  return cma.bridges.issueInvokeGrant(
    bridgeId,
    {
      expected_bridge_revision: observedBridgeRevision,
      target: exactTarget,
      shared_context: 'use_retained_agent_context',
    },
    { idempotencyKey: 'retain-consent-one' },
  );
}

async function revokeConsent(bridgeId: string, grantId: string) {
  const [bridge, grant] = await Promise.all([
    cma.bridges.read(bridgeId),
    cma.bridges.readInvokeGrant(grantId),
  ]);
  return cma.bridges.revokeInvokeGrant(
    grantId,
    {
      expected_bridge_revision: bridge.revision,
      expected_revision: grant.revision,
    },
    { idempotencyKey: 'revoke-consent-one' },
  );
}
```

Issue requires the exact observed bridge revision, selected target IDs and
explicit `shared_context`; consent has no default. Revoke requires both the
observed bridge and grant revisions. Both commands take a visible ASCII
`idempotencyKey` of at most 200 characters. Issuance returns HTTP 201 and an
active grant at revision one; revocation returns HTTP 200 and terminal revision
two, preserving original consent and lineage. These commands do not increment
the bridge definition revision. Reconfiguration does not refresh or reactivate
existing grants.

All five methods accept the ordinary `signal`, `timeoutMs` and `requestId`
controls. Aborting a command stops response observation, but a server commit may
already have happened. The SDK launches no automatic retries. After an uncertain
issue or revoke result, explicitly retry the unchanged body and original key to
recover the retained receipt under current management authority. This does not
guarantee exactly-once Agent execution or platform delivery.

Malformed input is rejected before HTTP. Success decoders validate grant IDs,
digest structure, revision relationships, actual manager attribution, target
correlation, page ordering and continuation. Unexpected grant fields are
rejected. The TypeScript numeric boundary is JavaScript's safe integer range.

`listDeliveries(bridgeId, options?)` and `readDelivery(bridgeId, itemId, options?)`
provide current owner/admin reply history. The list has default limit 50, maximum
100 and a nullable exclusive `next_cursor`. `BridgeDelivery` exposes original
configuration/receipt, chunk order, status, safe failure codes and up to sixteen
retained attempts. It excludes text, chat/thread, inbound IDs and secret references.

`recoverDelivery(bridgeId, request, {idempotencyKey, signal?})` accepts
`BridgeDeliveryRecoveryRequest`: original `item_id`, current `expected_revision`,
current `expected_bridge_revision`, and a `BridgeDeliveryRecoveryAction`. Retry
is `{kind: 'retry', accept_duplicate_risk: boolean}`; Unknown requires `true`
and can duplicate an earlier send. Abandon is `{kind: 'abandon'}`. Reconcile is
`{kind: 'reconcile'}` and only refreshes expired local worker leases; with
`upstream_reconciliation_supported: false`, Unknown remains unconfirmed.

A `BridgeDeliveryRecoveryReceipt` describes the original post-command result.
Queued is not delivered. Keep the exact request/key across cancellation, timeout
or a lost response; the server returns the same committed result after fresh
manager authorization even if the item has since progressed. Changed key input
conflicts. No method automatically resends, reroutes a reply or submits a new
recovery command in the background. SDK decoders enforce identity, source,
revision, attempt ordering and exact command-result correlation.

## Shared parent placements

`BridgePlacement` also represents `workspace_per_partition` and
`worktree_agent_per_partition`. The first selects `sandbox_id` and exact
`workspace_profile` / `agent_profile` pins. The second selects both `sandbox_id`
and `workspace_id` and an exact `agent_profile` pin. Every shared placement
requires `BridgeSharedTrust` with
`all_participants_share_filesystem_processes_and_credentials` and
`BridgeSharedCapacity` with `keep_other_workspaces` and
`keep_other_agents_per_workspace` from 1 through 15. Missing acknowledgment and
invalid reserved capacity are rejected before request transport.

All participants share the parent's filesystem, processes and credentials.
Separate Agent homes and Git worktrees do not provide security isolation. The
server checks current Bridge management, whole-parent management authority,
exact owner/ancestry, immutable profiles and existing service/model scope. An
incompatible parent returns `bridge_incompatible_target` (409). Existing
partitions retain their original placement and profiles after configuration
updates. Reset retains the old generation and creates distinct children; it
never moves sibling work.

`client.bridges.capabilities({ organization })` exposes an optional `placements`
array of `BridgePlacementCapabilities`. Each entry names one shared placement
kind and an availability of `runtime_unavailable`, `continuity_unverified` or
`available`. Older servers may omit the field; treat missing entries as
unavailable. The decoder rejects null, duplicate kinds, more than two entries,
and unknown kinds or states. The current server composition reports only the two
unavailable states; static runtime support does not establish live shared-parent
upgrade or rollback continuity, and stage switches cannot establish it.

The dashboard uses existing public Sandbox/Workspace inventory and effective
access to select exact parents in personal or organization scope. An available
catalog does not authorize a target: current owner/management authority, ancestry,
profile pins, explicit shared trust and nonzero reserves remain required. These
reads and new-placement availability are checked again before a fresh save.
Retained settings remain readable while unavailable, and unrelated settings may
be saved with an unchanged placement. An uncertain mutation must retain its exact
body and original key for explicit receipt recovery, regardless of later catalog
changes. SDK support and scripted editor tests do not prove live rollout gates.

Use `capabilities({ organization })` to read source/fixture capabilities and independent deployment switches under current owner/admin access. An unavailable or unknown catalog does not imply platform support. Telegram-new keeps the exact `telegram-new` identity; its gate is separate from Telegram. Text replies use explicit channel tools in channel-created active conversations. Attachments require a download capability that is currently unavailable, and protocol fixtures do not establish live platform delivery.

## Explicit child cleanup

`listAllocations(bridgeId, options?)` returns a `BridgeCleanupAllocationPage` of
ordered `BridgeCleanupAllocation` rows. `readChildCleanup(bridgeId, allocationId,
options?)` reads `BridgeChildCleanupStatus` without requesting removal.
`requestChildCleanup(bridgeId, allocationId, request, commandOptions)` submits a
`BridgeChildCleanupRequest` once and requires HTTP 202 with a matching
`BridgeChildCleanupReceipt`. All three methods accept ordinary cancellation,
timeout and request-ID controls.

```ts
import { createClient, type BridgeChildCleanupRequest } from '@chronoai/cma-sdk';

const cma = createClient();
export async function removeExplicitlyReviewedChild(
  bridgeId: string,
  allocationId: string,
  request: BridgeChildCleanupRequest,
  originalKey: string,
  signal: AbortSignal,
) {
  // Persist these exact revisions, action and key with the manager's consent.
  return cma.bridges.requestChildCleanup(bridgeId, allocationId, request, {
    idempotencyKey: originalKey,
    signal,
  });
}

export async function inspectAllocations(bridgeId: string) {
  let cursor: string | undefined;
  do {
    const page = await cma.bridges.listAllocations(bridgeId, { cursor, limit: 50 });
    for (const allocation of page.items) {
      console.log(allocation.id, allocation.generation, allocation.state);
    }
    cursor = page.next_cursor ?? undefined;
  } while (cursor !== undefined);
}
```

The request has exactly `expected_bridge_revision`,
`expected_allocation_revision`, and `action: 'remove_child'`. Revisions must be
positive safe integers. `BridgeChildCleanupProgress` and
`BridgeChildCleanupHold` represent current safe progress; `BridgeCleanupPlacement`
and `BridgeCleanupAllocationState` describe original allocation context.

New acceptance requires a retired `reuse_parent` allocation and current authority
of the original configuring actor. Cleanup requires explicit manager consent to
remove that allocation's Agent and Worktree. Its Workspace and history remain retained. The response
separates the immutable acceptance receipt from current progress and holds.
HTTP 202 confirms durable acceptance; capacity is released only at `complete`.
The allocation revision stays at the revision named in the original request.
An adopted cleanup observation may predate the receipt's acceptance timestamp.

Allocation reads check current Bridge and whole-parent management authority and
remain readable while the parent is paused. Lists filter inaccessible or missing
parents without hiding other accessible allocations. The limit defaults to 50
and permits 1 through 100. `next_cursor` is an opaque, Bridge-scoped continuation:
pass it unchanged and continue until null, including after an empty or short
page. Do not derive a cursor from an allocation ID.

Retain the exact request and original key after cancellation, timeout, response
loss, 409 or 503. A later explicit replay recovers the same receipt with current
progress after fresh authorization. Do not silently choose new revisions, a new
key or another target. The SDK performs no automatic retries. Cleanup keys
permit 1 through 200 visible ASCII characters without spaces; response decoders
reject inconsistent scope, receipt, target, capacity, retention or hold order.

## Separate owned Workspace cleanup

Child `remove_child` always keeps the Workspace within that operation's effect
scope. A later, separately authorized Workspace cleanup can remove the exact
Workspace originally created by a retired `workspace_per_partition` allocation,
after its named child cleanup is physically Complete. It preserves the parent
Sandbox, sibling Workspaces and retained history. It never cascades to additional
Agent or Worktree history. A reused Workspace is not eligible.

The Workspace request has exactly `expected_bridge_revision`,
`expected_allocation_revision`, `expected_workspace_revision`,
`child_cleanup_receipt_id`, and `action: 'remove_owned_workspace'`. Read and review
the exact target and current input revisions, then obtain separate consent before
submitting. `eligible` is true only when the current checks and demonstrated
runtime capability permit new consent; `runtime_supported` never follows merely
from an existing receipt or source-level tests. A missing capability or incomplete
child cleanup keeps new consent unavailable.

The `BridgeWorkspaceCleanupStatus` separates its immutable
`BridgeWorkspaceCleanupReceipt` from current `BridgeWorkspaceCleanupProgress`
and ordered `BridgeWorkspaceCleanupHold` observations. Nullable Workspace revision,
child receipt and observation fields are always present in JSON. Ineligible reads
may have unavailable inputs; eligible reads and accepted jobs must retain their
reviewable Workspace revision and child receipt. Current Workspace revision may
advance after the original consent. Progress is `not_requested`, `requested`,
`admission_closed`, `removal_started`, `runtime_removed`, or `complete`.

Only `workspace_capacity_released: true` at `complete` confirms release of this
Workspace reservation, backed by permanent completion evidence. Acceptance and
runtime-removal progress do not release it. `parent_retained` and
`history_retained` stay true. The original child receipt's `workspace_retained`
continues to describe its own limited effect; it does not forbid this later
separately consented operation.

Keep the original five-field request and key after cancellation, timeout, a lost
response, 409 or 503. An explicit identical replay recovers the same receipt after
fresh authority checks, even when Bridge revision or current runtime capability
has changed. Never substitute current revisions, a replacement child receipt,
a different key or another target during recovery. These methods submit once and
perform no automatic mutation retry. Keys contain 1 through 200 visible ASCII
characters without spaces.

`readWorkspaceCleanup(bridgeId, allocationId, options?)` inspects current safe
status. `requestWorkspaceCleanup(bridgeId, allocationId, request, commandOptions)`
requires HTTP 202 and validates the entire returned consent against the exact
captured request. Cancellation, timeout and request-ID options are supported.

```ts
import { createClient, type BridgeWorkspaceCleanupRequest } from '@chronoai/cma-sdk';

const cma = createClient();
export async function removeExplicitlyReviewedOwnedWorkspace(
  bridgeId: string,
  allocationId: string,
  originalRequest: BridgeWorkspaceCleanupRequest,
  originalKey: string,
  signal: AbortSignal,
) {
  // Persist all five original fields and this key with separate Workspace consent.
  return cma.bridges.requestWorkspaceCleanup(bridgeId, allocationId, originalRequest, {
    idempotencyKey: originalKey,
    signal,
  });
}
```

## Original-sender control consent

```ts
import { createClient, type BridgeSourceControlRequest } from '@chronoai/cma-sdk';
const cma = createClient({
  baseUrl: 'https://cma.example',
  credential: () => 'selected-credential',
});
export async function recordSourceConsent(
  grantId: string,
  request: BridgeSourceControlRequest,
  originalKey: string,
  signal?: AbortSignal,
) {
  return cma.bridges.setSourceControls(grantId, request, { idempotencyKey: originalKey, signal });
}
export async function inspectSourceConsent(grantId: string) {
  return cma.bridges.readSourceControls(grantId);
}
```

Consent is recorded separately for each invoke grant. A missing policy is required
null, whereas a revoked policy keeps its identity and revision. Only an explicit
request records or revokes consent; reads never do so. Current Bridge management
and exact target management/interaction authority remain required.

The request contains exactly four fields: `expected_bridge_revision`,
`expected_grant_revision`, `expected_policy_revision`, and `action`. Bridge revisions
are positive signed 64-bit values (safe integers in TypeScript); grant revisions are 1
or 2. Absent policy uses revision 0. Allow accepts policy revisions 0 through 254;
revoke accepts0 through 255. Policy result revisions are 1 through 256, with 256
reserved for revocation. Actions are `allow_sender_cancel_and_steer` and
`revoke_sender_controls`. Scope is always `source_sender`; approval mode is always
`dashboard_only`. There is no approval action in this consent API.

Keys contain 1 through 200 visible ASCII characters without spaces. Retain the
exact request and original key after cancellation, response loss, 409 or 503.
Recovery rechecks current authority and returns the original receipt before
fresh revision checks, including after policy or invoke-grant revocation.
Recovering an older enabled receipt does not re-enable the current policy.
Read the policy separately when current state is needed. New enablement of a
revoked invoke grant is refused; explicit policy revocation remains available.
Neither SDK retries mutations automatically.

An enabled policy records consent only. It does not establish current connector,
source-sender, exact-turn, retained-runtime, custody or admission authority, and
does not enable cancel/steer execution, source-reference tools, channel approvals,
reply producers or automatic notices. No resource, invoke grant or conversation
is rewritten by recording this policy.

## Read partition status

`listPartitions(bridgeId, options?)` and `readPartition(bridgeId, allocationId, options?)` inspect exact retained partition generations.
List pages default to and allow at most 25 rows; pass the opaque scoped cursor
unchanged. Detail reads expose at most 64 pending Channel commands plus the
latest correlated response. `pending_truncated` and `source_unavailable` mark
incomplete observations. Target/source/activity use explicit available,
unavailable or redacted states; reserved targets identify resources not yet
present. Source labels describe the first accepted message, and must be rendered
as text. Durable Agent status does not prove current worker connectivity. These
GET operations preserve existing conversation continuity and admit no work.

## Additional platform boundaries

Slack (`slack`), WhatsApp (`whatsapp`) and X (`x`) retain distinct platform
identities in configuration and connector choices. The capability catalog can
report verified identity fields and Slack `slack_thread` facts while
`inbound_text` and `output_text` remain false. These boundary fixtures do not
establish human/loop evidence, Agent admission, media access or live delivery.
Always inspect the capability and stage values before offering channel work.

## Explicit partition reset and detach

`resetPartition(bridgeId, allocationId, request, { idempotencyKey })` and
`detachPartition(bridgeId, allocationId, request, { idempotencyKey })` require
`expected_bridge_revision` and the exact allocation `expected_revision`.
The manager must choose the action explicitly. HTTP 202 returns a permanent
`PartitionCommandReceipt` for that original request, including the retired
original result and, for reset, its successor generation. A reserved successor
has not yet finished resource creation.

Reset preserves the original configuration and history. For `existing_agent`
placement the successor keeps the same Agent's native conversation and context.
Detach stops new partition input while retaining the Agent, resources and
history. Cleanup uses separate explicit operations.

After an uncertain reply, retain the complete action, allocation, request and
key. Explicitly replay those same values to recover the original receipt even
if later revisions changed. A changed body or action under the same key
conflicts. Current owner or organization-admin authority is always required,
including receipt recovery. The SDK sends each request once and never
substitutes a new key or retries automatically.
