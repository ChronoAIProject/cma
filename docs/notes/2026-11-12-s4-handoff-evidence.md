# S4 source-specific evidence and accepted handoff

## S4 response facts dependency

Narrow implementation commit `99f9826f4b98ff3784c03492ce877f883700526b` on `tl/cma-log-segments/response-facts`;
normal develop-merged branch pin `91b487f575dfdf60758bb4d211324a7036f4f5c5`.
Shared base `11f4fcc69`; no mixed integration tip imported. Adds required
settings_changed/legacy_model_known, response_usage/response_usage_sequence
through the next TurnStarted, and exact adjacent prewrite_failure_sequence
paired with original end_sequence. The existing publication and bootstrap fold
both populate them. S1 consumes the same facts for indexed response parity.
Rust1.96.1 Layer2 projection tests15passed,zero failures,1419filtered; strict
Layer2 all-targets/all-features Clippy passed with warnings denied. Fmt,whitespace,
5998-file size passed. Every changed file<500lines. Release-only lock,2jobs,
minimum memory20%,zero pauses. Durable missing fields fail closed; activation
remains closed. Contract:docs/notes/2026-11-12-s4-response-facts.md.

## Final S4 source and test checkpoint

Source `bc075a36aa45ac8fc826d4999748a9bcc8071c6d` is pushed on
`tl/cma-log-segments/writer-publication`. It includes the authorized shared
optional-generation correction, develop through42fbf75de, the narrow response
facts dependency99f9826f4, and a durable fact commit/abort Mongo regression.
47unit/configuration,37writer+lease Mongo,23legacy caller Mongo,9retained
Mongo and8original unpublished Mongo tests passed:124passes,zero failures.
Final strict diagnostics and cleanup are appended in the post-review evidence.
P2-5 has passing corrected Mongo evidence; P1-1 remains open at discovery,
claims, command reservation and targeted approval. Activation stays closed.
No missing interface blocks the completed original receipt callers. Shared
strict composition still requires sandbox_reconciliation::transaction::apply_scoped
and agent_capacity_recovery::{admission,authority,records,status} through
TransactionScope::insert_one. No slice PR; integration owns the combined PR.

Final strict CMA at `bc075a36a` exited 101: 22 shared reachability diagnostics, zero type errors, no S4 receipt diagnostic. The 124-test batch completed at 02:21 SGT. Minimum available RAM 25%, no pause. Final source-bound gate evidence is in `2026-11-12-s4-postreview-gates.md`; the 01:30 qualification deadline was missed and reported while the build continued.

## Lead reassignment: scoped runtime reconciliation (Thu 11-12)

The lead reassigns the S2 scoped reconciliation production wiring to S4. Exact
path boundary recorded before implementation; S2 has been informed by the lead.

- S4 takes `api/crates/cma/src/store/sandbox_reconciliation.rs` production
  `recover`/`apply_attempt` dispatch, retaining S2's existing scoped dispatch.
- S4 takes `sandbox_reconciliation/transaction.rs` receipt composition,
  `transaction/scoped.rs`, `agents/scoped.rs`, and `reconciliation.rs` exact
  original publication/domain reconciliation.
- S4 adds focused tests beneath `sandbox_reconciliation/` and their local module
  registration. Canonical Mongo selector registration is limited to this root.
- S4 consumes `agent_capacity_recovery::source::capture_segmented_scoped` through
  the actual root; its existing signature and scoped proof checks are preserved.
- `TransactionScope::insert_one` has a real consumer in
  `agent_capacity_recovery/records.rs::insert`. That consumer and
  `agent_capacity_recovery::{admission,authority,records,status}` stay S5-owned.
  S4 will neither remove that API nor create a fake consumer to silence lint.
- S5 owns bounded `agent_capacity_recovery/lineage.rs`; its existing accepted
  count/byte-bound correction is a dependency for activating this root. S4 will
  reuse the published correction without changing its bounds or semantics.

S2's existing integration dispatch is reused; no mixed integration tip is merged.
Publication receipts must survive scope release and join the root's original
commit/abort/Unknown handling. Unknown or absent/foreign receipt never permits a
new plan or success. Gates use Rust1.96.1, two jobs, one target and the existing
release-only lock with memory pause below10%. No PR or activation is authorized.
