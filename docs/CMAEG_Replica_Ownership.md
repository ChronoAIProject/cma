# Event Gateway replica ownership dependency

This is the ownership API dependency for #1530 and #1541, integrated through
#1370. It is not an activation qualification. The delivery, native socket,
console, process-loss and performance gates below remain required.

## Configuration and migration

`CMAEG_REPLICAS_ENABLED` accepts only `true` or `false` and defaults to `false`.
Enabled startup requires direct Mongo access, durable routed admission and
recovery, and the additive `cmaeg --migrate-replicas` migration. The migration
adds an optional discovery projection and indexes to existing connection and
interaction collections; it does not enable ownership writers.

`CMAEG_REPLICA_ADDRESS` is a direct HTTP origin with a literal pod IP and the
gateway's explicit listening port. Membership authenticates that address inside
the existing encrypted records. Caller-supplied URLs never choose a peer.

The first enabled startup persists a protocol floor. Subsequent mode-off starts
and a different deployment identity are refused. Old readers that reject the
optional projection are below the compatibility floor. Rollback must use a
compatible fenced binary and retain replica mode. The deployment identity is
the SHA-256 of the configured management public origin.

The first activation repairs legacy admission roots before serving. This is a
bounded page scan, not yet a bounded total-time migration. It must be measured
against retained production volume and the approved no-overlap cutover budget.
The normal repair loop queries only indexed active admission roots. Root bytes
and version 1 remain immutable when discovery metadata is cleared.

## Authority and order

Membership and partition leases target 15 seconds, renewal every 3 seconds,
and individual store calls bounded at 2 seconds. These constants alone are not
a sudden-loss recovery result. Mongo server time and a conservative monotonic
deadline govern expiry; transactional mutations check server expiry again.

The canonical partition identity includes the versioned domain, deployment,
authenticated plan owner, channel, source record, target consumer, provider slug,
canonical conversation and partition key. It excludes event, target, body and
pod identities. Deterministic placement assigns exact quotas with maximum skew
one; placement alone never grants write authority.

Each permanent partition head retains an epoch watermark. FIFO insertion and
its retained item marker commit atomically before ingress can return success.
Completion and head advancement commit atomically. Exact admission replay
retains its original position; a completed item cannot be appended again.
Unknown or Partial delivery does not authorize automatic advancement or blind resend.
The explicit, audited operator abandonment below is the only additional release authority.

Mongo transactions retry only an explicit `TransientTransactionError`, with
at most five attempts inside one shared two-second deadline, including backoff
and Mongo I/O. This bound also applies to direct indexed admission-root writes;
the partition store retains its separate outer two-second timeout. A deadline
returns unavailable without retry and does not establish whether the transaction
committed: callers must reconcile the same retained admission. An
`UnknownTransactionCommitResult` takes precedence over that label and remains
unavailable for reconciliation. The retries repeat only the same record writes
and expected versions; they do not repeat an external provider operation.
Conflicting versions return to the queue's bounded reread loop. Separate item
and head reads may straddle another admission's commit; finding the work in the
head requires rereading and validating its exact retained item before success.

`cmaeg_replica_transaction_retries_total` counts known-aborted retries.
`cmaeg_replica_admission_rereads_total` distinguishes verified concurrent
insertions from missing or mismatched retained items. Neither counter proves
which stage committed for an individual rejected HTTP request.

`PartitionLease` is opaque and cloneable. A clone retains the captured instance,
claim, epoch, work identity and deadline. A journal mutation validates that
generation and touches the partition record in the same Mongo transaction as
the target mutation. A later renewal cannot extend an already captured clone.

`Journal::capture_partition`, `guard_partition`, and `write_fenced` expose the
boundary to delivery implementations. `partition_owner` returns only the
authenticated member address and expected fence. A nonowner cannot construct
or adopt that member's local lease.

Delayed work uses `Journal::enqueue_effect` with a separate stable work ID in
the original partition. `EffectExecutor` supplies execution and bounded recovery
callbacks; only `Completed` advances the queue. Tool and media implementations
must retain their own encrypted payload and outcome custody and compose behind
this interface. Initial effect-root admission is indexed and create-only;
subsequent effect transitions use `write_effect_fenced`.

