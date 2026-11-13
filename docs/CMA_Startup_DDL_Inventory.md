# Agent Manager startup DDL inventory and first retention slice

Source baseline: ChronoAIProject/cma develop 89a008a0d (2026-11-10).
Prepared for #1329; supports #1328 and M-OPS-02 #1554.

This inventories direct production DDL callsites under api/crates/cma/src,
excluding tests, legacy Gateway test adapters, role declarations and the new
explicit migration CLI. The constructors below are reached by MongoStores::open,
MongoSandboxStore::open_with_storage, or their service composition. A callsite
inventory is not a measured startup budget or proof that every configuration
executes every call. Indirect initialization helpers and startup data scans are
listed separately; qualify the selected deployment's actual call graph and scale
before moving a path out of startup.

## First safe slice

Remove activity's catch-all create-index-error → collMod fallback. A mismatched
TTL or permission failure must refuse startup without mutating current retention.
An exactly matching index still opens normally. Bootstrap creation stays as the
current behavior; this slice does not authorize changing retention or completing
all startup DDL extraction. The separate runner initially handles only the two
nonunique activity query indexes. No retention-changing runner step is approved.

Tests: fresh/matching index success; mismatched TTL leaves existing retention and
records unchanged; arbitrary create-index failure never emits collMod; retry after
restoring the matching configuration succeeds. Production preflight must compare
the configured and actual retention before releasing this stricter startup guard.
If they disagree, preserve the actual setting and obtain the retention decision;
never silently change data retention to make the process boot.

## Dependency and recovery inventory

| Path | Side effects / risk | Exit requirement |
| --- | --- | --- |
| store/activity.rs | Query indexes and TTL creation; catch-all collMod can change retention | First remove implicit TTL mutation; later explicit retention plan with backup/decision and compatible runtime verification |
| store/bindings.rs | Creates generation-scoped unique index then drops legacy global Agent index | Prove expected prior indexes, old-writer exclusion, partial-step recovery and constraints before planned contraction |
| store/schema.rs | Lists/creates collections and scans unsupported versions; exact legacy session exception | Preserve writer/guard parity; bounded read-only preflight only after all required structures are independently provisioned |
| store/runtime_routing.rs | Scans all retained Sandboxes, inserts original provider choices and activation marker | Preserve immutable provider identity; bounded checkpointed migration, receipts and unresolved exceptions before removing startup scan |
| store/native_warm_pools.rs and warm_pools startup | Collection/unique index preparation plus deployment/default authority checks | Preserve released runtime and retained home/pool authority; index migration is separate from pool readiness |
| store/usage_reports/schema.rs | Initializes reporting collections/indexes even when reporting is disabled | Separate provisioning and bounded verification without bypassing existing schema checks |
| Auth and custody adapters | TTL and unique indexes supporting sessions, credentials and cleanup | Retention/key/credential custody must not change incidentally during runtime boot |
| Remaining direct callsites below | Primarily idempotent query/unique/TTL index creation | Inventory exact prior/post state, mixed-version writers and bounded DDL budget per service-owned step |

## Direct DDL callsites

Paths are relative to api/crates/cma/src. Links pin the inspected baseline.

