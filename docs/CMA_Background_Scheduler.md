# Agent reconciliation background scheduler

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
