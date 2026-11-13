# Allocated usage report failure, 6 November 2026

FINDING: the measured-collection switch does not gate report admission or workers.
Production demonstrated a successful owner report while collection was off.
The observed outage combines a cumulative-read pressure circuit with retained
maximum-size reservations that block ordinary refreshes after two reports.

## Production evidence

Read-only VictoriaLogs Kubernetes proxy GETs selected production cluster
`chronoai-prod-asia-northeast1-gke`, namespace `chrono-bot`, API container, starting
at 03:00 UTC (11:00 SGT). API source is `b31a88603`; the reported web revision is
`597318209`; its report hook, error mapping and dashboard source match the
inspected API-checkout versions. The live Deployment and ConfigMap have no usage metrics environment
keys; the default is false. Production log level is INFO. No database query,
Pod exec, source permission change, runtime action or configuration write occurred.
Only route templates, timestamps, statuses, durations and fixed diagnostic
categories are retained here. Raw identifiers and customer content are excluded.

| SGT time | Observation |
| --- | --- |
| 11:15:15 | Owner POST `/api/v1/usage/metrics/reports`: 202; first 256 MiB reservation |
| 11:15:17 | Report generated successfully in 960 ms, `source_health=source_unavailable`; owner poll 200 |
| 11:15:18 | Owner report Sandbox page: 200 |
| 11:15:31 | Admin POST `/api/v1/admin/usage/metrics/reports`: 202; total reservations 512 MiB; queue count 1 |
| 11:15:34–38 | Admin poll: three 200 responses while pending, approximately two seconds apart |
| 11:15:39 | Admin report generation failed after 6,630 ms; reporting workers began logging budget exhaustion |
| 11:15:41 | Admin poll: 503, 137 ms response latency |
| 11:15:39–11:16:08 | 31 worker queue-budget failures, consistent with the 30-second circuit |
| 11:16:22 / 11:17:22 | Admin report POST: 429 |
| 11:16:24 / 11:17:24 | Owner report POST: 429; retained owner report Sandbox page remains 200 |

The captured window had two admissions, one successful and one failed generation,
one report-poll 503, four create 429s, and seven successful report/member GETs.
No `reporting store unavailable` or checkout-circuit warning occurred in this
interval. INFO logs omit the per-read phase timer, so they do not establish which
individual projection phase exceeded its cumulative threshold.

## Deployed code path

References below are to the deployed `b31a88603` source, before this fix.

- `api/crates/cma/src/sandbox_metrics/mod.rs:76`: report workers start regardless
  of collection; only observer/rollup/replay/diagnostics workers depend on it.
  `source.rs:169` returns empty unavailable measurements with no source client.
  A ready report can legitimately carry `source_unavailable`.
- `api/crates/cma/src/store/usage_reports/budget.rs:29`: dropping `ReadTimer`
  opens a 30-second circuit when the entire logical read exceeds two seconds.
  `inventory.rs:23` includes multiple queries and decoding; allocation and metric
  rollup reads also include multiple operations. There was no checkout warning,
  leaving this cumulative timer as the circuit-opening path in the deployed code.
- `api/crates/cma/src/store/usage_reports/mod.rs:105`: subsequent guarded reads
  return `MetricsError::Busy`. `sandbox_metrics/report.rs:26` converts a generation
  error into a failed report, losing its original error category in existing INFO
  telemetry. `usage_reports/reads.rs:79` maps a failed poll to `Unavailable`;
  `layer1-public/src/rest/sandbox_metrics/mod.rs:162` maps it to 503
  `usage_unavailable`. The exact failed generation phase is not logged.
- `api/crates/cma/src/store/usage_reports/admission.rs:60`: admission sums reserved
  bytes per principal across owner/admin scopes. The defaults reserve 256 MiB per
  report against a 512 MiB principal cap. `work.rs:169` finalizes without reducing
  that reservation, including failed reports, until ten-minute expiry and cleanup.
  After the circuit elapsed, the two recorded reservations explain both routes'
  429 `reporting_budget_exceeded`; the queue/global limits were not reached.
- `web/src/usage/metrics/useReport.ts:96` honors the report's two-second polling
  interval, stops on failure and refreshes no faster than once a minute while
  visible. The observed requests follow that policy. The hook preserves a prior
  successful report on transient error, explaining allocated data below the alert.
  There is no allocation-endpoint fallback.

## Mitigation and correction

No existing configuration alone corrects both faults while collection remains
off. Raising `CHRONO_USAGE_METRICS_TENANT_MIB` from 512 to 4096 would increase the
per-principal allowance from two to sixteen full 256 MiB reservations, subject
to the unchanged 2048 MiB global cap (eight full reservations). It only delays
exhaustion and does not correct the cumulative timer; it is not a sufficient
mitigation. Enabling collection neither fixes these bugs nor respects the owner
choice, and monitoring access has not been granted. No production config changed.

The fix skips measured reads when collection is disabled, retains explicit null
measured values, counts terminal cache bytes rather than unused work reservations,
and measures connection pressure independently of multi-query projection time.
Lease fencing, genuine pressure limits, authorization, report expiry and measured
aggregation remain in place. Added diagnostics distinguish generation errors and
admission limits without customer identities. No new durable enum value or field
is added; old and new readers share the existing report/cache schema.

Existing terminal reports retain their older conservative reservations until
ordinary expiry cleanup, at most the remaining ten-minute logical lifetime plus
the worker cleanup interval. The API fix needs deployment by the lead before any
production recovery can be claimed. No web files changed.


The local fault-injection regression reproduces `Busy` with the deployed timer
and passes with the fix. See [validation.md](validation.md) for commands, coverage
and local environment limits.