The handler receives a mutable captured token. `Journal::renew_partition` may
extend that same live generation; it cannot resurrect an expired token. Ordinary
clones retain their captured deadlines. A scoped owner explicitly publishes fresh
snapshots of its same claim/epoch during provider acceptance and automatic, tool,
delayed, webhook and media-delivery waits. Renewal runs in the awaited future,
never a detached task, and is serialized with gate checks. It updates execution
or media ownership and the exact reply-guard attempt where applicable. A failed
renewal cancels further attempts and prevents receipt commit. The driver uses
only the scope's latest successful snapshot for bounded receipt publication and
release. Co-holders retry bounded partition-version conflicts only after proving
the same live work/claim/epoch; a concurrent renewal is not ownership loss. A
parent waits in place for a Pending tool's receipt while renewing, then continues
its retained frame without releasing or re-claiming. Waiting observes the attempt
deadline and has the invocation budget as a hard bound. Error cleanup never
releases while a tool is Pending or its state cannot be read reliably.
Preparation stages keep their existing retained checkpoints and retry behavior.

## Unresolved-head operator contract

`--replica-list-unresolved`, `--replica-abandon-unknown --dry-run|--apply`, and
`--replica-abandonment-audit` use authenticated loopback requests under the existing
master-key authority. The ticket binds deployment, port, method/path, deadline and
body digest. Inspection is read-only. Apply binds exact partition ID, front work
ID and physical head version, request UUID, reason and dry-run digest. It refuses
live leases, live receiptless guard attempts, unsupported/missing member capability,
changed evidence and ineligible states. Eligible states include dispatch Unknown,
effect Unknown or abandoned Sending, and Accepted with retained delivery/guard
Unknown or Partial. Starting alone is not eligibility.

One Mongo transaction records `unknown_abandoned`, closes the reply guard,
quarantines the target, retires the exact FIFO item and advances its head. Original
TargetState phase, response ID, cursor, frozen input and delivery evidence are not
rewritten. The immutable audit binds prior evidence, reason, timestamp and the
master-key ID used to authenticate the operation. Evidence chunks staged before
this transaction carry no execution authority. An uncertain commit is reconciled
by the same request UUID; it never authorizes releasing a different head.

New kinds use the existing encrypted connection/interaction stores without new
roles, migration, discovery kinds or writer flag. An immutable capability companion
is written once per unique process instance at join, retained thirty days in the
existing expiring interaction store; its Member row alone grants liveness.
Missing/expired companions refuse operator apply. Member's strict serialized shape
is unchanged; the companion reports capability and authenticated origin, not the
never-bumped package version as a build identifier. Apply checks fresh live
members, including draining members whose leases remain live, and refuses a
missing companion. Scoped renewal is compatible with overlapping older replicas.
Operator abandonment requires all old writers terminated and rollout/scale/rollback
frozen: capability inspection cannot atomically prevent a future old process join.

The audit index requires an exact rollback deadline for every entry. A missing
deadline is corrupt evidence; a missing audit receipt for an unexpired entry
fails closed. There is no earlier index format to support. Lists remain read-only, while
apply atomically prunes expired constraints and recomputes the maximum before
checking the 512-unexpired-entry bound. Expiring capability companions identify
the compatible operator cohort; older permanent companions do not authorize the
new writer. Pre-quarantine readers still never select these non-discoverable kinds.

Every reply path, including capability minting and recovery, checks quarantine.
Older binaries do not. Rollback to a pre-quarantine binary requires BOTH:

- For every abandoned target, the stored event's `expires_at + REPLY_TARGET_TTL`
  has passed; the TTL is 24 hours. Use the exact printed UTC `rollback_safe_after`,
  not an estimate or the maximum expiry of currently queued effects.
- Every quarantined effect has a saved refusal and retired FIFO entry, or its
  capability has expired. Inspection reports the outstanding count and the
  deployment-wide maximum rollback timestamp.

