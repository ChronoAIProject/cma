# Scoped runtime-loss reconciliation qualification — 2026-11-12

Part of #1350. The lead reassigned S2's scoped runtime-loss production wiring to
S4. S2 confirmed the exact ownership in the shared assignments note at 02:35 SGT.

## Source and ownership

Executable source: `9243f917137d66b9af8ecc94e85824cfad8aec89`.
Published source/documentation checkpoint:
`de645dcab68851727ca3a80f4e20abca3debace2` on
`tl/cma-log-segments/writer-publication`.
Develop `1d425cffc6e48bf33cc26635b6e1a58f92aec79f` is normally merged.
No mixed integration tip, force-push, PR or production activation.

The original `recover` caller selects `transaction::apply_scoped` for segmented
children. The format lookup is only a dispatch hint. All source validation,
Agent publications, legacy siblings and Sandbox/event changes remain in one
owned transaction. Legacy siblings receive no adoption authority from v2 peers.

`writer_commit::publish_captured` reuses the certified source and returns an exact
`Recorded` publication. Runtime-loss receipts retain each publication across
scope release. Unknown commit reconciliation compares the original complete
publication, its request/result identity, exact Agent and log batch, and original
Sandbox/events at the original coherent commit snapshot. Missing or foreign
receipts remain unresolved; they do not trigger replanning or acknowledge success.

The production capture helper and both scoped publication layers are now used.
`agent_capacity_recovery/source.rs` keeps its existing signature. S5 still owns
`agent_capacity_recovery::{admission,authority,records,status}`; specifically,
`records.rs::insert` is the production consumer of `TransactionScope::insert_one`.
There is no missing interface for the reassigned runtime-loss root.

## Narrow dependencies and changes

- `c57b68cabd8e4fe156441f74e47feba903b6c13d` isolates only S2 dispatch from
  `c13eff3b5` and accepted S5 lineage bounds/test from `a8b18866f`, on the common
  scoped-contract base `ecd84ecd3`. It was normally merged.
- `59aaca5f9` adds original publication receipts, root qualification tests and
  canonical Mongo selection. Eight existing reconciliation tests were moved
  without semantic changes into modules below 500 lines.
- `b4e905660` corrects the late-corruption assertion to inspect durable event
  absence. The public event reader correctly rejects that intentionally invalid
  stream; its rejection is not evidence of rollback failure.
- `02f634983` isolates the exact two-line S5 fixture correction from
  `5fa84974f8ed576f4c8993278301d71016c4c77e`: independent creation keys and Worktree
  identities let 513 synthetic children reach the cumulative bound. No bound or
  production behavior changes. It was normally merged.
- `9243f9171` merges develop. Four conflicts preserve both parent-readiness pacing
  and the independent transport/output/read-frame flags, with both sets of tests.
- `de645dcab` documents the already-existing parent-readiness pacing flag in the
  canonical retained-recovery reference. It remains default-off.

## Production-root scenarios

Seven new Mongo tests exercise actual gated legacy adoption and the runtime-loss
caller, rather than constructing v2 envelopes directly:

1. Mixed v1/v2 children publish the original recovery once; legacy siblings stay
   v1 and a repeated stale observation is superseded.
2. Unchanged terminal v2 children receive an exact transaction fence alongside a
   nonempty publication for a live child, with no extra terminal log/publication.
3. A parent CAS race aborts the staged child publication; the public root then
   refuses the stale observation with no durable child changes.
4. Corrupt v2 identity, a corrupt legacy sibling and late Sandbox event validation
   all refuse and leave the original head, projection, receipts and logs unchanged.
5. The actual root refuses 513 children before source capture/publication.
6. An original receipt rejects abort, foreign domain/log/request/publication and
   missing durable publication evidence. A reopened store proves the original
   commit; later deletion cannot invalidate the original coherent snapshot.
7. Injected committed and uncommitted unknown results, transient write conflict
   and permanent insert failure exercise original commit reconciliation, retry
   and abort. No uncommitted unknown result permits replanning or duplicate writes.

## Gate protocol

Rust 1.96.1; existing `.heavy-release.lock`, with blocking acquisition up to
3,600 seconds; `CARGO_BUILD_JOBS=2`; one `api/target`; no incremental/debug data.
The supervisor pauses the owned process group below 10% available RAM and resumes
at 12%. The disposable Mongo 8.0.13 replica set uses the canonical runner's pinned
image, failpoints, resource bounds and exact owned-container/volume cleanup. The
local runner changes only package/filter selection to avoid a workspace build.

Strict command:

```sh
cargo clippy --locked --offline --manifest-path api/Cargo.toml \
  -p cma --all-targets --all-features --no-deps -- -D warnings
```

