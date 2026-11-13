# Agent reconciliation background scheduler

## Reserved Sandbox recovery capacity

`CHRONO_SANDBOX_RECONCILIATION_RESERVED_LANE_ENABLED` is a strict boolean,
default `false`, independent of the Agent reconciliation and command pacing
switches. The disabled path retains the shared background budget and its
foreground admission rule. When enabled at process composition, the Sandbox
reconciler reserves one permit from the two-permit shared budget and owns an
independent one-permit lane. Shared workers retain one permit even while
foreground requests are active. The total remains two background chains,
including when there are no foreground requests. A busy or invalid reservation
fails startup instead of silently falling back to shared admission.

The same switch replaces the outer 15-second batch tick with the background
work's next-run deadline. A denied or yielded attempt resumes after the existing
187–250 ms jittered delay; failures retain the existing exponential 2–60 second
backoff with 75–100% jitter, and completed sweeps retain the jittered 15-second
interval. The disabled path keeps its original 15-second tick. Supervision
heartbeats continue during work, only one owned attempt runs at a time, waiting
shutdown exits immediately, and shutdown drains an in-flight attempt without
cancelling it. The 30-second work deadline and all sweep, operation, binding
opportunity and hold bounds are unchanged.

The reservation lasts through all owned reconciliation attempts and returns
only when the reconciler's background work is dropped. It does not authorize
runtime adoption, recovery, reclamation, or a write: all existing durable
identity/revision checks, binding holds and cleanup proofs remain mandatory.
No persistent state, reader format, or migration is introduced. Rollback sets
the switch to `false` and restarts the API through the release owner.

Ship disabled. The release owner captures and compares background admission,
slot occupancy, full-sweep and binding-pass wall times, completed recovery,
Mongo and native HTTP checkout wait, route latency, CPU, readiness and restarts
before activation. Existing production baseline evidence is recorded in #1791;
local scheduling tests do not establish a production latency improvement.
Track `cma_background_attempts_total`, `cma_background_deferred_total`,
`cma_background_pending_since_timestamp_seconds` and
`cma_background_completion_timestamp_seconds` for `sandbox_reconciliation`,
plus `sandbox maintenance sweep completed` and phase-progress logs. The initial
production targets in #1791 are a full-sweep p95 within 15 minutes and a binding
pass within three minutes under comparable load. Reserved-lane shared-budget
denials should be zero. Activation and its one-hour observation window belong
to the release owner; default-off merge is not production verification.

Binding-refresh rules before retained tail work are unchanged. Changing tail
fairness requires the conditional measurements in #1791. Policy gating of
runtime updates, optional provenance backfill cadence, and hold causes remain
outside this scheduling change. Roll back on the existing Mongo/native HTTP
checkout, route latency, readiness or restart gates, or lost recovery progress.

## Agent reconciliation

The Agent Manager scheduler performs two independent jobs: due Agent work and
connection reconciliation. Due work keeps its existing one-second discovery
cadence. Reconciliation checks durable live-Agent candidates and repairs a lost
process attachment when the durable eligibility and lease fences allow it.

The deployed read-only attribution found about 652 reconciliation sweeps in one
hour, 32,671 `recovery_deferred` observations, and about 6.69 detached-candidate
reads per second without resulting recovery. The same baseline measured 5.81 CPU
cores, 100.26 Mongo checkouts per second, and 137.10 ms mean checkout wait. These
figures rank repeated read work; they do not claim that reconciliation consumed
that percentage of CPU.

## Event-driven mode

`CHRONO_AGENT_RECONCILIATION_EVENT_DRIVEN_ENABLED` is a strict boolean and
defaults to `false`. The disabled path retains the existing one-second
reconciliation sweep. With the setting explicitly enabled, reconciliation runs
once at startup, wakes when durable parent readiness changes, and keeps a
30-second fallback sweep for missed notifications or process-local loss. The
fallback is a bounded liveness guarantee, not an authority grant.

