# Agent-log activation packet for owner review

Status: **NOT APPROVED; all activation held**. Prepared for the lead to take to
the owner under the 2026-11-13 decision requiring approval for **each** switch
and data-changing operation. Implementation and merge do not authorize execution.
Deployment engineers execute approved changes through the reviewed release
controller. The implementation owner performs no production activation.

The four exact selector-stage patches live in `.github/agent-log-stages/`.
They are explicit operator inputs, not automatically applied deployment manifests.

## Release record required before approval

Bind this packet to the merged source, immutable API and runtime image digests,
compatible recovery artifacts, actual deployed configuration, schema status,
controller revision, and fresh authorized cohort observations. Record the owner's
approval separately for each row/stage below. The lead retains exact Agent IDs,
home identities, operation keys and approvals privately; public evidence contains
aggregate counts only. An unknown or missing value blocks the corresponding row.

The initial canary is the **owner's own test Agents**, selected explicitly after
current ownership and lifecycle checks. Do not select historical incident IDs or
other users' Agents by assumption. No synthetic input, restart, stop, retirement
cancellation, import release or index creation is authorized by this packet.
The release owner must request approval for any such canary action separately.

## Independent changes and canaries

| Approval unit | What changes | Initial canary and prerequisites |
| --- | --- | --- |
| #1648 explicit Agent cohort | `CHRONO_AGENT_LOG_ADMISSION_AGENTS` prepares the selected histories serially during store startup, then selects bounded admission/public windows. Fleet percentage stays `0`. | Owner-approved test Agent IDs, at most 16; qualify startup/readiness, retained history, exact command receipts, quarantine and read-only readers. Keep adoption/recovery false. |
| #1648 fleet 10%, then 50%, then 100% | Apply the corresponding JSON merge patch to ConfigMap `chrono-bot/cma-agent-manager`, then a reviewed rollout of the same API image. Each stage is a separate approval. Patches change only the two legacy selector keys. Initial stages require unchanged adoption/recovery switches to be false; the release controller must refuse the stage if that readback precondition fails. | Explicit owner canary passes first. Percentage buckets are stable but are **not owner-scoped**; even 10% can include other users. Owner approval must explicitly cover fleet exposure before switching from the explicit canary. |
| #1666 title reader | `CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED=true` enables the proven first-turn prefix on the shared selected cohort. | After #1648 prerequisite 100% plus one stable hour, approve an owner-test cohort check and the actual consumer exposure separately. |
| #1666 approval reader | `CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED=true` enables the proven pending-approval suffix on the shared selected cohort. | Approve independently from title reads; preserve the exact pending set, original locators and command authority. |
| #1350 segment adoption | `CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED=true` enables discovery; durable `agent_log_segments_v1` CAS admission separately authorizes v2 adoption/rollover. These are two approvals. | One eligible owner test Agent first; complete schema, compatible readers/writers/recovery image, old-writer exclusion and backup/restore evidence. Keep the durable policy explicit initially. |
| #1351 runtime proof | `CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED=true` enables retained native-state observation in a qualified runtime. | Owner test runtime with original home/conversation authority and supported proof. An already-running runtime cannot gain historical proof merely from an API switch; any retained runtime change needs its own approved continuity procedure. |
| #1351 capacity recovery | `CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED=true` permits new admission only when both durable writer and explicit `agent_log_capacity_recovery_v1` gates agree. Approve the process switch, gate revision and each recovery operation separately. | One currently eligible capacity-only failed owner test Agent; fresh dry-run, expired prior lease, exact original native/home/input evidence and completed writer canary. No percentage/owner-wide recovery policy. |
| #1668 ledger-failed cohort | Supported maintenance/recovery of an eligible ledger-failed Agent; this is distinct from #1351 log capacity recovery. | One eligible owner test Agent/home first, then separately approve expansion toward at least three recovered Agents. Current retirement or stop/delete intent blocks resurrection. Require the qualified executable path, full-home backup/restore, physical writer exclusion and dispositions. |
| #1461 held-import release | Authenticated durable release of one held imported home, allowing only its explicitly approved pending effects. | One owner test home, both platform-owner and affected-user consent, verified backup/restore, exact source/provider/home identity, exclusion of all old writers/descendants and accepted preserve/export/delete dispositions. A local file/Boolean/DB edit is never a release command. |
| New Mongo indexes / segment schema | Explicit additive migration installs its reviewed validators/indexes and records the exact migration checksum. | Approve the exact collection/index definitions, resource budget and migration unit separately. Database-wide indexes cannot be scoped to a test Agent; qualify on a disposable representative replica set before the approved production DDL. No new index is introduced by these stage patches or the commit-retry fix. |

