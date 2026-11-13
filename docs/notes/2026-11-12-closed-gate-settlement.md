# Restore default-off settlement transactions

Part of #1350. Candidate `affdcb04f72ff34bc37955762e4bdf5bc3fd09b9`
failed the real-Mongo 64 MiB terminal-settlement contract at
`capacity_timing_tests.rs:120`: three transactions rather than two.

The #1768 ordinary writer caller unconditionally invoked
`conversation::adoption::prepare`. Its transactional gate read and abort occurred
even with no gate record, before the existing capacity-refusal transaction and
capacity-settlement transaction. Bounded command reservation (#1591) is unchanged;
its eventual output/checkpoint publication reached this extra preflight.

## Exact baseline evidence

The same single-test replica-set harness, Rust 1.96.1 and target directory ran:

| Source | Result |
| --- | --- |
| `affdcb04f72ff34bc37955762e4bdf5bc3fd09b9` | Fails, 3 versus 2 transactions |
| `01a0f5d84c74cd3ce7ec98df120e196e994342f2` (#1768) | Fails, 3 versus 2 transactions |
| `3dd89725aff2eea9650a5d5fda9200e7b20f30fd` (#1766) | Passes both admission modes: 2 transactions; 41 full-fallback or 23 trusted-admission Mongo commands |

Local evidence is retained in
`notes/2026-11-12-closed-gate-regression/`. Each execution creates one owned,
loopback-only Mongo 8.0.13 replica set and removes its container and volume.
Heavy work holds `lockf -k` on the reserved release slot, uses two Cargo jobs,
and pauses below 10% available memory.

## Correction and activation contract

`CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED` is a strict, independent, default-off
process switch, composed into the production Mongo store. When false or absent,
adoption preparation returns before creating a session or sending any Mongo
command. It preserves the original legacy publication sequence. Existing v2
publication and receipt reconciliation remain reachable with this switch off.
The durable writer policy remains the authority when the switch is on.

This changes writer activation: the process switch must be enabled for the
controlled writer stage, in addition to the durable cohort gate. Rollback closes
the durable gate first, drains exact admitted work with dispatch still enabled,
then rolls out the process switch as false. Durable closure alone while dispatch
is enabled still performs discovery. The previous gate-only activation packet
must be updated and re-sealed before writer activation with this candidate.

The regression compares command order and collection targets for both ordinary
conversation and checkpoint roots against their original transactional helpers,
with absent, disabled and enabled durable gate records while dispatch is off.
Only the reference abort versus actual commit name is normalized. Existing
BSON-parity, natural-adoption, refusal, source-conflict, abort and uncertain-commit
tests explicitly opt into adoption. The original two-transaction assertion is
unchanged; the capacity contract also requires zero adoption gate queries.

## Final local gates

- Real Mongo: 14/14 pass, including both original capacity modes and the new
  command-sequence regression. Final settlement remains two transactions and
  41 full-fallback / 23 trusted-admission commands, with zero gate queries.
- Focused writer units: 16/16 pass; strict segmentation config units: 4/4 pass.
- Whole CMA strict Clippy: all targets/all features, `--no-deps -- -D warnings`,
  pass without allowances.
- Workspace fmt check passes. Repository file-size gate checks 6,451 files;
  all 12 changed files are below 500 lines (maximum 498).
- The committed-head history secret-scan result is recorded in PR validation
  and local evidence so that it includes the fix commit itself.

No production calls or develop merge were performed for this checkpoint.
PR Backend Mongo contracts and every Backend tests shard must finish green,
with no failed job and an explicit merge relay, before merge.
