# S5 integration handoff — 2026-11-11

Branch: `tl/cma-log-segments/capacity-recovery`. Core state-machine checkpoint
`01700d909` is pushed; subsequent source checkpoint follows. Develop freeze stays on.

Consumed S1 `5ed7eaae1` and S4 `453b41742`, isolated root registrations
`5f20045a6`/`ec6002898`, then S1 reader/registry update `0d1da3bb3` by normal merge.
No evolving integration tip was imported.

## Shared registrations requested from S2

- Store root: `pub(crate) mod agent_capacity_recovery;` for the owned directory.
  Its children are `source`, `admission`, `authority`, `records`, and `status`.
  This is a focused transaction API, not a second transaction framework. The
  earlier possible MongoCapacityRecovery wrapper is not required by this checkpoint.
- Integration-test root: `mod agent_capacity_recovery_tests;` for the owned test
  file. Children cover parent pacing, the worker flag, and source format/fences.
- AgentWorkerConfig: `pub parent_not_ready_pacing_enabled: bool`, default false.
  S5's connection module defines `with_parent_not_ready_pacing(bool)`; S2 wires
  `CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED=false` through config/bootstrap.
- Allocate the bounded `agent_capacity_recovery_work` carrier in migration/schema,
  backup and grants inventories. This is recovery work, not a gate or receipt;
  existing S4 Admission and Publication remain sole admission/publication schemas.
  S5 supplies encoding/read validation under `records.rs`. No production DDL or
  implicit enablement is authorized by this source handoff. Carrier work is at
  most 16 KiB serialized JSON and 32 KiB for its BSON envelope. Stable `_id` is the
  original creation/failure recovery key. Before activation, add the approved
  schema/index verification and due-work discovery via the shared migration runner.

## Implemented source boundary

`admission::persist_admission(store, AdmissionRequest, session)` receives current
resource authorization, original request, S4 CapacityRecovery Admission and optional
opaque BootstrapPlan. It rereads exact policy and parent/Agent ancestry in the
existing transaction, captures the source and original failure through S1, prepares
an empty-batch S4 publication, inserts bounded non-ready recovery work, fences the
parents and calls S4 persist for the sole Agent CAS. The caller must abort on every
error. No transaction start/commit/retry or input transport is hidden in this helper.
Legacy without a certified staged BootstrapPlan refuses. The process service's
independent default-off flag and both durable gate proofs are still required.
The public lifecycle/HTTP path, accepted-work Mongo adapter and restricted native
runtime adapter are remaining S5 implementation; this helper alone is not delivery.

`status::read` consumes existing fresh resource authorization and rereads exact
ancestry/policy in the caller's transaction. It exposes bounded disposition/work
observations; an absent carrier is not KnownAbort and a carrier grants no runtime
permission. The current helper explicitly refuses Completed until completed-publication
receipt verification is wired; it cannot present successful recovery from a row
alone or authorize output acknowledgement.

Five owned consumer families now branch explicitly on the captured envelope.
Legacy uses its unchanged validator. Segmented checkpoint capture/publication,
maintenance, unpublished-generation checks and runtime reconciliation require S1
captured tail/through/projection/maxima and never fall back after a proof error.
Runtime reconciliation calls the same S4 prepare/persist boundary with stable
request identity. Write fences bind the exact schema/incarnation/head envelope;
parent/command/session checks are retained.

## Validation and remaining shared dependencies

The first integrated core test attempt held heavy-0 with two jobs and failed
before tests ran on S3's missing UsageFold derives and projection-identity export.
S4 collection-handle references and S1 transaction-scope proof remain reported by
the integration owner. No repeat heavy run is warranted until a dependency changes.
All 34 core recovery tests and the added real-Mongo contracts need execution after
those dependencies land. Full fmt, Clippy, Mongo crash/unknown/no-replay/consumer
bounds, strict file size and complete-history secrets remain PR readiness gates.
No PR-ready, runtime recovery or activation claim accompanies this source handoff.


## Recovery connection follow-through

The production worker must keep completed recovery on the restricted reconnect
path after a crash between durable completion and registry activation. Ordinary
PublishConnection may replay pending managed context/settings and schedule queued
first input. It must not be used wholesale for an Agent carrying accepted recovery.
The existing ConversationChannel confirms only native conversation identity; the
runtime adapter must supply the required native-state/output evidence explicitly,
not derive a new Ready claim solely from the work row. These are remaining S5
worker/composition obligations before the full PR, including a test for lost
completion acknowledgement while the Agent remains live (in addition to the
existing later-head/owner-stop test). No gate may activate this source checkpoint.

Cleanup after the mandatory machine-pressure relay: removed the owned
api/target/seg-3 and temporary GitHub transport files, freeing 0.71 GB (0.66 GiB).
No owned Mongo container or volume exists. Disk readback after cleanup: 35 GiB.
No further heavy gate started while dependencies and pressure remain unresolved.
