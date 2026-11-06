# Continued usage-report errors after #1386

FINDING: a single successful 252 ms Mongo connection checkout opened the
reporting circuit at 14:09:36.611 SGT on 6 November 2026. The 250 ms soft
threshold immediately rejects work for 30 seconds. The filtered admin report
then failed with `Busy`, which persisted as a failed report and produced a poll
503. The same circuit rejected the previous ready report's inventory with 429.
Neither measured collection, retained quota nor a stale lease caused this event.

## Evidence and request sequence

The live API Deployment uses image digest
`sha256:418c1cf0bdb568963a7547534945c6d9c2d95883bbd7e6a12123ceb37f0a8735`,
corresponding to the reported production API revision `8d2c8c109` (#1386).
The Deployment and ConfigMap contain no usage metrics environment keys, so
measured collection remains off. The API log level is INFO.

Read-only VictoriaLogs Kubernetes API proxy GETs selected production cluster
`example-cluster-02`, namespace `chrono-bot`, API container, from
13:20 SGT through the incident. A direct `kubectl logs` read of the current API
pod corroborated the exact circuit and generation-failure lines. The requested
November 6 latency-method note was absent; the same existing proxy service and
read-only method from the November 4 investigation were used. No production
DB connection/query, Pod exec, restart, configuration write or user action was
performed. Only allowlisted diagnostic values and templated paths are retained.

| SGT time | Route/event | Result |
| --- | --- | --- |
| 14:08:54.280 | Owner POST `/api/v1/usage/metrics/reports` | 202, 166 ms |
| 14:08:55.677 | Owner report generated | Success, 188 Sandboxes, 1,323 ms |
| 14:08:56.755 / .995 | Owner poll / Sandbox page | 200 / 200 |
| 14:09:08.555 | Admin POST `/api/v1/admin/usage/metrics/reports` | 202, 68 ms |
| 14:09:11–20 | Admin pending polls, about two seconds apart | Five 200 responses |
| 14:09:21.821 | Full admin report generated | Success, 373 Sandboxes, 11,467 ms |
| 14:09:22.786 / 23.039 | Full report poll / Sandbox page | 200 / 200 |
| 14:09:33.048 | New admin selection POST `/api/v1/admin/usage/metrics/reports` | 202, 171 ms |
| 14:09:35.433 | New report poll | 200 pending, 236 ms |
| 14:09:36.611 | Reporting database circuit opened | `operation="checkout" elapsed_ms=252 failed=false` |
| 14:09:36.806 / .815 | Report generation failed / finished | `Busy`, `success=false`, 2,313 ms |
| 14:09:37.617 | New report GET `/reports/{report}` | 503 `usage_unavailable`, 71 ms |
| 14:09:37.821 / 49.543 | Previous report GET `/reports/{report}/sandboxes` | 429 `reporting_budget_exceeded`, 71 / 175 ms |
| 14:10:49.611 | Subsequent admin selection POST | 202, 242 ms |
| 14:10:56.715 | Subsequent filtered report generated | Success, 36 Sandboxes, 6,215 ms |
| 14:10:57.038 / .353 | Filtered report poll / Sandbox page | 200 / 200 |
| 14:12:49.493 | Subsequent admin refresh POST | 202, 91 ms |
| 14:12:54.468 | Refreshed report generated | Success, 36 Sandboxes, 4,095 ms |
| 14:12:56.239 / .522 | Refreshed report poll / Sandbox page | 200 / 200 |

The 14:05–14:15 window contains five admissions, four successful generations,
one failed generation, eighteen successful report/member GETs, one poll 503 and
two member-page 429s. Twenty-nine worker queue refusals span 14:09:36.815 through
14:10:04.339, consistent with the single 30-second circuit. There is exactly one
circuit-open diagnostic in the inspected 13:20-onward interval and no store-error,
admission-budget or finalization error in the incident window.

The failures are on the administrator route; principal IDs were discarded. The
reported `active` selection and 38-running card come from the owner's screenshot;
HTTP traces do not log request bodies. The prior report's 373-Sandbox cohort is
confirmed by backend coverage logs. The later filtered report covers 36 Sandboxes;
this does not imply the screenshot and later observation have identical lifecycle
state. The previous report's successful summary remains visible because the web
retains cached data when a new selection fails. A page-level 429 explains the
separate table alert. Observed polls honor the two-second interval; no rapid retry
loop appears.

## Exact deployed code path

References are to API `8d2c8c109`, before this follow-up.

- `api/crates/cma/src/store/usage_reports/budget.rs:28` opens the circuit for
  **any** successful checkout above 250 ms. `:46` sets the deadline 30 seconds
  ahead. A completed 252 ms operation is treated as an outage with no persistence
  or failure requirement.
- `api/crates/cma/src/store/usage_reports/mod.rs:105` returns `MetricsError::Busy`
  while that deadline is active. Inventory, allocation-rollup and cached page
  reads check it. The exact subsequent generation phase is not logged, but the
  generation error is explicitly `Busy` 195 ms after the circuit opened.
- `api/crates/cma/src/sandbox_metrics/report.rs:24` finalizes a generation error
  as a failed report. `api/crates/cma/src/store/usage_reports/reads.rs:79` maps
  polling that failed report to `Unavailable`.
- `api/crates/cma/src/store/usage_reports/reads.rs:120` checks the circuit before
  reading an existing report's page. The public mapper in
  `api/crates/layer1-public/src/rest/sandbox_metrics/mod.rs:161` maps `Busy` to
  429 `reporting_budget_exceeded` and `Unavailable` to 503 `usage_unavailable`.

The logs prove the successful checkout delay and its effect. They do not separate
connection setup, scheduler delay and waiting behind other pool work; none of
those distinctions is needed to reproduce the single-sample circuit defect.

## Earlier reservations and restart leases

Earlier traffic does show old maximum reservations: at 13:32:30, 13:36:50,
13:36:52 and 13:38:55, admission diagnostics report 768 MiB globally and 512 MiB
for the requesting principal, with zero queued jobs. Those older reports were
created at 13:31 and retain predecessor reservations until ordinary expiry.

At 14:02:13 a new admission logs exactly 256 MiB total reserved, proving **zero
pre-existing reserved bytes** at that time. At the incident's 14:09:33 admission,
the total is 270,986,499 bytes, including the new 268,435,456-byte work reservation;
only 2,551,043 cached bytes pre-exist. Both are below admission limits. No direct
Mongo read was made or needed to establish these reservation totals.

ReplicaSet creation times confirm replacements at 13:26:12, 13:30:13 and
13:49:34; only the last remains active and its container restart count is zero.
Report and worker leases expire after 70 seconds. Successful reports after these
replacements demonstrate recovery; the filtered report starts approximately
1.45 seconds after admission and finishes as failed, without a finalization error.
A stale lease cannot explain its correlated 252 ms checkout circuit or cached-page
429. Durable lease documents themselves were not queried in production.

## Fix forward

Retain the 250 ms soft threshold but require three consecutive slow successful
checkouts, each within one second of the preceding sample, before opening the
circuit. A healthy checkout or an expired streak resets the evidence. Failed
checkouts and individual checkout/connection-hold hard stalls above two seconds
still open it immediately. The four-connection pool, query/report deadlines,
admission byte limits, authorization and lease fences remain in place.

This changes only process-local pressure observation. No configuration, public
contract, durable field or enum value is introduced. Measured collection remains
off; this follow-up does not alter #1386's allocation-only path or byte accounting.


The Mongo regression reproduces the exact `(503, 429)` response pair when only
the deployed #1386 pressure observer is restored, including its logged `Busy`
generation error. See [validation.md](validation.md) for the fixture and gates.
