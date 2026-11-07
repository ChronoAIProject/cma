# API background work measurements — #1421

This is a partial reduction of repeated work, measured against develop
`a806d60a65d73cd11d45830daa9be05ea2ec475a`. It does not explain all production
CPU consumption or qualify multiple API replicas. #1421 remains open.

## Production attribution

The read-only observation on 2026-11-07, 18:50:24–19:50:24 SGT, measured
3.419 API CPU cores, 744,085 Mongo checkouts (206.7/s), and 0.221 application
responses/s. These are whole-process totals, not per-worker CPU samples.

There were 2,180 `RemoveWorktree` attempt logs across 44 Agents. Durable logs
in the same interval contain 2,160 failures and 11 completions. Nine attempt
records remain unjoined across publication/interval boundaries. Of the
failures, 2,041 have class `agent_cleanup_unavailable`, including 2,012 with
runtime code `mutation_authority_corrupt`; 119 have class
`agent_resource_quota`. A later metadata snapshot found 34 pending removals
with durable attempt counts between 13 and 4,736.

These are predominantly retries of failed removal, not successful recurring
cleanup. `AgentWorker::remove_worktree` routes provider failures through
`handle_failure`; the retry retains the removal step and operation evidence,
increments attempts, and makes the same work due again. Previously every
ordinary failure used the 30-second retry delay. `published=true` in the
worker attempt log includes persisted failures; it does not mean the
worktree was removed. This change does not repair corrupt runtime authority.

All 1,417 connection-reconciliation sweeps scheduled zero recoveries. Their
69,938 candidates included 24,097 detached candidates sent through the
recovery store path, or 17.006 per sweep and 6.694/s. Many of these reads can
only return ineligible/already-scheduled. Historical aggregate logs do not
establish that every one would pass the new discovery eligibility skip.

## Code paths and local attribution

`main.rs` starts the workers per API process. Agent, Sandbox, Workspace,
launch, Sandbox-control, bindings and Agent-access scheduling have one-second
fallback ticks; command scheduling polls every 250 ms. Those timers are
unchanged. Local process registries do not coordinate reads between replicas.

The local fixture separates discovery from actual cleanup transactions:

- Discovery: 13 owners, 339 Sandboxes, 1,402 Agents and 50 live candidates.
  The live mix is 33 attached, five paused, ten closing and two already
  awaiting connection recovery. Retained rows include 34 removal intents,
  two process-close intents, and failed/stopped/deleted records.
- Cleanup: actual claim/provider-failure/commit paths using real Mongo and
  deterministic in-memory provider errors, with both short and 10,000-entry
  histories. The long history is a worst case, not the observed median.
- Stream control: an idle stream snapshot with 5,000 retained entries.
  Open streams can poll without new HTTP responses. Low response counts
  therefore do not exclude this contributor.

A ten-second local symbol sample identifies
`commit_agent_work -> scan_complete_log_with_session -> scan_collection`
and BSON decoding/allocation in the expensive cleanup path. At history
capacity, failure settlement previously scanned the complete history just to
select its last entry, in addition to full-log validation in the commit.
Blocked-thread sample counts are not CPU percentages.

## Controlled comparison

Both versions use the same fixtures, serial test execution, macOS/ARM64,
the unoptimized Cargo test profile, and an owned Mongo 8.0.13 replica set.
CPU is the Rust process's user + system time from `ps`, with 10 ms
resolution; it excludes Mongo and Docker CPU. Checkouts are counted with
the driver's CMAP callback; no command-monitor callback is installed.
These component measurements are not a complete serving-process simulation
or a prediction of production core savings.

| Measurement | Before | After |
| --- | ---: | ---: |
| 50-candidate reconciliation CPU per sweep | 9.60 ms | 5.30 ms |
| 50-candidate reconciliation elapsed per sweep | 16.16 ms | 6.25 ms |
| Reconciliation Mongo checkouts per sweep | 18 | 1 |
| Reconciliation checkouts/s at one offered sweep/s | 18 | 1 |
| Failed cleanup CPU, 10,000 entries, per attempt | 604.0 ms | 411.3 ms |
| Failed cleanup elapsed, 10,000 entries, per attempt | 724.9 ms | 494.0 ms |
| Failed cleanup checkouts, 10,000 entries, per attempt | 29.03 | 27.03 |
| Failed cleanup attempts in one logical hour | 120 | 12 |
| Rust CPU consumed during that logical hour | 71.92 s | 5.25 s |
| CPU seconds / logical second for that workload | 0.01998 | 0.00146 |
| Mongo checkouts/s for that workload | 1.933 | 1.087 |

