# Agent worker failure pacing

Part of #1421; preserves the cleanup rules in #1443 and the independent recovery
scheduler in #1448.

## Eligible failures

Only `PublishConnection` failures classified as `AdapterError::Unavailable` by
read-only runtime inspection carry a retry observation. This includes the
existing refusal when the bound runtime process was replaced. Classification
uses the typed error, never diagnostic text. The observation records the exact
Agent, Sandbox, Workspace and Worktree revisions read before inspection.

Unavailable results from effectful operations, unknown publication outcomes,
other error classes, healthy connections, stop work and cleanup retain their
existing behavior. Connection publication conflicts still get their existing
short retry; an unknown commit still cannot release its lease. Ledger holds and
final filesystem cleanup keep their existing pacing.

## Durable scheduling hint

After a validated failure publication commits, the Mongo store may CAS a
`worker_failure_pacing_v1` envelope beside the Agent record. The hint contains
only scheduling data, the failure class and revisions; no diagnostic detail,
credential, identity, payload or provider response is copied into it.

The first attempt is prompt and the first retry keeps the configured delay.
With the production 30-second base, consecutive matching observations wait
30, 60, 120, 240, then at most 300 seconds. Consecutive means the exact prior
publication followed only by the next claim and failure publication, with the
same typed class and all three parent revisions. Any intervening Agent change
or different observation starts a fresh streak.

Pending work and its ordinary `ready_at` remain intact. Discovery filters
unchanged held work before applying the page limit. A separate indexed query
checks only future pacing hints against current parent revisions; ordinary work
has no parent joins. Each query arm returns at most the requested page size,
then their union is deduplicated, sorted and bounded. Changed Agent or parent
revisions wake connection work on the next existing sweep, including before the
ordinary retry time. Direct claims recheck the hint and always retain the
existing lease, revision, parent ownership and checkpoint fences.

The five-minute fallback does not depend on notifications or local memory.
Provider recovery without a durable revision change becomes eligible for observation at that bound.
The serialized reconciliation scheduler, cursor fairness and reserved recovery
opportunity are unchanged. The hint never grants runtime or deletion authority.

## Failure and rollback behavior

The hint is optional. A crash between publication and the hint CAS, an old
writer, or a missing hint causes an ordinary retry. A late hint cannot overwrite
newer Agent work. Metadata or queued input adopted while publishing a failure
also invalidates the inspected revision and wakes work. Publication failure never installs a hint, and pacing does
not reinterpret an uncertain mutation as successfully aborted.

The Agent record and work schemas are unchanged. Old readers ignore the
additional envelope and continue normal retries, so this change requires no
expand/contract activation and no new environment flag. A rolling overlap with
old workers may reduce the optimization, but retains pending work and fencing.

## Verification

Real Mongo contracts use a deterministic clock to cover restart persistence,
first work, changes to every parent, changed Agent work, owner stop, stale
observations, lease takeover, a lost hint and the bounded fallback. A scan-count
contract compares one hour of identical repeating failures against the ordinary
path using the existing `WorkerPublication(PublishConnection)` counters: 121
complete scans with ordinary retries versus 15 with pacing (87.6% fewer).
This is a deterministic local fixture, not a production CPU or latency result. A
worker integration contract proves that read-only inspection failures actually
supply the observation. Existing worker, unknown-outcome and recovery contracts
remain regression gates.

Production verification belongs to the release owner. Compare aggregate
complete-scan counts for the same caller/step before and after the train, along
with queue progress and request latency. Local scan reduction is not evidence
of a production CPU or latency improvement. No production profiling, restart,
provider mutation or environment change is performed by this implementation.
