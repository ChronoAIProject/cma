# Bridge setup

<!-- nav: 12 Bridge setup -->

`Client::bridges()` manages bridge definitions and existing connector selections.
Use the current human/API dashboard bearer credential. A channel sender, callback
attestation, bot key or reply token does not grant dashboard management access.
Current organization membership is checked by CMA for every request.

`choices(organization)` lists existing bots, key metadata and routes for the
selected owner scope. `list(&BridgeListOptions)` returns a `BridgePage` with an
exclusive `next_cursor`; its default page size is 50 and maximum is 100. `read`
returns the safe current member/owner projection. `configuration` returns the
manager-only configuration, server-derived callback URL and safe custody status.

Create a disabled draft with an explicit `ResourceOwner` and
`BridgeConfiguration`. The configuration selects existing bot/key/route IDs,
partition policy, placement, sender/chat admission, reply policy and limits.
It has no issuer, provider base URL, callback override or custody reference input.
Those fields are determined by the server.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::bridges::{BridgeConfiguration, CreateBridgeRequest};
use cma_sdk::resource_access::ResourceOwner;

# async fn create(
#     client: &Client,
#     owner: ResourceOwner,
#     config: BridgeConfiguration,
# ) -> Result<(), Box<dyn std::error::Error>> {
let receipt = client.bridges().create(
    &CreateBridgeRequest { owner, config },
    &IdempotencyKey::new("create-bridge-one")?,
).await?;
let detail = client.bridges().configuration(&receipt.bridge_id).await?;
println!("{}: {}", detail.bridge.display_name, detail.callback_url);
# Ok(())
# }
```

Fresh `ExistingAgent` placement requires `FixedSharedAgent`, explicitly sharing
the retained Agent's context. Other partition policies use `NewSandbox` with
three exact published profile revisions. Configuration reads preserve older
retained policy syntax so that existing definitions can be inspected and
corrected. An incompatible historical policy cannot be submitted as a fresh
configuration.

`BridgeAdmission::Scoped` intersects explicit sender and actual-chat gates with
group mention/reply rules. Each gate is `Deny`, `Open` or a bounded unique
`Allowlist`. Exact opaque sender/chat IDs are preserved, including Unicode and
leading zeroes. Open admission does not override the server's verified
bot/system/loop exclusions or create a private reply destination.

Import the exact selected key using a borrowed, write-only request. Its `Debug`
implementation redacts the key, and the Client and Bridges handle retain no key
value. Import and validation do not take an idempotency header. Their expected
revision still pins the configuration observed by the manager.

```rust,no_run
use cma_sdk::Client;
use cma_sdk::bridges::ImportBridgeKeyRequest;

# async fn connect(
#     client: &Client,
#     bridge_id: &str,
#     selected_key: &str,
# ) -> Result<(), Box<dyn std::error::Error>> {
let detail = client.bridges().configuration(bridge_id).await?;
let status = client.bridges().import_key(bridge_id, &ImportBridgeKeyRequest {
    expected_revision: detail.bridge.revision,
    key: selected_key,
}).await?;
assert!(status.present && !status.revoked);
let validation = client.bridges()
    .validate(bridge_id, detail.bridge.revision).await?;
assert!(validation.valid);
# Ok(())
# }
```

`update`, `control` and `revoke_key` require an explicit `IdempotencyKey` of at
most 200 bytes, in addition to the exact current revision. Reuse one key only
with the same logical request. `BridgeControl` exposes enable, disable, retire
and independent ingress/execution/outbound emergency controls. Disable/retire
and relevant rebinding requests preserve the explicit `DrainOriginal` or
`SuspendOriginal` decision. Custody revocation also requires its exact current
custody revision and checks the returned successor revision.

The SDK rejects malformed bounded input before sending HTTP. It checks success
response identity, revision relationships, management attribution, inventory
scope/order and continuation consistency. Typed API errors retain the server's
reported setup/refusal code. The transport does not retry automatically.

Setup validation and enablement establish current configuration state. They do
not prove a Sandbox was provisioned, an Agent command completed, or a platform
reply was delivered. CMA reports unavailable renewable creation authority as an
explicit setup refusal. Existing-Agent execution additionally requires the
separate current bridge invoke grant.

`list_invoke_grants(bridge_id, &PageOptions)` returns management-only consent
history, including revoked grants, ordered by grant ID. Its default limit is 50
and maximum is 100; carry `BridgeInvokePage::next_cursor` into the next request.
`read_invoke_grant(grant_id)` reads the safe original target lineage and current
state. Grant IDs are `brinv_` followed by 64 lowercase hexadecimal characters.
These projections omit connector configuration, custody, authorization audit
records and execution proofs.

`invoke_readiness(bridge_id)` observes whether the current placement needs consent
and whether an active grant matches the current configuration. A
`ConsentPresent` candidate does not prove current key custody, enablement,
allocation readiness or permission to execute a command. CMA checks current
authority at each effect.

Issue consent only after the manager explicitly acknowledges sharing the retained
Agent's existing conversation context. The required `shared_context` enum has no
default. Both grant commands require an explicit idempotency key and observed
bridge revision; revocation also requires the observed grant revision.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::bridges::{BridgeAgentTarget, BridgeSharedContextConsent,
    IssueBridgeInvokeGrantRequest, RevokeBridgeInvokeGrantRequest};

# async fn acknowledged_consent(
#     client: &Client, bridge_id: &str, bridge_revision: u64,
#     exact_target: BridgeAgentTarget,
# ) -> Result<(), Box<dyn std::error::Error>> {
let request = IssueBridgeInvokeGrantRequest {
    expected_bridge_revision: bridge_revision,
    target: exact_target,
    shared_context: BridgeSharedContextConsent::UseRetainedAgentContext,
};
let issue_key = IdempotencyKey::new("grant-retained-context-one")?;
let issued = client.bridges().issue_invoke_grant(
    bridge_id, &request, &issue_key,
).await?;
let current = client.bridges().read(bridge_id).await?;
let revoked = client.bridges().revoke_invoke_grant(
    &issued.result.id,
    &RevokeBridgeInvokeGrantRequest {
        expected_bridge_revision: current.revision,
        expected_revision: issued.result.revision,
    },
    &IdempotencyKey::new("revoke-retained-context-one")?,
).await?;
assert_eq!(revoked.result.revision, 2);
# Ok(())
# }
```

Issuance starts an active grant at revision one; revocation is terminal at
revision two and preserves the original target lineage and consent. Grant
commands do not increment the bridge definition revision. Reconfiguring a
bridge does not silently refresh or reactivate its original grant.

Cancelling or dropping a command future stops awaiting its response. A server
commit may already have happened; cancellation cannot roll it back. The SDK
launches no detached tasks or automatic retries. After an uncertain issue or
revoke result, explicitly retry the unchanged body with the original key to
recover the retained receipt under current management authority. This is not
an end-to-end exactly-once execution or delivery guarantee.
