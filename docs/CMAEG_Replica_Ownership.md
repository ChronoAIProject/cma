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
Unknown or Partial delivery does not authorize advancement or blind resend.

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
extend that same live generation between completed stages; it cannot resurrect
an expired token. Each stage retains its own snapshot through its awaits. The
driver uses the handler's renewed token for final completion or release.

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
The manifest owner's staged contract is CLI, then sleep 30 seconds, with a
provisional 760-second termination grace period. Activation remains separate.

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
