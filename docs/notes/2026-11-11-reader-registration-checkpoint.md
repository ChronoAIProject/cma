# S1 reader registration checkpoint

Based on S1 source `96d432ef3`, with its own normal shared-contract merge.
Registers segment reader/store modules, permits only scalar Agent envelope1/2,
and invokes verify-only sidecar startup checks. Adds the six sidecars to the
existing exact AM role/telemetry inventory; no live grant or DDL is performed.
The envelope predicate keeps wrong-type/array rejection and all other schemas1.

This is a dependency checkpoint, not a review-ready foundation. S1 must combine
S4 point/visitor delegations and the frozen captured-source signatures; S2 still
must register the additive migration through the current multi-manifest lease
runner and its authority/backup tests. No startup auto-creation of sidecars is
introduced. Source formatting is checked here; Rust/Mongo gates are pending.
S1 can normally merge this exact dependency commit without importing S2/S5 code.
Develop freeze remains active.
