# S4 shared-root merge compatibility proposal

Part of #1350. Source pin `5fdfae28133246ffbeb1d6bc5689513139d9cf95`.
This patch is prepared for S2 ownership handoff and is **not applied**.

The required develop merge made the retained/unpublished inspection generation
optional. The segmented branch still compares its u64 maxima with that Option,
so both strict Clippy and focused unit compilation stop with two E0308 errors.
The proposal matches S2's existing combined-source decisions: only unpublished
recovery checks the generation, and segmented retained reopen refuses until its
original bounded history proof is implemented. It changes no public signature,
activation policy, admission gate or reader format.

```diff
--- a/api/crates/cma/src/store/sandbox_unpublished_recovery/transaction.rs
+++ b/api/crates/cma/src/store/sandbox_unpublished_recovery/transaction.rs
@@ -135,9 +135,16 @@
             Ok(Some(source)) => {
                 // These are full-prefix certified maxima. A missing point row
                 // never proves the proposed generation was unpublished.
-                if source.max_opened_generation() >= generation
-                    || source.max_retired_generation() >= generation
-                {
+                if require_reopen {
+                    observation.finish(Reason::Unavailable);
+                    return Err(ApplyError::Domain(SandboxStoreError::Unavailable(
+                        "segmented retained reopen requires its original bounded history proof".into(),
+                    )));
+                }
+                if require_unpublished && generation.is_some_and(|generation| {
+                    source.max_opened_generation() >= generation
+                        || source.max_retired_generation() >= generation
+                }) {
                     observation.finish(Reason::PublishedGeneration);
                     return Err(ApplyError::Domain(SandboxStoreError::InvalidTransaction(
                         "runtime generation was already published",
```

After the narrow pin or handoff, rerun strict CMA with warnings denied, the focused
writer units, real-Mongo writer/lease/legacy roots, and the existing retained
assessment/reopen contracts under the authorized release slot. Keep two jobs,
pause below10% available memory, and preserve all shared lock inodes.
