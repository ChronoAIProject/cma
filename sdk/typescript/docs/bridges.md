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
