# Channel operational signals and alert definitions

CMA emits these observations through its existing `tracing-subscriber` text
formatter, with ANSI color disabled so field queries are stable. The container
log sink stores the line in Cloud Logging
`textPayload`; it is not a JSON `jsonPayload` event. The checked-in
[`channel-alerts.json`](operations/channel-alerts.json) contains eleven concrete
Google Cloud Monitoring `conditionMatchedLog` policy definitions for that format.
They are **disabled**, have no notification destinations, and are not installed
by the CMA manifests. This change does not apply cloud policy or require a new
exporter, permission, role, index, collection, or worker scan.

The log observations reuse existing durable reads and current authority checks.
They never authorize a retry, release a lease, confirm delivery, or change a
retained receipt. Recovery remains governed by
[Channel lifecycle recovery](CMA_Channel_Lifecycle_Recovery.md) and the public
audited recovery commands.

## Emitted format and bounded fields

An example using the actual formatter shape is:

```text
INFO cma::bridge::operations: channel_health operation="pending" worker="child_cleanup" age_ms=360000 held=true overdue=true work_id_sha256="..."
```

Only fixed `operation`, `worker`, `traffic`, `outcome`, `reason` and boolean
fields are suitable as dimensions. Numeric durations/counts are sample values.
`command_id`, existing `outbox_id`, and `work_id_sha256` are correlation fields;
**never extract them as metric labels**. Do not use sender, bot, route, owner,
provider identity, callback body, prompt, tool output, token or credential as a
label. The new observers receive typed records or a hashed durable work identity
and do not serialize content, arbitrary errors, credentials or provider replies.

| Operation | Observation | Meaning and limit |
| --- | --- | --- |
| `ingress` | Existing fixed acceptance/failure `outcome`, `elapsed_ms` | Covers failures before durable acceptance. `acceptance_unknown` is distinct from definite refusal. No unauthenticated request identity enters these event fields. |
| `pending`, worker `inbox` | `age_ms`, `held`, `overdue` | Seconds in the durable callback receipt are converted to milliseconds. Age survives process restart. `held` reflects the retained execution hold at this read. |
| `pending`, worker `allocation` | `age_ms`, `overdue` | Since allocation reservation, including time spent obtaining authority, resources and runtime. Five minutes is diagnostic delay, not proof of an orphan or time since last progress. |
| `allocation_effect` | `outcome="mint_unknown"` | Initial provision and renewal report an unresolved mint effect. Consult the original mint intent and evidence before any action. |
| `command`, traffic `channel` or `ordinary` | `outcome`, `age_ms`, `dispatch_age_ms`, `queue_overdue`, `dispatch_overdue` | Observed during the existing bounded command page scan. Queue age starts at durable admission. Dispatch age starts at the immutable fence. The latest retained observation takes precedence over the initial dispatch state. |
| `authority` | Fixed `reason` | Membership unavailable, denied, or missing current admin evidence; failed human refresh, expired human token/control grant, failed selected-credential verification, or expired effective credential lease. Denial may be intentional revocation. Verification failure does not prove token expiry. |
| `credential_expiry` | Signed `remaining_ms`, `expired`, `expiring` | Current Sandbox credential lifetime at an existing authorized renewal snapshot read. Expiring means at most five minutes remain. This is not an inventory of every unused credential. |
| `pending`, worker `child_cleanup` or `workspace_cleanup` | `age_ms`, `held`, `overdue` | Since explicit cleanup admission; held progress is visible even when the scheduling call returns `Ok`. A hold can be correct while a child is busy or authority/evidence is unavailable. |
| `retirement` | `outcome`, durable step `attempts`, `stalled` | `retention_held` is distinct from advanced work. Five unsuccessful attempts flag attention; this is a retry count, not a duration. Retained data remains protected. |
| `outbound` | Existing `outcome`, `reason`, `age_ms`, `elapsed_ms` | Original outbox age and attempt duration, including `unknown`, credential failure and provider rate limiting. Unknown remains unknown. |
| `scheduler` | Existing fixed worker, active/queued slot counts, discovering | Heartbeat every 30 seconds, including empty workers. Slot counts are process-local, not durable backlog totals. |
| `scheduler_failure`, `command_worker_failure` | Worker or fixed ledger `code` | Discovery/storage/worker failure remains visible when no durable row can be read and age samples are absent. |