| Source | Declared effects |
| --- | --- |
| [document_artifacts/mongo.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/document_artifacts/mongo.rs#L30) | index creation |
| [store/activity.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/activity.rs#L74) | index creation; retention mutation; TTL declaration |
| [store/agent_access_requests.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_access_requests.rs#L134) | index creation |
| [store/agent_channel_usage/records.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_channel_usage/records.rs#L177) | index creation; unique constraint |
| [store/agent_commands/audit.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_commands/audit.rs#L55) | index creation |
| [store/agent_commands/records.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_commands/records.rs#L153) | index creation; unique constraint |
| [store/agent_lifecycle_operations.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_lifecycle_operations.rs#L153) | index creation; unique constraint |
| [store/agent_log.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_log.rs#L40) | index creation; unique constraint |
| [store/agent_task_status.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agent_task_status.rs#L17) | index creation |
| [store/agents.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/agents.rs#L99) | index creation; unique constraint |
| [store/auth.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/auth.rs#L149) | index creation; TTL declaration; unique constraint |
| [store/bindings.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/bindings.rs#L91) | index creation; index drop; unique constraint |
| [store/bot_setup_contexts.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/bot_setup_contexts.rs#L40) | index creation; TTL declaration |
| [store/bots.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/bots.rs#L62) | index creation; unique constraint |
| [store/checkpoint_objects/model.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/checkpoint_objects/model.rs#L195) | index creation; unique constraint |
| [store/checkpoint_objects/tombstones.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/checkpoint_objects/tombstones.rs#L33) | index creation |
| [store/checkpoints.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/checkpoints.rs#L42) | index creation |
| [store/directory.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/directory.rs#L121) | index creation |
| [store/event_workers.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/event_workers.rs#L36) | index creation |
| [store/message_feedback.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/message_feedback.rs#L45) | index creation; unique constraint |
| [store/native_prepared.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/native_prepared.rs#L41) | index creation |
| [store/native_warm_pools.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/native_warm_pools.rs#L37) | index creation; unique constraint |
| [store/operation.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/operation.rs#L80) | index creation; TTL declaration |
| [store/profiles.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/profiles.rs#L96) | index creation; unique constraint |
| [store/request_limits.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/request_limits.rs#L21) | index creation; TTL declaration |
| [store/resource_access/artifacts.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/resource_access/artifacts.rs#L238) | index creation |
| [store/resource_access.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/resource_access.rs#L206) | index creation |
| [store/sandbox_billing.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_billing.rs#L12) | index creation |
| [store/sandbox_controls/records.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_controls/records.rs#L73) | index creation |
| [store/sandbox_credentials.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_credentials.rs#L97) | index creation; unique constraint |
| [store/sandbox_events.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_events.rs#L33) | index creation; unique constraint |
| [store/sandbox_metric_bindings/mod.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_metric_bindings/mod.rs#L36) | index creation; unique constraint |
| [store/sandbox_mongo.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_mongo.rs#L248) | index creation |
| [store/sandbox_sessions.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_sessions.rs#L138) | index creation |
| [store/sandbox_usage.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandbox_usage.rs#L31) | index creation |
| [store/sandboxes.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/sandboxes.rs#L131) | index creation; unique constraint |
| [store/schema.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/schema.rs#L33) | collection creation |
| [store/suggestions.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/suggestions.rs#L20) | index creation; unique constraint |
| [store/trigger_definitions/indexes.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/trigger_definitions/indexes.rs#L105) | index creation; unique constraint |
| [store/trigger_definitions/launches/indexes.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/trigger_definitions/launches/indexes.rs#L109) | index creation; unique constraint |
| [store/trigger_definitions/launches/visitor_quota.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/trigger_definitions/launches/visitor_quota.rs#L18) | index creation; TTL declaration |
| [store/trigger_first_messages.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/trigger_first_messages.rs#L39) | index creation; unique constraint |
| [store/usage_reports/schema.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/usage_reports/schema.rs#L125) | index creation; TTL declaration |
| [store/warm_sandboxes.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/warm_sandboxes.rs#L29) | index creation |
| [store/workspaces.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/workspaces.rs#L88) | index creation; unique constraint |
| [store/worktrees.rs](https://github.com/ChronoAIProject/cma/blob/89a008a0d/api/crates/cma/src/store/worktrees.rs#L47) | index creation; unique constraint |

## Handoff and remaining gate evidence

Startup extraction remains incomplete until each selected store is provisioned
through a reviewed runner and boot uses bounded compatibility verification. The
current static inventory does not certify 90-day scale. Record at least ten
startups per migration stratum with every maximum below 60 seconds; report p99
only at 1,000 or more samples in that same stratum. Record exact candidate/source,
configuration, environment, time window, sample count and raw observations.
Retain corrupt records and diagnostics; do not weaken guards or repair data into
success. #1554 still requires complete reader/writer cells and two compatible
forward-repair drills on November 19/20.
