# S4 frozen publication signature handoff — 2026-11-11

Part of #1350 and transactional #1349. This implements the exact compile
contract in integration-owner amendment `7860048be`, after normally merging S1
signature commit `5ed7eaae1` into `tl/cma-log-segments/writer-publication`.

`agent_log_segments/writer_commit.rs` supplies `PublicationInput`,
`AggregateUpdate`, opaque `PreparedPublication`, `prepare`, and `persist` with
the frozen types and lifetimes. `writer_bootstrap.rs` supplies opaque
`BootstrapPlan`. There is no public/default/deserialization constructor for
proofs. Pending operations return typed `TransactionLogError::Domain` with
`SandboxStoreError::Unavailable` before any write. No fabricated receipt,
fallback success, transaction start/commit, panic or gate activation is present.

S4's own focused test-child registrations are already in its owned parent
modules. The remaining shared declarations belong to S1/S2 under the binding
manifest and have been relayed to the lead through the user:

- S1 `agent_log_segments/mod.rs`: `pub(in crate::store) mod` declarations for
  `publication`, `projection`, `writer_admission`, `writer_bootstrap`,
  `writer_commit`, `writer_control`, `writer_gate`, `writer_plan`,
  `writer_projection`, `writer_reconcile`, and `command`.
- S2 core sandbox root: register `agent_log_projection` and re-export
  `AgentLogAnswerEvidence`, `AgentLogAnswerLookup`, `AgentLogEvidenceChange`,
  `AgentLogProjectionState`, `AgentLogQueuedReference`, `AgentLogTurnFacts`,
  `AgentLogTurnLookup`, and `BoundedAgentLogFold`.
- S2 store/test roots: consume the S1 segment store/schema and S4 focused Mongo
  test roots; shared authority/migration inventory includes gates/admissions.
  S4's exact heavy-build lock assignment is still requested.

S5 can write its storage-only recovery caller against these exact signatures.
S4 continues replacing the refusal bodies against S1's accessors, adding the
certified opened/retired-generation maxima and source/session binding, then
same-transaction persistence. No signature checkpoint is a review-ready PR or
an executable atomicity claim. Develop merge freeze remains active.
