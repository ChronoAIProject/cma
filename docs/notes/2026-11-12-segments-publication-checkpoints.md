# Ordinary publication checkpoints

Retained checkpoint evidence from the writer slice; final integration qualification is tracked in the assignment ledger.

## S4-B1 confirmed allocation: ordinary v2 publication

Lead answer #229 assigns S4 all remaining paths; no allocation remains pending.
Branch `tl/cma-log-segments/ordinary-v2-publication` starts at qualified `5875b8ddf`.
S4 owns `store/agent_conversation_store.rs`, its `publication.rs`, `checkpoint.rs`
and new scoped publication/checkpoint helper modules;
`store/agent_commands/reconcile.rs`; the necessary single registrations in
`store/agent_log_segments/mod.rs`; and regressions under
`store/sandbox_mongo_integration_tests/agent_segment_tests/`. Existing v1 roots
and all parent, ownership, command, budget and checkpoint fences remain enforced.

Requested narrow S1 dependency through the lead: read-only bounded
`agent_log_segments::command_evidence::read_transaction(store, agent, agent_row,
command, session) -> Result<Option<CommandEvidence>, TransactionLogError>`.
None means legacy only; Some must prove exact original dispatch/turn/delivery/
approval/terminal/deferred-failure facts through the current head, without writes
or transaction control. S1 supplies that module and any required core evidence
constructor; S4 wires the caller and its single module registration. No lifetime
scan fallback is acceptable for segmented command settlement.

Target pin 15:00. Required evidence is actual adopted Agent ordinary output and
command settlement, checkpoint/private assembly, refusal, abort and unknown-commit
reconciliation against original publication/domain receipts. Gates remain strict
CMA/affected-core lint, selected units and real Mongo, fmt, canonical size and
secret scan; shared slots use `lockf -k -n -t 3600`, two jobs, one target and RAM
pause below 10%. Activation stays closed until integrated review/qualification.

### S4-B1 source checkpoint while executable gates queue

Executable source `d925c3ead` is on `tl/cma-log-segments/ordinary-v2-publication`.
Develop `fa292b834` is normally merged. Conversation/checkpoint callers now use
scoped v2 publication and original immutable receipts; legacy behavior is retained.
The command transaction's existing evidence fold is extracted for the requested
bounded S1 reader. That interface pin is still pending; settlement is not closed.

Seven Mongo regressions cover adopted ordinary dispatch/output/settlement,
checkpoint restart and exact replay, abort/refusal, foreign/missing receipts,
and committed/uncommitted unknown outcomes without runtime-input replay.
No current-source executable pass is claimed: the fresh compile has waited on
shared slot 1 since 13:50. The queue uses `lockf -k -n -t 3600`, two jobs,
one owned target, and pause below 10% available RAM. Debug symbols and
incremental caching are disabled to reduce disk use; assertions remain enabled.

Interim light checks: fmt and whitespace pass; canonical size 6,407 files pass;
all 24 touched publication/settlement source files are under 500 lines. Preliminary
secret history: 4,184 commits, zero findings. The composition size regression from
the develop merge was resolved by moving unchanged lifecycle options to the
existing runtime module. Exact counts, execution logs and source inventory will
be recorded after the complete caller/dependency batch passes. Local evidence:
`notes/2026-11-12-ordinary-v2-publication-evidence/`. P1-1 stays open and activation
stays closed. No PR or develop merge is performed by this checkpoint.

### S4-B1 executed checkpoint before settlement dependency

Tested source `b5b102777f404eb2f1afe334e2351f4cacd34a7e` (API tree
`85e990ef3deb3fa01bce278396491e1f9b14f2f5`): 17/17 focused units;
strict CMA all-targets/all-features Clippy with `-D warnings` passes with zero
diagnostics (2m51s); fmt/whitespace and 6,407-file size pass; secret history
4,191 commits / zero findings. The first fresh compile found one omitted snapshot
mode at keyed replay; `fff30e110` fixes it, and the corrected compile passed.

Real Mongo: 52 executed, 49 passed, 3 failed. The 45 legacy/admission/gate parity
cases passed, as did four new scoped output/checkpoint abort/refusal and committed/
uncommitted unknown tests. One new end-to-end test incorrectly required no Agent
revision change on the first chunk: runtime correctly publishes ExecutionObserved.
`6c56b8471` fixes that assertion and verifies only that fact is public while the
message prefix remains private; this correction still needs its rerun. The two
settlement abort/unknown tests reach the outstanding legacy-only command reader
and refuse Corrupt. The narrow S1 `command_evidence::read_transaction` pin remains
required for production wiring and the final complete caller qualification.