The title and approval booleans share the admission selector. They do not have
independent percentage controls. Changing the shared percentage to stage a reader
also changes admission/public-window exposure. Do not claim a title-only 10%
stage while the shared selector remains 100%. The #1666 release owner must submit
the exact combined configuration and obtain approval for every changed switch;
an incompatible proposed stage remains held.

The repository's ledger release runbook currently identifies an unqualified
real-home maintenance/release boundary. #1668 and #1461 remain **HOLD** until their
implementation owners supply an executable, tested release path and exact image
evidence. This packet supplies no workaround and makes no recovery-success claim.

## Measurements and expansion criteria

The owner's production deadline is **14 November 2026, 14:00 SGT**. The hard
limits below measure from the initiating client send through completion of the
whole operation. Record the exact client-visible completion condition and its
durable evidence before each comparison; an early admission ACK, first byte or
server span cannot substitute for completion.

| Complete operation | Hard elapsed limit | Intended stage contribution |
| --- | --- | --- |
| New Agent plus Linux or desktop Sandbox ready | <=5 s, reported separately for each Sandbox type | Provisioning control for these stages; bounded logs alone do not establish this target. |
| Agent tab open, including the required usable view | <=1 s | Explicit/fleet bounded reads, title/approval consumers and segmented readers target this path. |
| Send operation complete | <=1 s | Bounded admission and segmented append/receipt paths target this operation. |
| Interrupt operation complete | <=1 s | Bounded validation and original-command settlement target this operation. |
| Admin Sandbox usage report complete | <=2 s | Separate reporting control; Agent usage reads do not prove this path. |

For every measured operation retain sample, success, error and cancellation
counts, elapsed distribution, maximum and **every limit exceedance**, including
first-use samples. Timed-out, failed, cancelled and incomplete operations remain
in the evidence; do not remove them from the denominator or report them as fast
successes. A favorable p95 cannot pass a hard limit. Every exceedance fails that
operation's acceptance and holds expansion; unmeasured paths remain **UNVERIFIED**.
Recovery/release/DDL stages additionally retain their own continuity and safety
criteria and cannot claim these ordinary-request limits from internal timings.

Collect a matched off baseline of at least 30 minutes, preserving image, replicas,
workers, Mongo pool and resource limits. Retain first failures and separate rollout
and first-use preparation cost from steady-state results. Bind every observation
to source/image, process boot/generation, effective switches, approved cohort,
start/end time and nonzero sample counts. Never compare cumulative counters across
process restarts without accounting for resets.

- Explicit cohort startup: measure rollout/process start through store composition
  and readiness separately. `prepare_bounded_admission` awaits each selected Agent
  serially, with a 10-second per-Agent timeout and 60-second overall bound. Missing,
  invalid, failed, timed-out or unconfirmed preparation fails startup. Record
  attempted/prepared counts, elapsed time, first failure, readiness failures and
  restarts for each process generation. Percentage zero does not enter fleet lazy
  preparation. A successful explicit canary does not qualify fleet first-use
  failure isolation.
- Fleet: preparation prepared/reused/failed/busy, fallback/quarantine mix,
  write/window scans avoided, full-scan rows/BSON bytes/duration, Mongo commands
  and checkout wait p50/p95, readiness/503s, route latency p50/p95/p99, memory and
  CPU. Require at least 100 successful natural ordinary-message samples on each
  side for an admission p95 claim; report history/open-Agent samples separately.
  Observe at least 60 steady minutes per approved stage before requesting expansion.
  Separately retain the first eligible write's preparation latency/failure and the
  complete client request that triggered it. This lazy fleet phase has its own
  timeout/cooldown/fallback isolation; startup success is not evidence of it.
