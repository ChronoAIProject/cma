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
