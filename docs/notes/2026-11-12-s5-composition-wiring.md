# Accepted capacity recovery composition — 2026-11-12

Part of #1351. The lead's 03:25 allocation transfers accepted-work service and
worker composition to S4. S5 retains record decoding, restart discovery and native
boundary correctness. The exact paths were recorded before production changes in
`docs/notes/2026-11-11-segments-strict-lint-assignments.md`.

## Source composition

Code pin: `fbe13913292cbf8230ab403685cd5d76d43bed64`.
Branch: `tl/cma-log-segments/writer-publication`. S2 owns the combined PR and integration.

- Normally merged S5 checkpoint `69eda007a` at `1a6d8401d`.
- Reused S2's scheduler commits `0eba12933` and `541c3f7f2`, isolated as
  `82ecad179` and `6a0399f3a`, then normally merged at `559545ada`. The omitted
  integration-only migration test assertion is covered by the new real-Mongo
  presence test. No mixed integration tip was imported.
- Reused only S2's published `sandbox_unpublished_recovery.rs` caller delta from
  `c13eff3b5`, isolated as `c9733c7be`, normally merged at `6850446de`. The later
  retained-snapshot entry is preserved. S2 retains ownership of that root.
- Added service/worker composition Mongo tests in `b5bdfa9fd`; `202e1d715`
  corrects a test assertion to match the refusal variant because the successful
  status payload intentionally has no equality implementation.
- `720cb9262` documents the imported strict, independent, default-off
  `CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED` flag in the canonical operations docs.

- `f209cfcc9` makes atomic-state assertions from one snapshot completed with
  majority concern and checks the exact Unavailable result when changed ownership
  prevents current lineage capture. It changes test observations only.
- `e22dcd123` normally merges develop `8167d01d8`, preserving the new hierarchy
  batching prerequisite together with independent recovery/parent-pacing flags.
- `787eab95a` normally merges S5 `f07034864`, including its fixture conversion fix.
- `fbe139132` makes the service's per-Agent lookup facade available only to its
  existing Mongo contract tests. Production restart already consumes bounded
  `discovery::for_agent`; no reserved restart edit, extra runtime query or lint
  allowance is needed. There is no missing interface for this composition slice.


The authenticated router constructs the real RecoveryService. Accepted commands
and the bounded scheduler call the real AgentWorker and shared accepted worker.
Every mutation obtains fresh current authority, publishes through one owned
transaction and reconciles the original receipt on uncertainty. Admission reaches
`records::insert` and `TransactionScope::insert_one`; status reaches `status::read`.
No synthetic consumer or lint allowance was added.

The scheduler retains at most one 32-row page and runs at most two operations per
turn. Discovery grants no authority. Before the optional collection exists, it
performs only a presence read; it does not run startup DDL. Closing new admission
preserves accepted work, status, original-request lookup and exact settlement.


## Tests

Six new Mongo tests cover ten scenarios through the real service and worker:

1. Optional collection presence is read-only before schema installation and
   observes the later migration without creating a work record.
2. Real admission inserts one carrier; a reopened default-off service returns
   the same receipt and completes the accepted request exactly once. Status and
   lookup agree with the final publication.
3. Foreign original-request timestamps and changed current ownership refuse
   before native IO, leaving durable state unchanged.
4. A failed acknowledgement after durable completion is retried after reopening;
   the original claim is not repeated and Agent/carrier/publications do not change.
5. Permanent claim failure, uncommitted Unknown and committed Unknown never
   attach or acknowledge. Only a proven committed claim advances durable state.
6. Permanent completion failure, uncommitted Unknown and committed Unknown retain
   exact outcomes. Only the exact committed completion acknowledges; unresolved
   work stays at the last durable claim and does not publish a replacement.

Native IO is a counted protocol stub. These tests qualify orchestration and real
Mongo persistence, not production native proof. The production boundary remains
Unsupported and activation remains closed pending its separate qualification.

## Gate protocol and results