Queue alerts trigger at 60 seconds of waiting; unresolved `DispatchFenced` or
`RecoveryRequired` alerts trigger at 30 seconds after the fence. `RuntimeLost`
remains a separate outcome. A turn waiting for human approval and an observed
active turn are not queue-overdue or ambiguous-dispatch alerts. Root retirement
alerts require five attempts and an outcome other than `advanced`.

These are repeated observations, bounded by the existing worker/page scheduling.
Their counts are **not counts of unique messages or failures**, and the observed
ages are not a full-collection maximum. Do not sum page samples into a backlog
gauge. Missing samples do not prove an empty queue: inspect worker heartbeats,
storage failures and existing deployment readiness. No new counter duplicates
the durable ledger.

## Query and policy use

Every checked-in policy scopes the existing `chrono-bot` namespace, `api`
container, and `cma-api-` pod prefix. A policy's `conditions[0].conditionMatchedLog.filter`
is directly usable in Logs Explorer. Example:

```text
resource.type="k8s_container"
resource.labels.namespace_name="chrono-bot"
resource.labels.container_name="api"
resource.labels.pod_name =~ "^cma-api-"
textPayload:"cma::bridge::operations"
textPayload:"operation=\"command\""
textPayload:"traffic=\"ordinary\""
textPayload:"queue_overdue=true"
```

This observes ordinary UI/API/Trigger delay during a Channel rollout. A long
ordinary turn can legitimately delay a queued message; correlate this symptom
with the rollout, current receipts, HTTP latency/status and storage failures
before attributing it to Channel traffic. Existing HTTP events include request
path, response status and latency; query them for investigation without turning
raw paths or request IDs into dimensions.

For a specific health line, retrieve the authorized public command/outbox or
cleanup receipt. A `command_id` leads to the existing retained envelope, response,
source and immutable audit correlation. For hashed work identity, hash the exact
authorized inbox/allocation/cleanup/Sandbox ID as UTF-8 SHA-256; the scheduler and
new observations use the same digest. This permits correlation without exporting
sender, chat/thread identity or secret material into health dimensions.

The eleven policies cover Channel/ordinary queue delay, dispatch ambiguity,
pending allocation/cleanup, unknown mint, membership failures, credential expiry
or failure, retirement stalls, unknown delivery, worker/storage failure, and
ingress availability. They are log-match alerts, not traffic-volume thresholds.
Each has a five-minute notification rate limit and 30-minute incident auto-close;
auto-close is a notification setting and **does not establish recovery**.

To prepare an operational rollout, select existing notification destinations and
review thresholds against the disposable canary's observed timings. Enabling or
installing policies is a separate operator action using existing authority; no
policy application is part of this source change. The policy catalog is a wrapper
with `schema_version` and `alert_policies`; each array entry is a policy object.
Keep optional log-derived metrics limited to the fixed dimensions above, and use
duration samples for distributions rather than per-ID time series.

## Verification and remaining acceptance

Focused tests cover dispatch/approval distinctions, durable clock age and skew,
and actual text emission with redacted work identity. The alert catalog is checked
against captured formatter fields; it does not claim Cloud Monitoring has accepted
or evaluated a policy in production.

Real disable/re-enable/rollback and ordinary-traffic impact drills, current sink
retention/collection coverage, operator notification routing and final source/image/
configuration pins remain delivery acceptance. A stopped scheduler cannot produce
new age observations: use its 30-second heartbeat and the existing platform's
workload/log-absence monitoring alongside these event alerts. There is no claim
that an uninstalled alert protects production or that a diagnostic stall proves
permission to remove storage or repeat a provider effect.
