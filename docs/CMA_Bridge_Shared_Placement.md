# Shared parent Bridge placement

Milestone issue #864 adds two explicit trusted collaboration placements. They
remain subject to backend verification and the live Agent continuity gate #850;
the dashboard choices remain disabled until those gates pass. Source changes or
SDK support alone do not establish runtime, platform, cleanup or continuity proof.

| Placement | Selected parent | New allocation-owned children |
| --- | --- | --- |
| `workspace_per_partition` | Exact Sandbox | One Workspace, Worktree and Agent |
| `worktree_agent_per_partition` | Exact Sandbox and Workspace | One Worktree and Agent |

Both placements require a pinned Agent profile. Workspace creation additionally
requires a pinned Workspace profile. The live parent's immutable snapshots are
retained in the allocation. A configuration edit affects new partitions; an
existing partition retains its current allocation and original profiles. An
explicit binding reset creates a new generation and new child identities. It
retains the previous generation and never moves or removes sibling work.

## Shared execution and authority

The required `shared_trust` value is
`all_participants_share_filesystem_processes_and_credentials`. There is no default
consent. All admitted senders execute inside the selected Sandbox's filesystem,
process and credential boundary. Worktrees and Agent homes avoid conversation
and path collisions. They provide no security isolation: shell commands and
tools can read sibling state. Everyone admitted by the Bridge must be trusted
with that whole execution scope. Replies in a chat may disclose information
learned from shared files or credentials.

Every setup and deferred allocation step requires both current Bridge management
and a current `ManageSandbox` decision for the selected parent, using the actual
retained configuring actor and credential. Organization roles and whole-Sandbox
sharing policies remain effective. A narrow Agent share cannot grant parent
management. The parent owner must exactly match the Bridge owner.

Only a newly created partition Agent receives immutable `ChannelBridge` Agent
creation provenance. Reusing a manual or Trigger Sandbox does not change that
Sandbox's origin. An already existing manual or Trigger Agent does not gain
Channel tools by attaching it to a Bridge.

## Capacity and compatibility

`capacity` requires both `keep_other_workspaces` and
`keep_other_agents_per_workspace`, each from 1 through 15. These free slots are
retained for unrelated owner work under the parent's immutable limits. Live
children and all pending Bridge child reservations count together, including
reservations that have not created runtime resources. Bridge partition limits
also apply. Retired allocations continue consuming capacity until exact child
cleanup is reconciled; resetting a binding does not release capacity.

The authority checks parent identity and ownership, pinned profiles, immutable
limits, effective service scope, model support, skill availability and runtime
capabilities. Unsupported parent combinations return `bridge_incompatible_target`
(HTTP 409). The initial adapter supports ChronoAI-managed model access and
requires every selected model/skill service to already be in the parent's
credential scope. It never expands credentials or changes Sandbox environment,
installation commands, limits or model access to fit a partition. A new Workspace
profile must have an empty automatic Agent-profile list so ordinary reconciliation
cannot create additional children outside the partition reservation.

New Workspaces have deterministic `channel-<digest>` names. Worktree creation
increments the existing Workspace counter under its transactional resource fence
and reserves the exact `wt-N` path and `cma/wt-N` branch. Existing names, paths and
branches veto a collision. Runtime preparation, setup and Git worktree mutation
use the supervisor's shared operation lock and durable operation journal. Agent
turns keep the runtime's existing concurrency behavior.

## Recovery boundaries

The allocator retains exact source, generation, resource IDs, original operation
IDs and startup plan. It revalidates the original current authorities before each
runtime effect. An unknown outcome is retried using the same operation and
resource identity. A real worktree/home/open result and transactional tool-binding
fence precede allocation readiness.

Only existing manual/Trigger roots with ordinary lifecycle provenance are supported.
Known Channel-created roots are refused, including roots found through retained
allocation admission even if a creation sidecar is missing. They require a
separate original-human lifecycle composition before reuse can be enabled.

Reused roots keep the ordinary Sandbox lifecycle and credential workers. No
`sandbox_admission` or Channel root marker is written. A paused parent's initial
child reservation requests the shared ordinary revival path, then waits for a
fresh authority and active runtime. Ready-child revival integration and exact
allocation-child cleanup reconciliation have separate authority boundaries.

A queued Channel message for a Ready child can schedule revival of its exact
paused ordinary parent. The scheduling transaction checks the current Channel
execution grant, retained configuring human's current `ManageSandbox` authority,
original allocation and child provenance, and the unchanged command at the head
of the Agent queue. It preserves the command receipt and performs no runtime IO.
The ordinary worker restores the existing parent and conversations; the common
command worker then dispatches the original queued message. The child's reply
tool remains available during that active turn. Manual and Trigger siblings
retain their original eligibility and conversations.

Once child setup starts, direct pause, ordinary lifecycle scheduling, and
automatic image rotation wait until setup has reached Ready. These transitions
share the parent transaction fence with startup, so a pause cannot strand a
partially created child that still needs an active runtime.

Disposable Mongo tests cover both placements under manual and Trigger parents,
concurrent revival, queue order, revoked authority, failed audit rollback, and
reply-tool invocation before turn completion. Their runtime uses the in-memory
hosted backend. Real runtime-process, live platform and upgrade/rollback evidence
remain pending. Exact allocation-child cleanup, restore-aware deletion and
capacity release remain unfinished.
