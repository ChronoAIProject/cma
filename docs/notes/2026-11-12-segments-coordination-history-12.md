# Superseded combined queue and runner evidence

### Integration source and gate queue — c3ca9e3aa

Normally merged develop `cf1d6ce54` and resolved three overlapping declarations
by preserving both sides: configuration documentation inventories, default-off
reconciliation and pacing flags, and accepted recovery plus event-driven Agent
scheduler composition. Source fmt and config documentation passed. The one
combined branch is published at `c3ca9e3aa`; no develop merge occurred.

After the current-proof Mongo run completes, the immutable combined export
queues strict CMA, scheduler/config, adapter, canonical authenticated Mongo and
retained-fixture Mongo separately under heavy-1. A failure stops this batch for
inspection. These are pending checks, not source-head passes. Actual proof unit
results remain at `44ad38198`. Both shared lock files are preserved.


Runner qualification at `c3ca9e3aa`: **31 passed, 0 failed, 0 skipped**,
172.707s. These are mocked Cargo/Docker orchestration, selection and cleanup
contracts; they are not actual Mongo evidence. Formatting/config checks passed;
base-to-working-tree whitespace passes after trimming the history6 EOF blank.
The canonical secret scan at `b0b4943c5` passed3,977commits with zero findings.
