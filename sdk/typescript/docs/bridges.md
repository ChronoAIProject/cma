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

These types describe the public contract. The dashboard choices remain disabled
until backend validation and live Agent continuity checks pass; SDK support is
not proof that those rollout gates have passed.

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
