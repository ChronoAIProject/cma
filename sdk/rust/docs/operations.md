# Operations

<!-- nav: 9 Operations -->

`read_defaults()` returns deployment defaults available to the authenticated
caller. `admin()` uses public capability-gated routes for Sandbox and profile
inventory, durable events, runtime inventory, activity, and platform settings.
The SDK has no private operator channel.

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

`checkpoints.interval_seconds` controls periodic snapshots globally (60 through
86400 seconds, default 1800), using `Section<Checkpoints>`. Workers reread the
setting without a restart. Manual and inactivity pauses still require a fresh
completed snapshot. Only the latest completed snapshot is retained; replacement
commits before old archives are removed. Older APIs can omit the section.

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