The logical-hour fixture offers due-discovery once per logical second for
3,600 seconds to one already repeatedly failing cleanup with 10,000 log
entries. It executes those calls serially without sleeping between logical
seconds. Persisted `ready_at` determines which removals run. Checkouts include
the same 3,600 discovery queries in both versions. The results are 6,960
versus 3,912 total checkouts. The per-attempt measurement includes one
post-attempt observation; the logical-hour workload does not.

Unchanged controls show why this is only a partial reduction:

| Control | Before CPU / checkouts | After CPU / checkouts |
| --- | ---: | ---: |
| Agent due discovery | 0.20 ms / 1 | 0.20 ms / 1 |
| Sandbox due discovery | 0.20 ms / 1 | 0.30 ms / 1 |
| Workspace due discovery | 0.10 ms / 1 | 0.15 ms / 1 |
| 339-Sandbox authority scan | 31.0 ms / 4 | 31.5 ms / 4 |
| One resource-authority read | 0.70 ms / 3 | 0.70 ms / 3 |
| Idle stream snapshot, 5,000 entries | 108.5 ms / 10 | 111.0 ms / 10 |

Small timing differences in unchanged controls are measurement variation.
No speedup is claimed for short-history cleanup: checkout counts remain 18
per attempt. The full-log stream path and other per-process polling remain
follow-up attribution targets. Do not extrapolate these numbers to all
206.7 production checkouts/s or all 3.419 cores.

## Changes and latency

1. Carry an eligibility observation from the already validated discovery
   record. Detached paused/stopping/already-scheduled candidates skip the
   redundant recovery read. Eligible recovery still rereads state and uses
   the existing conditional write. Nothing is cached across sweeps; a state
   change is visible on the next existing sweep, without a new delay.
2. Back off failures only at `RemoveWorktree`, using the existing durable
   attempt count: 30, 60, 120, 240, then 300 seconds with the default config.
   A configured base above five minutes is never shortened. Advancement
   resets attempts. Restart and another worker retain the schedule. Existing
   timestamp semantics remain: provider/database elapsed time can consume
   part of the retry interval.
3. Fetch the capacity-settlement planning tail through the existing
   `agent_log_entry` API. The following fenced commit still validates the
   complete log and exact tail; an earlier gap cannot authorize a write.

The trade-off is that final worktree deletion can wait up to five minutes
between repeated failures, after prior process/home cleanup stages. Provider
or storage failures can still prevent progress. Connection recovery,
process-stop pacing, receipt validation and #1429 terminal interpretation
are unaffected. There is no durable schema, admission or ownership change.

## Verification and reproduction

Run under the shared heavy-build lock with at most two Cargo jobs:

```sh
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
  RUST_MIN_STACK=16777216 \
  bash .github/scripts/mongo-contract-tests.sh --cpu-profile
```

The script ignores caller Mongo URIs, creates its own bounded disposable
replica set and removes only its owned container and volume. The two
`cpu_profile` modules contain synthetic data and print aggregate JSON.
The normal Mongo-contract selector also includes the new regression tests.

Local validation passed: formatting; affected-crate clippy with warnings
denied; 2,628 ordinary `cma`/`layer2-core` tests; 23 selected real-Mongo
tests/profiles; locked API build; core doctests; dependency audit; file-size
validation; and ten Mongo fixture-runner tests. All builds used the shared
lock and at most two Cargo jobs.

The new real-Mongo tests cover ineligible discovery and next-sweep recovery,
restart/takeover before the retry deadline, ordinary and full-history retry,
lost provider acknowledgement, eventual single completion, and an earlier
history gap despite a valid tail. With only the new tests on baseline,
both retry tests and the discovery-read test fail their assertions; the
existing full-history integrity guard continues to pass. Existing checkpoint
receipt tests run in the same Mongo validation set.

## Rollout and remaining work

Only the API needs a new image. The lead decides deployment after review.
Compare like-for-like windows using removal attempts and durable outcomes,
`recovery_deferred`, sweep elapsed time, Mongo checkout rate, API CPU,
request latency, and connection-recovery delay. A falling attempt count
alone is not proof of successful deletion; inspect the retained failures.

Rollback can use the previous API image because no durable format changes.
Already persisted retry times remain due at their recorded times; rollback
does not erase their possible five-minute wait. Once retried, the previous
image returns to its prior pacing. Mixed API versions can still write their
own retry delays under existing claims, so the full reduction requires the
old image to exit. This does not certify general multi-replica behavior.

Further changes to history reads, notification-driven polling or shared
scheduler discovery need separate review and realistic serving-process
measurements. #1421 remains open, related to #1356 and #1370.
