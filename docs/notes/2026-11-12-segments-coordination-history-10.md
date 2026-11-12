# Runtime-loss source ownership and pin history

## Lead allocation 2026-11-12 02:35 — S4 runtime-loss wiring

S4 now owns the production `sandbox_reconciliation::transaction::apply_scoped`
entry, its scoped publisher and `capture_segmented_scoped`, including their
success/failure qualification. S2 stops editing those semantic paths and retains
integration, shared registrations and P1-2. S5 retains capacity recovery service,
codec, discovery and native proof. S1 owns the underlying captured-proof contract.

Handoff source is already pushed: `c13eff3b58edcc70b6ac08b52676ba5a1790f162`
contains the production dispatch in `api/crates/cma/src/store/sandbox_reconciliation.rs`.
The published combined reference is `e7e4cdda0eb94ca04ff977a442b25ddc1a4b8e8a`.
S4 at `8958e69ff31ed235e3bf25a58218cb807f2e99a0` already contains
`ecd84ecd3` and `5c255dfd9`; its four supporting files below are byte-identical to
that reference. The only missing production delta is the root dispatch (33 lines
in the original commit). S4 can consume that exact root delta in its own narrow
wiring commit; it need not reimplement the scoped helpers or import unrelated
preflight/unpublished-recovery edits from the three-file `c13eff3b5` commit.

Transferred exact paths, all under `api/crates/cma/src/store/`:

- `sandbox_reconciliation.rs`
- `sandbox_reconciliation/transaction.rs`
- `sandbox_reconciliation/transaction/scoped.rs`
- `sandbox_reconciliation/agents.rs`
- `sandbox_reconciliation/agents/scoped.rs`
- `agent_capacity_recovery/source.rs`

The source file includes raw/scoped capture and envelope-fence helpers shared
with S5. S4 owns edits to this whole file; S5 requests changes through S4, rather
than editing it concurrently. `agent_capacity_recovery/lineage.rs` stays S5-owned.

Preserve one owned transaction scope from captured v2 source through
`writer_commit::{prepare,persist}`; release it only before unchanged legacy work
in the same transaction. The format lookup is a dispatch hint, never proof or
adoption authority. Legacy races must refuse v2; terminal children are fenced;
all domain and driver failures abort the transaction at the root. Keep the
original commit/unknown-outcome reconciliation and no read-side audit writes.

S4 qualification: actual runtime-loss root with a mixed v1/v2 Sandbox and
nonempty segmented child changes, unchanged terminal child, malformed/stale
capture refusal with unchanged durable state, parent CAS race and lost commit
reply. No helper-only or empty-child test qualifies this production root.
Target source checkpoint Thu 04:00 SGT, focused Mongo result by 08:00 if the shared
slot permits; these are integration targets awaiting S4 confirmation, not passes.
