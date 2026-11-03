# Operations

<!-- nav: 9 Operations -->

`read_defaults()` returns deployment defaults available to the authenticated
caller. `admin()` uses public capability-gated routes for Sandbox and profile
inventory, durable events, runtime inventory, activity, and platform settings.
The SDK has no private operator channel.

Sandbox and Profile inventory rows include `owner_name`, and Sandbox detail
includes `owner_name` beside `sandbox`. Activity includes `actor_name` and a
`target_name` for user targets. These public names are `Option<String>`; a missing
name must not be replaced by an owner or actor subject identifier. The ordinary
Sandbox read also includes immutable `owner` and its current `owner_name`, with
absence supported for older servers. Names never grant resource authority.

```rust,no_run
use cma_sdk::{Client, admin::{ActivityListOptions, SandboxListOptions}};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let defaults = client.read_defaults().await?;
let sandboxes = client.admin().list_sandboxes(&SandboxListOptions::default()).await?;
let activity = client.admin().list_activity(&ActivityListOptions::default()).await?;
let inventory = client.admin().read_runtime_inventory().await?;
let settings = client.admin().read_settings().await?;
# let _ = (defaults, sandboxes, activity, inventory, settings);
# Ok(())
# }
```

Settings `replace_settings`, `patch_settings`, and `reset_settings` require the
current `expected_revision`. On CAS conflict, reread and evaluate the intervening
change rather than overwriting it automatically. `Section<T>` retains additive
settings fields, so editing a known field does not discard newer deployment
configuration. Patch replaces only supplied sections; it is not an arbitrary
deep merge. Server validation remains authoritative.

Sandbox persistence is owned by Agent Sandbox and its attached PV. CMA exposes
no public setting for snapshot scheduling or checkpoint archive retention.

`pause_triggers` and `resume_triggers` change orchestration controls. They do not
cancel admitted Launches or authorize deleting their resources. Operator
`retire_sandbox` and `retire_profile` are explicit mutations under their own
capabilities, not keyed owner-resource commands. Inventory and activity pages
use opaque cursors with an inclusive 1 through 200 bound.

The pause control sets `triggers.orchestration_paused`, freezing orchestration
of admitted work. It does not disable new admission. `triggers.enabled` is the
separate admission setting; changing either control must preserve the other's
recorded value and the authority of already admitted Launches.

`read_openapi` and `read_conformance` inspect anonymous deployment declarations.
`unsupported().mcp()`, `a2a()`, and `agent_card()` expose named refusal evidence;
they do not invent Agent tools, tasks, or an executable terminal plane.

## Native warm pools

Native pools reserve the deployment's pinned system runtime image and CPU/memory
allocation. The server owns the initialized home seed and runtime admission
policy. Inventory and writes require `configure_platform`.
The Rust and TypeScript SDKs expose the same three pool routes and replay the
shared backend contract corpus for accepted operations and API errors.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey, admin::{ConfigureWarmPoolBody, ResizeWarmPoolBody}};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let inventory = client.admin().list_warm_pools().await?;
let target = client.admin().configure_warm_pool(
    &ConfigureWarmPoolBody {
        cpu_millis: inventory.default_cpu_millis,
        memory_mib: inventory.default_memory_mib,
        target_replicas: 2,
    },
    &IdempotencyKey::new("operator-pool-create-2026-10-13")?,
).await?;

// A 202 receipt acknowledges durable intent. Poll inventory for readiness.
let refreshed = client.admin().list_warm_pools().await?;
if let Some(current) = refreshed.items.iter().find(|pool| pool.id == target.id) {
    client.admin().resize_warm_pool(&current.id, &ResizeWarmPoolBody {
        expected_revision: current.revision,
        target_replicas: 0,
    }).await?;
}
# Ok(())
# }
```

Reuse the same key and request after an uncertain creation response. Pool keys
are limited to 128 visible ASCII bytes, narrower than the shared
`IdempotencyKey` type. Resize requires the current positive revision below
`i64::MAX`; refresh after a conflict before evaluating another command. Targets
range from zero to 1,000; zero stops replenishment while claimed Agent runtimes keep their homes
and lifecycle.

`available: false` means provider inventory could not be observed. Recorded
targets remain visible with their original images. `ready_replicas: None` means
readiness is unknown; inspect `status` and `observed` for reconciliation progress.

Pool inventory exposes each target's immutable `config_sha256` and the current
`default_config_sha256` for the default image, CPU, memory, home, and Pod policy.
Older servers omit these fields, which the SDK represents as `None`.
Select default capacity only when both digests are `Some` and equal;
matching only image and resources can select a retained older policy.
Use a new idempotency key to create a target after policy changes. Replaying an
old key and resizing an old target preserve that target's original policy.

## Task experience policy

`client.read_task_experience().await?` dispatches authenticated
`GET /api/v2/ui/task-experience` without caching. The version1 `TaskExperience`
contains only the caller's `account_id` and `TaskExperienceCapabilities` booleans:
`task_home_admission`, `suggested_followup_admission`, `desktop_enable`,
`browser_takeover`, `replay_presentation`. Invalid or unsupported schema replies
return `Error::Protocol`. Fence replies to the current identity; refresh policy
after identity/focus changes. Unsupported servers do not prevent ordinary manual
conversation and never implicitly enable the new features.

Policy permissions are separate from real environment capability and per-request
resource authorization. Closing admission preserves original accepted request
recovery, keys and records. Browser status/receipt recovery and explicit return
remain available while new acquire/input/renew is disabled. Replay is presentation
only; authorized history remains readable. Native lease behavior is unchanged.