Rust 1.96.1; two Cargo jobs; one `api/target`; incremental/debug data disabled.
Initial steps used the existing release lock (inode 160988921). After the lead
reserved it for the train, the final batch used `.heavy-1.lock` (inode 160080032)
with blocking acquisition up to 3600 seconds. The train-slot test build was stopped
before tests ran; that interrupted stage is not counted as a gate pass.
The supervisor pauses its own process group below 10% available RAM and resumes at
12%. Mongo uses the canonical pinned 8.0.13 image, isolated replica set, failpoints,
resource bounds and exact owned container/volume cleanup. Its temporary runner
changes only package/filter selection and records the owned fixture name.

Strict command:

```sh
cargo +1.96.1 clippy --locked --offline --manifest-path api/Cargo.toml \
  -p cma --all-targets --all-features --no-deps -- -D warnings
```

Focused units select capacity recovery, the seven bounded scheduler tests and
configuration. Mongo selects all capacity-recovery, reconciliation and unpublished
recovery cases, serially with `--test-threads 1` and `count:1/1`.

Historical qualification is kept separate: `6850446de` passed 106 units and 54/56
Mongo tests; both new assertion failures were corrected in `f209cfcc9`.
`e22dcd123` then passed 107 units and all 56 Mongo tests. Its two strict diagnostics
are resolved by the S5 fixture correction and the service-local test facade above.
The final pin is qualified independently below; earlier runs are not substituted.

## Exact final results

All Rust gates below ran at `fbe13913292cbf8230ab403685cd5d76d43bed64`. Develop
`8167d01d896bd9d6f6ceba21a20a9e145ce059ac` is normally merged. The final handoff
commit adds evidence only; executable sources remain byte-for-byte identical.

| Gate | Actual result |
| --- | --- |
| Strict CMA Clippy, all targets/features, warnings denied | PASS, exit 0, zero diagnostics |
| Focused CMA units | 107 passed, 0 failed, 2535 skipped |
| Unit breakdown | 75 configuration, 7 scheduler, 7 accepted-worker, 18 capacity storage/domain |
| Real Mongo 8.0.13 | 56 passed, 0 failed, 2586 skipped; 354.755 seconds |
| Mongo breakdown | 25 capacity (including all 6 new composition tests), 19 reconciliation, 12 unpublished/retained recovery |
| Formatting and whitespace | PASS |
| Source-size check | PASS, 6186 files; all new/edited slice source files below 500 lines |
| Canonical configuration coverage | PASS |
| Mongo runner harness | 31 passed, 0 failed |
| Configuration-parser harness | 4 passed, 0 failed |
| Source history secret scan | 3875 commits, zero findings; staged/final publication history also scanned before push |

There are 163 passing Rust tests. Harness checks are reported separately and are
not counted as actual Mongo tests. Six new composition tests cover the ten named
success/refusal/crash/uncertainty scenarios; no simulated native transport is
represented as production native qualification.

Final resource supervision: existing shared slot 1, two jobs, minimum available
RAM 26%, 0 pauses, exit 0.
No signature or ownership question remains for the reassigned composition slice.
Project 9 stays In Progress; S2 owns the combined PR, review and explicit merge
relay. Writer/recovery activation remains closed pending the separate findings
and native qualification.

## Evidence and cleanup

Retained local evidence: `notes/2026-11-12-s5-composition-evidence/manifest.json`
and its final logs. The manifest records the API tree, log hashes, fixture cleanup
and all three lock inodes. The requested local note and this committed mirror are
identical. The assignments note contains the allocation and exact final source.

Removed 2.372 GB of owned target outputs, downloaded tools and superseded gate
files. Both local dependency branches created for this task are deleted after
normal merges. The worktree remains for integration-owner verification.
The final owned Mongo container `cma-mongo-ci-dcfcf7f3970b81420fccf8f1` and its data
volume are independently confirmed absent. Shared lock inodes remain 160171589,
160080032 and 160988921; no shared lock was deleted or recreated.

Final gate log SHA-256 values:

- `clippy.log`: `5909df11c5ec3e1f93232dc0173403efbc027e2ddf9501989dca912631196c33`
- `units.log`: `5dfef0eeb82ce88c9f41b52e7e0a3763337bfc9e933070f83e20ce28f25d1971`
- `mongo.log`: `2a054efbb52dc12ab878aef76ef823a803e4dd9fdbf194f100cfae50fb99bd4f`
- `supervisor.json`: `687e0330a24bbad601d3927fa998a3c3b50fbe127b0ddd4ca508758befcea74d`