The first condition covers fresh keys and capabilities minted while the original
reference was still valid. Zero outstanding queued effects alone is insufficient.
See [operations](CMAEG_Operations.md#replica-unresolved-head-recovery) for execution,
refusals, audit recovery and the required deployment freeze.

## Native source follow-up

Replica-mode Discord uses the existing bot-keyed source lease, with a 15-second
TTL and 3-second renewal. Selection is balanced across live members; source
authority and lease contention are checked every second. Lease writes verify
Mongo server expiry and the current channel version transactionally. Inbox roots
and ACK claims touch the source lease and channel in the same transaction.
Staged body chunks alone never acknowledge an event.

The socket watches a monotonic lease deadline even while admission is blocked.
After it stops processing, a bounded two-second close handshake must observe
peer closure before an early lease release. Unconfirmed closure retains the
lease until expiry. These changes preserve the existing bot lease identity and
source revision rather than introducing a competing connection owner.

Local follow-up gates passed strict Clippy, 200 Discord tests, ten ownership
unit tests and eight execution-lease regressions. The authenticated Mongo suite
passed four existing ownership cases; its new native case initially failed on
an incorrect fixture resume URL, then passed after including the protocol's
required query parameters. Failed evidence is retained. The native case verifies
source takeover, stale checkpoint/renew/release refusal, and credential-version
rejection. The socket tests verify confirmed/unconfirmed close and cancellation
of blocked admission at expiry.

Observed single-trial ownership times were 15,080 ms for partition takeover
after heartbeat stop and 15,132 ms for native source acquisition after the last
confirmed renewal. Poll cadences were 100 ms and 250 ms respectively. These are
independent-client boundary observations, without an independently observed
expiry instant, socket reconnect, first provider effect or client recovery.
They are not percentiles or end-to-end RTO results. Twenty partition claims
were distributed 10/10, 5/5/5/5, then 10/10 through two/four/two members.

## Drain contract

`cmaeg --replica-drain` submits an authenticated loopback POST to
`/internal/replicas/drain`, using existing master-key authority. The ticket binds
deployment, port, method, path and a bounded deadline. Repeated valid calls are
idempotent. The CLI is a no-op with replica mode disabled.

HTTP 204 means readiness was withdrawn and drain requested. It does not prove
socket closure, lease release or completed handoff. Native socket closure and
the full shared drain must finish before manifest grace values are qualified.

The #1994 design ruling requires the Kubernetes native
`preStop: { sleep: { seconds: 30 } }` action and retains the 760-second
termination grace period. The lifecycle configuration must allow at least
30 seconds of propagation, the parsed `CMAEG_DRAIN_SECONDS` value, and
45 seconds of teardown margin: 735 seconds with the configured 660-second
application drain. This arithmetic is a configuration contract, not a measured
hard lifetime. The manifest and Rust/Ruby contract correction is a separate
#1994 integration dependency.

An exec hook is prohibited while the in-container media transformer is enabled.
Making the drain CLI non-dumpable does not protect its independently executed
parent shell or sleep helper from same-UID access to inherited Secret variables.
The earlier staged CLI-then-sleep shell hook is therefore not an accepted
deployment mechanism; historical preparation and release receipts do not qualify
it for reuse. Native sleep provides propagation delay without creating that
helper, but does not withdraw readiness or request ownership drain before the
delay. Secure early owned drain remains pending #1370/S2 integration, including
authentication, bounded completion/failure semantics and Secret isolation for
every participating process. Activation and rollback acceptance remain separate.

## Remaining acceptance gates

- Full native Discord supervisor placement and socket handoff against independent
  processes, including first served effects after abrupt loss and graceful drain.
  Telegram native ingress is webhook based; NyxID owns its upstream platform
  connections. No Telegram long-poll owner is implemented in this gateway.
- Tool relay, delayed reply capability and media FIFO composition, including
  ambiguous provider outcomes and authoritative reconciliation where supported.
- Public retained console sessions, CSRF, refresh serialization and logout across
  replicas. A zero session gauge does not remove this production requirement.
- Every enabled ingress/delivery path's dedupe, rate-limit and cache contract.
  The explicit routed-channel selector and legacy launcher fallback are still a
  concrete whole-service activation blocker. Passing selected-channel tests does
  not qualify unselected channels or connection-completion paths. No additional
  agent-published producer has yet been identified in this topic's source.
- Custom Sync streaming uses process-local waiters. Durable admission does not
  route output back from a different partition owner. Cross-replica streaming
  and recovery need their own implementation and process tests, including media
  continuation output; successful selected webhook delivery does not qualify it.
- Independent real processes against one Mongo: duplicate ingress, abrupt loss,
  zombie mutation, node-loss equivalent, and 2 to 4 to 2 scaling. Independent
  client boundary tests do not substitute for process/provider proof.
- Same-candidate N=1, N=2 and scaling latency scorecard, measured baseline noise,
  hard process-lifetime capacity and exact-SHA architect review.

Local gate receipts are handed to the integrator with each dependency SHA.
No receipt may label skipped database tests or unresolved accepted effects PASS.
