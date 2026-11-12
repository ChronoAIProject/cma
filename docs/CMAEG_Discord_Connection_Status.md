# Native Discord connection observations

The existing owner/organization-authorized channel create, GET, update and list
responses include an additive `native_discord_activation.connection` object for
native Discord sources. Older servers can omit it. The activation `state` still
reports eligibility and scheduling: **`selected` does not mean connected**.
The status read uses the already-loaded channel definition and an in-memory
registry; it does not resolve additional credentials or query Discord or MongoDB.

## Scope and freshness

`scope` is `serving_instance`. Another replica may hold the durable per-bot lease.
A missing object, unobserved source, different source revision, process restart or
stale observation cannot prove a bot disconnected across the deployment. This
status never authorizes starting a relay, transferring a bot, clearing a lease or
re-identifying. The durable journal and its existing fences remain authoritative.

`connected`, `authenticated` and `ready` are nullable. All become true only after
validated READY or RESUMED is durably committed. A saved checkpoint, reserved or
sent Identify, or heartbeat ACK cannot establish that state. A known local failure
or stop sets them false. Absent evidence or expired freshness sets them null and
`outcome` to `unknown`, retaining historical timestamps and the failure code.

Freshness uses monotonic deadlines: the supervisor observation expires after 30 s;
positive connection health also requires the current lease and heartbeat deadlines.
Renewing the lease cannot refresh the heartbeat, and an ACK cannot renew the lease.
`observed_at` and `valid_until` are RFC3339 display timestamps, not lease authority.
During a long retry cooldown, the observation can become stale; `retry_at` remains
an estimate, not a promise that a socket will open then.

## Fixed vocabulary

| Field | Values / meaning |
| --- | --- |
| `stage` | `unobserved`, `resolve`, `discovery`, `lease`, `authorize`, `socket`, `hello`, `identify`, `resume`, `ready`, `renew`, `event_commit`, `stopped` |
| `outcome` | `unknown`, `pending`, `ok`, `retrying`, `blocked`, `stopped` |
| `lease` | `unknown`, `acquired`, `contended`, `lost`, `released` |
| `identify` | `not_attempted`, `reserved`, `sent`, `resuming`, `resume_sent` |
| `last_dispatch_outcome` | `unknown`, `admitted`, `pending`, `refused`, `expired`, `invalid`; admission is not reply delivery |

`last_ready_at` records a committed READY/RESUMED. `last_event_at` records an
authenticated MESSAGE_CREATE reception; `last_event_committed_at` records its
durable commit. Other Discord event types do not increment these message
observations. `last_dispatch_at` records the inbox dispatch observation separately.
An empty activity page alone cannot distinguish a disconnected socket, no traffic,
admission policy refusal or unavailable downstream provider.

`last_failure_stage`, `last_failure_code` and `last_failure_at` preserve the first
specific failure of the latest failed attempt. Successful reconnects of the same
source revision retain that history. A source revision change clears it. Cleanup
errors never replace the primary reason. `retry_at` is null for terminal failures;
terminal authentication/configuration failures retain the existing requirement
for a source revision change before reconnecting.

`session_limit_invalid` is a transient discovery refusal, not terminal source
configuration. The same revision retries after 5–7.5, 10–15, 20–30, then 40–60
seconds, with fresh jitter on each attempt. An upstream REST cooldown can extend
those delays and is never shortened. Shutdown interrupts the wait. Discovery
401/403 and a mismatched bot identity remain terminal for that revision; lease
contention is a separate retryable authority refusal.

Failure codes are bounded to:

- Authority/resolution: `unavailable`, `authority_refused`, `invalid_configuration`,
  `missing_message_content_intent`, `rate_limited`, `protocol`.
- Discovery: `discovery_unauthorized`, `bot_identity_mismatch`,
  `sharding_unsupported`, `session_limit_invalid`.
