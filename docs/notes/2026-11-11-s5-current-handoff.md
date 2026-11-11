# S5 capacity recovery checkpoint

Branch: `tl/cma-log-segments/capacity-recovery`. Part of #1351 and recovery
consumers in #1349. No S5 PR, review verdict, merge relay or production activation.
This is an implementation checkpoint; the full slice is not PR-ready.

## Executed evidence

All 45 core recovery tests passed, including crash/retry, exact completed-attempt
receipts, no replay, revoked authority, parent retirement and fence replacement.
All three public HTTP dry-run tests passed: exactly five metadata fields, unchanged
Agent/log, anonymous/foreign/shared authority refusals and no-store on errors.
The HTTP fix/test commit is `e3b9f8534`.

The two commands ran under existing heavy-1 with two jobs, one target and bounded
180-second runs. Current strict Clippy and CMA compilation are still queued for
either heavy slot. No Mongo fixture has been created for this checkpoint yet.
The test counts are actual execution; new receipt tests below remain unrun.

## Source and dependencies

Normally consumed S2 recovery/scoped registrations `34c9a3228` / `9e5c47a72`,
canonical Mongo registration `9011a3796`, and focused integration repair
`6123c0ecf` (local cherry-pick `105da041b`). The restricted Layer 3 contract and
minimal shared patch are published at `d9a0335d4`.

S1's published executable capture checkpoint `62c1bdcd5` is now normally merged.
Its source/gate note reports 8 focused unit and 36 real-Mongo passes. Those are
S1's dependency results, not a claim that S5 end-to-end recovery has passed.
S1 source certification is no longer a missing implementation on this branch.

`ccbaa17d4` adds bounded restart receipt verification. The work carrier now has
BSON `format_version: 2` and a strict `result` object: original Agent result hash,
log-batch hash, result revision, through sequence and publication timestamp.
No Agent snapshot or user input is copied. The S4 `result_digest` binds this
witness plus the entire recovery work under a distinct v2 result domain.
Earlier draft carrier rows are refused; no production v1 recovery carrier exists.
The domain `RecoveryWork.format_version` remains 1.

Status and settlement use the same exact receipt reader. It verifies original
admission identity, exact work revision and result binding, the current certified
segmented source, publication/seal coordinates and the retained completion event.
Later Agent/head movement or stop does not replace the original result. Missing
publication is unavailable/unknown, never known-aborted. A completed receipt
allows receipt/status reconciliation only; it does not activate a native channel.
Seven new pure codec/receipt tests and the updated three Mongo carrier tests are
registered, but compilation/execution are pending.

## Runtime and shared-file handoff

Exact functions and the supplied patch are documented in
`docs/notes/2026-11-11-s5-runtime-confirmation-handoff.md` and
`docs/notes/2026-11-11-s5-adapter-shared.patch`.
S2 owns shared Layer 3 trait/export/hosted/Codex wiring; S5 owns the focused neutral
contract and hosted validation/test children. The new children remain unregistered
until S2 integrates that patch. Seven authored Layer 3 tests have not executed.

The primary method is
`api/crates/layer3-adapter/src/adapter.rs::SandboxAdapter::reconnect_agent_for_capacity_recovery`.
Its hosted path is
`api/crates/layer3-adapter/src/hosted/adapter_sandbox.rs::reconnect_agent_for_capacity_recovery`,
with the optional native hook in `hosted/backend.rs::HostedBackend` and Codex
forwarding in `codex/adapter.rs`. Shared roots need the owner's size refactor.
Layer 4 must supply retained-process-only native state/output observation and
attachment. Supervisor AgentProcess/Bridge currently have no independent
prompt/approval-state proof. No ordinary reconnect, settings/context replay,
PublishConnection or inferred native state is permitted.

## Remaining implementation and integration

- S4 must accept the independently admitted CapacityRecovery empty-batch successor
  for an already segmented source; its current writer_prepare still permits only
  Rollover admission for that source. Legacy staged recovery is separately gated.
- S5 must finish production transaction/outcome and accepted-work worker assembly,
  durable unknown-attempt reconciliation, native channel activation after durable
  completion and the post-completion crash path. Receipt validation alone does
  not complete those responsibilities.
- S2 must wire production services, admission/status routes and bounded carrier
  discovery plus migration/schema/grant/backup inventory. Carrier schema must use
  the v2 result witness above, not the earlier draft shape. Keep all feature gates
  independent and default-off.
- Actual S5 Mongo success/failure/crash/no-replay and compatibility qualification,
  strict CMA lint and all final repo gates remain required before any PR-ready claim.

## Resources and persistent evidence

One target is retained for the immediately queued gates: `api/target/seg-3`.
No per-gate target copies, disposable Mongo containers or volumes are retained.
Shared heavy locks are only opened with `lockf -k -n`, never recreated/deleted.
The coordinator tries either free slot, executes one gate, then releases it.

Local notes: `2026-11-11-1351-unit-gates.log`, `2026-11-11-1351-unit-gates.py`,
`2026-11-11-1351-acquire-gate.py`, and `2026-11-11-1351-targeted-mongo.py`.
The targeted fixture uses the canonical pinned Mongo 8.0.13 image, 0.25 GiB cache,
2 GiB container limit, random owned labels/volume and verified cleanup. It never
accepts a supplied production/existing Mongo URI. No full recovery or activation
acceptance is claimed by carrier-only tests.