- Title and approvals: nonzero representative successful bounded reads and
  fallbacks for each consumer; exact title input/pending-set parity, scanned
  rows/bytes, commands/checkouts and net latency. Fleet aggregates alone cannot
  establish per-consumer benefit. Preserve the 200-row/4-MiB proof limits.
- Segmentation: actual natural rollover, one successor per predecessor, contiguous
  global sequences, retained original event bytes/receipts, bounded scan work,
  projection/audit lag and unknown-commit backlog. At least 36 minutes of read-only
  observation; without an observed rollover that acceptance remains pending.
- Capacity recovery: current dry-run dispositions, admitted/attaching/completed
  work and exact receipts, original Agent/native/home identity, input dispatch
  counts, output/checkpoint continuity and no recurring capacity loop. At least
  36 minutes after the approved recovery. Missing external evidence is blocked,
  and a correctly preserved terminal resource is a disposition, not a restored Agent.
- Ledger recovery/release: full before/after receipt and effect conservation,
  source/archive accounting, writer exclusion, outstanding destructive work and
  held/recovered/unknown counts. #1668 requires at least three actually recovered
  eligible Agents and 30 stable minutes after approved expansion; refusals and
  preserved retirement do not count toward that criterion.
- Index/schema: exact plan/checksum, postcondition readback, migration lease and
  resumability, index build duration/resource use, replication lag, checkout wait
  and serving latency. A source declaration is not proof the index is installed.

## Abort criteria

Stop expansion immediately for any identity/receipt/output mismatch, duplicated
input or effect, history gap/hash error, read-side mutation, unexpected format
conversion, quarantine growth, lost writer exclusion, unauthorized lifecycle or
destructive effect, unsupported reader, or unexpected pending-approval change.
Keep the release controller's existing stricter health/performance gates.
Failure to meet any applicable complete-operation limit above holds acceptance
and expansion, even when the relative regression guard below remains green.

Proposed additional stage guardrails for owner approval: abort on a new
candidate-attributed readiness/503 failure, or two consecutive five-minute
windows with route p95 or checkout-wait p95 more than 10% above the matched
baseline. Stop if unknown-commit backlog grows for two such windows. Insufficient
traffic, missing attribution or incomplete observation is a hold on expansion,
never a pass. The owner may tighten these proposed thresholds before execution.

## Rollback and evidence retention

For fleet rollback, the release controller applies `.github/agent-log-stages/off.json` and rolls the same
qualified API image after checking Deployment overrides. It clears both legacy selection
modes and preserves every independent reader, adoption and recovery switch. Preserve quarantine and all trusted
metadata. The patch alone does not change running processes. The rollback patch may be used while independently approved writer/recovery
dispatch remains enabled; it cannot bypass durable-gate closure or accepted-work
drain. Optional reader switches are separate approvals and must preserve the required v2 reader floor.

For segmentation/recovery, close the relevant durable gate by its exact expected
revision first. Reconcile/drain already-admitted preparations with their original
identities, then turn off process adoption/recovery admission through the approved
rollout. Already committed work, cleanup, bounded fitting appends and exact receipt
settlement remain supported. Do not disable runtime observation needed by accepted
recovery, reset history, remove markers, lower sequences or select an incompatible
old image. Unknown commit is not known abort and does not permit input replay.

For held imports and ledger maintenance, keep or re-establish the supported durable
hold; preserve source/archive/backup evidence and original operation identity.
Do not assume a partially released operation is reversible. Use only a qualified
forward-recovery/held-restore procedure approved for that exact state. Never restore
a stale home over accepted effects, restart pending deletion without disposition,
or cancel retirement to meet a recovery count.

For additive schema/index changes, stop further migration units and retain exact
ledger/postcondition evidence. Do not drop required indexes, validators or stores
as an emergency rollback. A compatible artifact and reviewed forward repair are
required after the new format is admitted. Each repair or removal needs separate
owner approval.

Return a content-free receipt to the lead after each approved action: approval ID,
source/image/configuration, stage/cohort count, observed interval/sample counts,
first failures, result, rollback status and unresolved conditions. The lead obtains
the owner's next approval; one stage never authorizes the next.
