# CMA worker supervision and execution readiness

Refs #1740; builds on #1664. Local supervision does not establish production
journey acceptance or close #1470/#1486. External qualification belongs to
ChronoAIProject/chrono-probe#18.

## Capability inventory

| Worker | Local capability | Readiness | Owned children / recovery authority |
| --- | --- | --- | --- |
| `launches` | Advance admitted Launch steps | Required | Single discovery and up to eight executions; existing Launch ID, work lease and step/result reconciliation |
| `agent_commands` | Drive admitted Agent commands | Required | Single paginated discovery and eight executions; command ID, durable dispatch binding, claim/generation and terminal receipts |
| `sandbox_commands` | Drive admitted Sandbox controls | Required | Single paginated discovery and eight executions; durable command ID, lineage/authority and control receipts |
| `agents` | Schedule Agent lifecycle work and connection reconciliation | Required loop | Single discovery, up to fifty executions, one separately admitted reconciliation child; fresh durable Agent claims |
| `sandboxes` | Drive Sandbox lifecycle sweeps | Required | Four owned sweeps; existing per-resource durable authority |
| `workspaces` | Drive Workspace lifecycle sweeps | Required | One owned sweep; existing per-resource durable authority |
| `sandbox_reconciliation` | Adopt runtime bindings, detect runtime loss, admit recovery and reconcile orphan resources / maintenance | Required loop | One owned resumable batch; retained cursors, binding holds, recovery schedule, durable revision/identity fences and existing background budget/deadlines |
| `native_warm_pools` | Maintain spare native runtime capacity | Optional | One batch child, at most twenty targets; durable pool target lease/revision, planned identity and original provider UIDs |

Agent connection reconciliation remains single-flight under the shared background
Mongo budget. Its permit waits, pacing, thirty-second resumable operation contract
and outcomes retain the existing `cma_background_*` instrumentation. Reconciliation
child failure does not mean the Agent scheduling loop exited. Pool reconciliation
has its own child, so a long batch no longer blocks scheduler activity or shutdown
notification. The existing per-target provider/reconciliation contracts are retained.

Runtime reconciliation is a distinct worker from Sandbox lifecycle sweeps and
Agent connection reconciliation. Its loss removes local recovery-admission
capability, so it participates in readiness. Its scheduling loop ticks every
second while at most one batch follows the existing fifteen-second cadence.
A long batch or background pacing does not block loop activity. Both the batch
and its retained reconciliation state survive scheduling-future recovery;
shutdown drains that child under the existing coordinator deadline.

The inventory distinguishes execution scheduling from auxiliary work. Titles,
suggestions, prewarming/prepared-capacity replenishment, backfill, observations,
usage aggregation and other background maintenance are auxiliary workers outside this
supervisor. Their loss can reduce features, freshness or recovery capacity; this
change does not assert that these workers are supervised or that all application
features are healthy merely because `/ready` succeeds. Credential/channel service
workers also retain their existing ownership and shutdown contracts.

## Local recovery

One common supervisor owns the scheduler's state for its entire lifetime. A panic
or unexpected return of the scheduling future is observed as that future completes.
A suspended, cancellation-safe scheduling future is replaced after stall detection;
its effectful execution children remain owned and continue running.
The future is recreated after delays of one, two, four and eight seconds. Five
failures within a rolling sixty seconds suspend retries for sixty seconds; retries
then resume. Recovery never exits the process and never restarts during intentional
shutdown or after the shutdown sender closes.
The supervisor invokes the owned-state drain once on every intentional stop,
including a stop during recovery backoff. Existing drain deadlines still apply;
shutdown does not construct another scheduling attempt or admit replacement work.

Crucially, the unwind boundary excludes the `JoinSet`, in-flight IDs, queued IDs,
pagination cursor and local deferral deadlines. Existing children continue while
the scheduling loop backs off. Replacement scheduling uses the same state, joins
completed children and applies the existing concurrency limits. A new future must
drive a real scheduler select/work turn before local readiness returns. Construction
or a restart counter alone is not recovery evidence. This assumes panic unwinding;
a build configured with `panic=abort` terminates instead of executing this recovery.

Child operation panic follows the existing completion path and frees only its
local slot. Discovery grants no execution authority. All execution still enters
through the existing durable claim and result-reconciliation path. The supervisor
never deletes a claim, resets a generation, creates replacement commands, kills a
runtime, removes a home or changes active conversations.