- Lease: `lease_contended`, `lease_owner_mismatch`, `lease_source_mismatch`,
  `lease_invalid`; renewal: `renew_failed`, `renew_timeout`.
- Identify: `identify_budget_exhausted`, `identify_bucket_wait`, `hello_timeout`,
  `hello_invalid`, `authentication_failed`, `disallowed_intents`, `invalid_intents`,
  `sharding_required`, `unsupported_version`.
- Connection/admission: `backpressure`, `commit_unavailable`, `heartbeat_lost`,
  `authority_revoked`, `invalid_session`, `reconnect`, `transport`.

## Bounds and privacy

There are at most 64 process-local source observations. Attempts are fenced by
revision and a monotonically increasing local generation. Removed selections free
their slot. Late updates cannot revive a removed/replaced observation. State is
constant-size per source and contains only fixed vocabulary, booleans and times.
No new identities, hashes, tokens, session IDs, message content, upstream error
text or endpoint URLs appear in the status or diagnostic log fields.

Fixed `native Discord connection state changed` messages carry only stage,
outcome and reason, detached from request spans. Per-source repeated transitions
are throttled for five seconds; changed failure codes and committed-ready
transitions can bypass that throttle. All status and cleanup/inbox-scan diagnostics
share a limit of 64 lines per minute per runtime registry. Logs may be suppressed
by that limit; the current status snapshot continues to update.

`native Discord session limit observed` uses the same 64-line budget and has no
parent span. It logs only `stage=discovery`, a fixed `reason`, boolean `refused`,
and unsigned 64-bit `shards`, `total`, `remaining`, `reset_after`,
`max_concurrency`. Each matching predicate gets its own event; `refused` applies
to that predicate. Several predicates can appear for one response:

| Reason | Refused | Meaning |
| --- | --- | --- |
| `total_zero` | true | No session starts allowed by the observation |
| `remaining_gt_total` | true | Inconsistent available budget |
| `max_concurrency_zero` | true | No Identify allowed in the five-second bucket |
| `reset_after_zero` | false | Zero-duration boundary; retain consumed local reservations |
| `reset_after_gt_7d` | false | Long duration; use saturating deadline arithmetic |

[Discord's session-start contract](https://docs.discord.com/developers/events/gateway#session-start-limit-object)
defines `reset_after` in milliseconds, without a positive minimum or seven-day
maximum. The previous reset checks were local restrictions, not documented
Discord bounds. Zero is accepted without assuming an unused or unlimited bucket:
repeated zero observations cannot replenish locally consumed Identify reservations.
A positive observation establishes a future reset without refunding reservations;
that boundary is persisted even while budget is exhausted. Long durations never
wrap into an expired deadline. The durable single-bot lease, remaining budget and
five-second shard-zero bucket still fence every Identify. No source hash is added.

Use these observations to distinguish upstream quota shape from a credential or
intents refusal. Do not infer a bot count from log rows or treat this recovery
change as production delivery acceptance; verify committed READY and natural
MESSAGE_CREATE/dispatch evidence after an authorized rollout.

## Verification and deployment

This change introduces no flags, migrations, persisted fields or new credential
access. Session-limit recovery changes discovery validation and retry; Identify,
lease ownership, admission and dispatch remain fenced by their existing authority.
Use a verified CI artifact in an owner-approved CMAEG-only rollout; do not build
production images locally. Deployment is a separate owner/lead decision.

Before rollout record the current image and migration-compatible rollback boundary.
After readiness, use an already authorized owner UI/API to read an existing native
channel. Check transitions through lease/Identify to committed READY, or a fixed
failure reason. Confirm no identities/secrets in diagnostic logs. Observe natural
traffic for received, committed and dispatch timestamps, and independently check
for duplicate dispatch and other-channel regressions. Do not generate customer
messages or enable relay fallback without owner authorization. A diagnostic-only
rollout does not itself resolve the underlying #1586 intake failure. Follow #1476
for controlled soak and recovery acceptance.
