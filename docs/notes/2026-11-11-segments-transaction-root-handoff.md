# Scoped transaction root handoff

The raw-session compatibility gap identified by S5 is real. A segmented caller must start and pass the exclusive scope at the transaction root; a raw already-running transaction must continue to refuse segmented proof/publication. Do not add adopt/attach or a public raw-session accessor.

Ownership, effective after normally merging the pinned scope dependency:

| Root / boundary | Owner | Required handoff |
|---|---|---|
| store/lifecycle_transaction.rs (start at89, apply at158) | S4, existing allocation | Start TransactionScope and pass it to S5 checkpoint_capture scoped validator and S4 publication; caller consumes release before commit/abort. |
| store/checkpoints.rs (transaction start151), checkpoints/agents.rs and checkpoints/reconciliation.rs | S5; the two previously unallocated roots are assigned to S5 | Keep the complete checkpoint operation atomic; use scoped capture/points, no segmented lifetime fallback. |
| store/sandbox_maintenance.rs (starts141/216), sandbox_maintenance/transaction.rs, consented.rs and retained_availability.rs | S5; previously unallocated roots assigned to S5 | Carry the same scope through evidence and domain updates; no direct runtime/home mutation from an observation. |
| store/sandbox_reconciliation/transaction.rs, agents.rs and sandbox_unpublished_recovery/transaction.rs | S5, existing allocation | Start/pass the scope through owned reconciliation paths and retain outcome/receipt rules. |
| store/sandbox_worker_store.rs (transaction start126) | S1, existing allocation | S1 adapts the owned root to the S5 scoped reconciliation/evidence interface; S5 supplies exact function name/signature without editing this S1 file. |
| store/agent_log_segments/transaction_scope.rs | S2 | Add a narrowly typed collection action if a caller needs another operation. Never expose transaction lifecycle through an arbitrary callback/raw session while a proof remains valid. |

S5 should own its scoped function signatures in the already-owned boundary files and publish them for S1/S4. S1/S4 roots adapt to those signatures. No overlapping edits to lifecycle_transaction.rs or sandbox_worker_store.rs. Keep legacy behavior on the legacy branch and gate new segmented behavior; a skipped segmented path is an activation blocker, not compatibility success. Tests must show source/context mismatch and transaction restart refuse before writes, successful publication shares the caller transaction, and rollback removes both caller and log side effects.

Next source checkpoint: Wed2026-11-11 21:00 SGT. The source freeze is lifted for separately reviewed authorized PRs; this segmentation integration still has no merge relay. No production action.