A supervisor can observe a future completing or yielding pending; it cannot
preempt a CPU-bound future that blocks a Tokio executor thread. If other executor
threads can still serve health, the request-side freshness check detects the stale
loop. Total runtime starvation also prevents the HTTP probe from answering.

## Stall suspicion and operation boundaries

A one-second watchdog reads in-memory state using Tokio monotonic time. Actual
scheduler turns record last activity, phase start, expected wake and a bounded
waiting reason (`idle_scan`, `discovery`, `execution_slots`, `reconciliation`).
Discovery/reconciliation children additionally expose their monotonic ages. The
watchdog never refreshes activity. Health requests only inspect this state and the
existing cached Mongo ping; they issue no additional discovery or Mongo poll.

The loop becomes `suspected_stall` five seconds after its expected wake if it has
not made another turn. Required schedulers tick at most every second, giving a
six-second suspicion bound, with at most another watchdog interval for the metric.
Warm-pool scheduling ticks every five seconds and is optional. All supervised scheduling futures await only cancellation-safe timers, notifications
and JoinSet joins. On loop suspension, the supervisor drops only this scheduling
future, retains its state/children and applies the same bounded restart policy.
An overdue discovery child alone never authorizes replacing or cancelling an
execution child. Actual resumed scheduling clears suspicion. Long user
reasoning, waiting for user action, full execution slots, zero due work, background
permit denial and dependency backoff do not require completed business work to
prove loop health. Readiness never uses the age of the last completed execution.

Only read-only Launch/Agent/command discovery gets a new five-second deadline.
Expiration releases the discovery child through normal completion handling;
pagination and other children remain intact, and the next normal tick can retry.
Query timeout is reported as an existing bounded unavailable/error outcome.
No new total execution, inference, provider-write or reconciliation deadline is
introduced. Existing per-operation deadlines/fencing remain authoritative.

Local cancellation, lease expiry or scheduler replacement does not establish
whether a provider or Mongo write committed. Unknown outcomes must be reconciled
against the original durable identity and provider receipts. They must not be
resent merely because a local task ended. In particular, existing long work leases
(for example the Launch worker's fifteen-minute default), dependency outages and
unknown provider outcomes can prevent durable progress within thirty seconds.
Those are explicit unmet work-recovery dependencies; a faster local restart does
not satisfy that acceptance target.

## Readiness and metrics

`/livez` remains process liveness. `/ready` now requires both the existing fresh,
successful cached Mongo check and all required local scheduling capabilities.
The response retains `dependency`; its top-level `reason` is `store` for dependency
failure, `workers` for local capability loss, or null. A bounded `workers` array
contains capability requirement, state, phase/phase age, actual activity age,
expected wake, waiting reason and discovery/reconciliation age. Optional warm
capacity does not remove the whole process from traffic after startup; the
existing exact-image startup pool gate still applies before serving.

States include `starting`, `running`, `suspected_stall`, `backoff`,
`restart_limited`, `draining` and `stopped`. No deployment setting was added.
Existing kubelet and Gateway probes still use `/ready`; deployment comments
specify its additional execution-capability semantics. Probe thresholds are
unchanged: the local detection bound is separate from kubelet/LB traffic-removal
latency. Dependency failure or suspicion does not trigger a liveness restart.

Existing #1664 metrics remain. New metrics use only bounded worker/reason labels:

- `cma_worker_execution_available{worker}`: local loop capability, independent of
  Mongo readiness and durable business progress.
- `cma_worker_supervision_exits_total{worker,reason="panic"|"exit"|"stall"}`: unexpected
  scheduling-future completions, excluding intentional shutdown.
- `cma_worker_restarts_total{worker}`: replacement attempts after backoff.

The existing alive, heartbeat and queue metrics also cover Sandbox and Workspace
scheduling. IDs, provider errors and task payloads never become metric labels.

## Qualification

See [local validation](investigations/cma-worker-supervision/validation.md).
Record three independent times for each injected fault: observation/suspicion,
resumed safe scheduling, and a new confirmed durable work result. Unit tests with
paused monotonic time prove scheduling bounds and ownership invariants; they are
not measurements of deployed wall-clock recovery or external effects.

Deployment qualification must attach the exact source SHA plus dirty-tree patch
and file digests, environment/runtime versions, effective non-secret configuration,
raw fault/test/receipt artifacts and API/runtime candidate image digests. Verify
unknown-commit and stale-writer regressions against a failpoint-enabled Mongo
replica set, then exercise isolated provider journeys while preserving the active
Sandbox/runtime/home/conversation state. A missing candidate digest, remote
receipt or measured work-recovery result remains an unmet gate.