Focused units select `store::agent_log_segments::`, the cumulative child-budget
unit, and `config::`. Mongo selects the complete reconciliation root, writer
persistence tests and `capacity_lineage_refuses_oversized_child_set_before_mutation`.
All Mongo runs are serial (`--test-threads 1`, `count:1/1`).

The first batch before the develop merge passed 41 units and 52/54 Mongo tests.
The two failures were fixture uniqueness and the intentionally invalid public
event reader assertion described above. Both corrections are included in the
final source. The first batch's uncertain-commit test passed in 85.334 seconds.
These historical results are not claimed as the final corrected-source gate.

## Remaining strict ownership

Strict on the final executable source exits 101 with 18 distinct dead-code
locations (production target 18; test target 17), zero type errors, zero unused
imports and no reassigned reconciliation warning. No lint suppression or dummy
consumer was added. The remaining locations are:

| S5-owned path under `store/` | Count | Names |
| --- | ---: | --- |
| `agent_capacity_recovery/admission.rs` | 3 | `AdmissionRequest`, `AdmittedRecovery`, `persist_admission` |
| `agent_capacity_recovery/authority.rs` | 5 | `Authority`, `context`, `capture`, `require`, `fence_parents` |
| `agent_capacity_recovery/records.rs` | 7 | `COLLECTION`, `Row`, `read`, `decode`, `collection`, `insert`, `invalid` |
| `agent_capacity_recovery/status.rs` | 2 | `RecoveryStatus`, `read` |
| `agent_log_segments/transaction_scope.rs` | 1 | `insert_one`, consumed by S5 `records::insert` |

This clears four of the prior 22 shared diagnostics. Whole-CMA strict green still
requires S5 composition and integration qualification. Ordinary writer/claim
compatibility and the other P1 activation findings are not closed by this one
runtime-loss root. Writer/recovery activation remains closed. Integration owns
the combined PR, review and merge relay; Project 9 stays In Progress.

## Final exact-source gate results (03:09 SGT)

| Gate | Result |
| --- | --- |
| CMA writer/capture/bounds and all configuration units | **114 passed, 0 failed, 2,464 skipped** |
| Real Mongo | **54 passed, 0 failed, 2,524 skipped** |
| Mongo breakdown | 19 reconciliation (including all 7 new scoped cases), 34 writer, 1 imported lineage-bound regression |
| Strict CMA all targets/features, warnings denied | **Exit 101**, 18 distinct S5 reachability diagnostics; zero type errors |
| Mongo runner harness | 31 passed, 0 failed |
| Canonical configuration documentation and parser | Passed; 4 parser tests passed |
| Rust formatting and whitespace | Passed |
| Source-file size | Passed; 6,124 files checked; every changed slice/conflict source file below 500 lines |
| Source history secret scan | 3,810 commits scanned, zero findings; final documentation history is scanned again before publication |

The exact-source Rust total is **168 selected tests passed**, with no failed
tests. Harness/parser checks are separate and are not counted as real Mongo.
The uncertain-result case passed in 96.448 seconds. The final batch's minimum
available RAM was 27%, with zero pauses. The first batch's minimum was 26%, also
with zero pauses. The existing release lock was retained throughout.

The final Mongo runner exited 0; its exact owned container and volume were
independently confirmed absent. Executable files under `api`, `.github` and
`infra` are identical to the source pin above; subsequent changes are documentation.

Raw gate-output SHA-256 values, recorded before disposable-log cleanup:

- `clippy.log`: `ab3f5e3232790f5a4eaad94cd85acb2eb38c9bb333f9a1b4bebc5091656be0b1`
- `units.log`: `2514df579c7c10a8983e733d709889521fa4a9b53bc53b9f2d21408d4357babc`
- `mongo.log`: `5cf0df6bf1c1293043927287d8a28563df868fb2a342c88478b58385444b2f5d`
- `pressure.txt`: `792c8b56e0a5297700382e22394e6b5e0cfecd0feac9e2d64f34040bc15f50b9`

## Cleanup

Removed **2.743 GB** of owned build outputs, downloaded nextest tools,
temporary runners, supervisors, logs and comment bodies. No build target remains
in this worktree. Requested dated notes and their GitHub mirrors are retained.
The worktree and published writer branch remain for integration-owner verification.
Available disk after cleanup: 47.50 GiB.

Shared lock inodes are retained: `.heavy-0.lock=160171589; .heavy-1.lock=160080032; .heavy-release.lock=160988921`.
The final owned Mongo container and volume were removed and independently checked
before target cleanup. No other worktree, container, volume or lock was removed.
