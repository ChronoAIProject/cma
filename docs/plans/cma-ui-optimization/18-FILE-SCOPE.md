# Ticket 18: final file scope

Branch: `codex/cma-ui-18`; worktree: `/Users/liuyitian/code/cma-ui-worktrees/ticket-18`.
Started at `d19c225bd`; latest merged integration is `7cd701d0f`.
The original ticket and U/A/R seams were user-approved. Completion evidence is
[18-EVIDENCE.md](18-EVIDENCE.md); the original acceptance text is unchanged.

Reuse the public Agent event snapshot/head/finite tail and typed SDKs, existing
AgentPanel/onSelect/AgentComputerObservation, and recorded bounded output. The
only environment capability change is read-only operation replay. The existing
Browser context, browser worker and control boundary remain independent.
No Layer 4 source or runtime image is changed. No API route is added.

The shared JSON inventory and new corpus are part of this manifest, alongside
real public-handler scenario setup and typed TypeScript/Rust dispatch coverage.
The latest merge preserves all incoming inventory operations. No other worker's
source, original workspace or integration worktree was edited.

## Exact manifest

Relative to integration `7cd701d0fe6c2496256a01eca59919ee8d899c82`, including this
manifest and completion documentation, the ticket changes these 37 paths:

- `api/crates/layer1-public/src/agent_v2/mod.rs`
- `api/crates/layer1-public/src/agent_v2/operation_replay_tests.rs`
- `api/crates/layer1-public/src/rest/public_contract_tests/agent_event_cases.rs`
- `api/crates/layer1-public/tests/fixtures/public-contract-inventory.json`
- `api/crates/layer1-public/tests/fixtures/public-contract/operation-replay.json`
- `api/crates/layer2-core/src/sandbox/agent_environment.rs`
- `api/crates/layer2-core/src/sandbox/agent_filesystem.rs`
- `api/crates/layer2-core/src/sandbox/agent_filesystem_tests.rs`
- `api/crates/layer3-adapter/src/codex/operation_replay.test.cjs`
- `docs/API_Agent_Events_V2.md`
- `docs/API_REST.md`
- `docs/Web_Style.md`
- `docs/plans/cma-ui-optimization/18-EVIDENCE.md`
- `docs/plans/cma-ui-optimization/18-FILE-SCOPE.md`
- `docs/plans/cma-ui-optimization/IMPLEMENTATION.md`
- `docs/plans/cma-ui-optimization/README.md`
- `docs/plans/cma-ui-optimization/issues/18-operation-replay.md`
- `sdk/rust/docs/agents.md`
- `sdk/rust/tests/public_contract/agent_events_v2.rs`
- `sdk/typescript/docs/agents.md`
- `sdk/typescript/tests/public-contract/agent-events-v2.ts`
- `web/e2e/operation-replay.check.ts`
- `web/e2e/operation-replay.config.ts`
- `web/src/Console.tsx`
- `web/src/agent/AgentSidePanel.tsx`
- `web/src/agent/AgentSurface.tsx`
- `web/src/agent/MessageStream.tsx`
- `web/src/agent/OperationReplay.test.tsx`
- `web/src/agent/PlanCard.tsx`
- `web/src/agent/StreamEntry.tsx`
- `web/src/agent/environment/OperationReplay.tsx`
- `web/src/agent/environment/observation.ts`
- `web/src/agent/environment/replayError.ts`
- `web/src/agent/environment/replayIndex.ts`
- `web/src/api/client.ts`
- `web/src/api/rest.ts`
- `web/src/resources/SharedAgent.tsx`

## Handoff

All ticket-owned source files stay at or below 500 lines and tests below 2000.
The known full-repository size failures are separately recorded in the evidence.
No uncommitted source or owned running proof service remains at handoff. The
worktree, private Rust target and external proof logs are retained for parent
cleanup. A separate merger integrates the branch; no push, deployment or PR.