The mode changes reconciliation cadence only. Durable candidate discovery, fresh
claims, lease and revision fences, recovery eligibility, idempotent scheduling,
and restart behavior are unchanged. A readiness notification is only a hint;
the reconciliation query still reads current durable state before scheduling any
recovery. Due Agent work continues on its existing scheduler and is not delayed
by this setting.

## Rollout and rollback

The release owner enables the setting through a fenced API process rollout after
capturing a baseline. Compare reconciliation sweeps per minute, candidate and
`recovery_deferred` counts, scheduled recoveries, retry reasons, Mongo checkout
rate/wait, CPU, readiness, HTTP errors, and completed recovery latency. Use
aggregate labels only; retain no request, Agent, Sandbox, credential, or payload
identifiers.

Disable the setting and roll the API process when the setting is no longer
needed. Roll back immediately on missed recovery progress beyond the 30-second
fallback plus the normal recovery budget, increased stale or duplicate claims,
lease/revision conflicts, readiness regression, Mongo errors/timeouts, or the
existing health and latency triggers. The disabled path is the compatible
one-second sweep and requires no data migration.

This is a slice of #1421. Empty-queue coalescing across the other schedulers and
multi-replica correctness remain separate work under the parent issue and #1356.

## Dispatched command reconciliation

`CHRONO_COMMAND_RECONCILE_PACING_ENABLED` is a strict boolean, default `false`.
It is independent of Agent connection reconciliation and of bounded admission
cohorts, including admission at 0%. With the switch off, discovery does not read
Agent heads, dispatched reconciliation returns zero delay, and failures receive
no additional cooldown.

When enabled, each discovery page reads at most 50 projected Agent heads in one
nontransactional query. A previously folded command can be skipped only while
its log cursor, live status, runtime generation, active message turn and lease
state still satisfy the domain predicate. Missing, malformed, unknown-schema or
unavailable heads fall back to the complete reconciliation path. Head lookup has
a 500 ms server limit and a one-second worker timeout. Lookup failures warn
without identifiers or raw error text and increment
`cma_command_reconcile_heads_errors_total`.

Every process first reconciles each dispatched command, then audits unchanged
commands every 300 seconds. The process-local caches are bounded to 4,096
entries and disappear on restart; a missing cache entry requires reconciliation.
A successful nonterminal fold has a one-second cooldown outside execution
slots. Failed dispatched attempts retry after 250 ms, doubling to a five-second
cap; success resets the failure count. Queued-command cadence is unchanged.
Head hints never authorize a write or manufacture a terminal state. All state
changes and post-execution settlement retain their authoritative evidence path.

`cma_command_reconcile_total{outcome}` uses only `skipped_current`, `unchanged`,
`advanced`, `settled`, and `error`. Successful store outcomes count after commit;
transaction retries are still fully represented in scan accounting. Compare
historical `command_admission` reads against the sum of `command_admission` and
`command_reconciliation`, including scans, documents, bytes and scan duration.

Ship disabled and collect attribution plus the dispatched nonterminal count N
and current-cursor count U through the release owner's authorized evidence
path. Enable only after reconciliation accounts for the majority of the prior
admission scans and U is at least one. Use the same image for the controlled
flag rollout, preserving healthy-session republishing and the existing
15-minute and full-hour observation gates. Report all-caller scan bytes, Mongo
checkout waits, route latency and CPU alongside reconciliation results; local
measurements do not establish a production latency improvement.

Rollback sets the switch to `false` and rolls the API, or restores the prior
image. Trigger rollback on terminal-latency regression, rising reconciliation
errors, or existing checkout/readiness/circuit-breaker gate failures. There are
no persistent fields, indexes or migrations. API rollout does not stop Sandbox
runtimes or interrupt a turn that its owner has not stopped. Production rollout
and the full-hour evidence in #1785 remain release-owner gates.