The 14:25 slot relay authorized the third lock. Executed batches used
`lockf -k -n -t 3600` on `.heavy-release.lock`, two jobs and one target. Minimum
available RAM was 25% for corrected compile/units/strict and 29% for Mongo; no
pauses, inode unchanged. The owned Mongo container and volume passed cleanup
absence checks even though tests failed. Evidence includes `unit-results.json`,
`mongo-results.json`, gate logs and supervisor records in the local directory
above. P1-1 remains open, activation closed; these results are not a green slice.

### S4-B1 complete caller source, dependency integrated

Caller commit `c778b1d571ff19d5897e18804dfae5defd0b957a` wires the requested bounded S1 reader into the actual
command settlement transaction and adds its single module registration. Legacy
folding is selected only when that reader proves a legacy root. Unavailable
proof remains Unavailable; driver errors preserve transaction retry handling.
The original continuation-authority fence, command dispatch and lane write remain.

S1 published its pin `9a41f5cfb` on mixed integration ancestry. To honor the narrow
dependency rule, the exact three commits `3bf06a39d`, `9ae6c09b3`, `9a41f5cfb` were
carried to the owned `tl/cma-log-segments/command-evidence-dependency` branch as
`ea473ff7a`, `eef2864a2`, `a1209cc7e`, then merged normally into this branch. No
unrelated integration tip was imported and no published history was rewritten.
No interface remains missing. Complete-source CMA/core units, strict Clippy and
all 52 selected Mongo contracts are queued for qualification; prior partial
results above do not qualify this new source. The 15:00 checkpoint is source-ready,
with full qualification continuing and activation still closed.

### S4-B1 qualified handoff, 15:33

Executable pin `6b3c025dbc6af5abc22ea663d8dafc75c8cb9c0e`, API tree
`9dd6a4a1d97182d140dd05ac79ea64078bef1ec8`, is fully qualified. This final handoff
adds documentation only; develop `fa292b834` is included by normal merge.
No interface remains missing. Ordinary v2 conversation/checkpoint publication
uses exact original receipts; command settlement uses S1's bounded original
facts in the same transaction, retaining continuation authority and lane custody.

- Focused units: 50 selected / 50 passed (17 CMA, 33 core).
- Real Mongo: 52 selected / 52 passed, including all seven new adopted-Agent
  publication/checkpoint/settlement tests and all 45 parity/admission/gate cases.
  Actual adoption, discovery, worker claim, contextual/targeted/ordinary admission,
  output after recorder restart and command completion are covered. Settlement
  asserts zero lifetime scans. Abort, foreign/missing receipt, source refusal,
  committed/uncommitted uncertainty and restart without input replay all pass.
- Strict CMA/core Clippy: all targets/all features, `--no-deps -- -D warnings`,
  zero diagnostics. No allowances. Fmt/whitespace pass; canonical size 6,410 files
  passes; all 37 touched/imported source files are below 500 lines.
- Secret history at the executable handoff: 4,199 commits, zero findings.
- The complete corrected batch exited 0 under `lockf -k -n -t 3600` on the
  authorized third slot, two jobs, one target; minimum available RAM 22%, no pauses.

The restart fixture captures only the injected transaction session and models
server cleanup after process loss; it proves cleanup changes no durable records
before retry settles the original output. This follows existing Mongo restart
contracts and changes no production behavior. Historical failed runs remain in
the evidence directory; the final successful run supersedes them.

Evidence and exact selected test names: local
`notes/2026-11-12-ordinary-v2-publication-evidence/final-results.json`,
`units-restart.log`, `clippy-restart.log`, `mongo-restart.log` and
`restart-supervisor.json`. Cleanup removed 2.426 GB of owned build outputs and
temporary runners/drafts, plus the owned local dependency branch. The Mongo
container/volume were verified absent; all shared lock inodes and the worktree
are retained. No PR or develop merge was performed. The slice is ready for
combined integration/review; Project 9 stays In Progress and activation stays
closed until the combined release approval. Older incomplete segmented projection
shapes still require S1's rebuild before activation.
