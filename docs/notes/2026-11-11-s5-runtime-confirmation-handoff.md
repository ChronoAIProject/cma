# S5 restricted runtime handoff

Source inspection at e9b134d8c, Wed 2026-11-11. This is an interface/ownership request, not a production recovery claim.

The S5 model port already requires `reattach_existing(&RecoveryClaim) -> (Attachment, RecoveryConfirmation)` and acknowledges only after an exact durable completion for the same attempt. `RecoveryConfirmation` binds the original request, native/home/runtime target, native writer fence and confirmed Idle/Working/WaitingApproval state. Its model runtime is explicitly a stub.

The production `ConversationChannel` in `api/crates/layer3-adapter/src/conversation_channel.rs` contains only an input sender and optional conversation ID. `SandboxAdapter::reconnect_agent_with_prompt` in `adapter.rs` returns that channel and has no native-state confirmation. The hosted adapter validates the original open receipt and native identity, but that does not independently prove current Idle/Working/WaitingApproval. Inferring native state from the persisted active-turn field would turn a model assumption into false production evidence.

Ordinary `agent_worker/startup_attachment.rs::reattach` also sends pending managed-context input and settings, then `agent_worker/connection.rs::try_finish_connection_publication` can schedule queued first input or pending skill work. Capacity recovery must not use either as its fallback. The live registry has staged attachment/activation primitives, but after a crash at durable completion the exact retained output must be reattached and activated without that ordinary route.

## Exact adapter entry and delegation path

The requested shared entry is in **`api/crates/layer3-adapter/src/adapter.rs`**:
`SandboxAdapter::reconnect_agent_with_prompt` (line 389 at this source) currently
returns only `ConversationChannel`. Prefer an additive restricted method beside
it, provisionally `SandboxAdapter::reconnect_agent_for_capacity_recovery`, with
an unsupported default until its backend supplies authoritative confirmation.
Do not weaken or reinterpret the existing reconnect contract. The segmentation
lead owns the choice of implementer and final interface name.

| File | Existing function/type | Required handoff |
| --- | --- | --- |
| `api/crates/layer3-adapter/src/adapter.rs` | `SandboxAdapter::reconnect_agent_with_prompt` | Restricted no-input reconnect entry returning native confirmation, not an inferred status. |
| `api/crates/layer3-adapter/src/conversation_channel.rs` | `ConversationChannel` (line 367) | Add a separate result/observation type if appropriate; the current channel carries only input and conversation ID. |
| `api/crates/layer3-adapter/src/codex/adapter.rs` | `impl SandboxAdapter for CodexAdapter::reconnect_agent_with_prompt` (line 383) | Forward the restricted entry without falling back to ordinary reconnect. |
| `api/crates/layer3-adapter/src/hosted/adapter_sandbox.rs` | `impl SandboxAdapter for HostedAdapter::reconnect_agent_with_prompt` (line 368) | Forward to the restricted hosted implementation. |
| `api/crates/layer3-adapter/src/hosted/adapter_agent.rs` | `reconnect_agent` (line 208), `open_agent` (line 254) | Preserve original receipt/fence validation; restricted mode must never use a launch/new-conversation fallback. |
| `api/crates/layer3-adapter/src/hosted/backend.rs` | `HostedBackend::open_agent` (line 341) | Supply or route backend confirmation bound to the retained native target and output boundary. An open receipt alone is insufficient. |
| `api/crates/layer3-adapter/src/hosted/acp.rs` | `Bridge::open_with_subscription_and_prompt` (line 174) | Keep retained output staged until exact durable completion; expose authoritative confirmation when the protocol supports it. |
| `api/crates/layer3-adapter/src/hosted/acp_handshake.rs` | `Bridge::{handshake,handshake_inner}` | Restrict handshake to identity/state confirmation; forbid settings/context/prompt replay. |

These are the exact existing path, not a request to edit all of them blindly.
If native state must instead come from a runtime ledger/control query, its owner
must identify that authoritative producer and expose the same restricted result.
The output must distinguish idle, active prompt and approval wait, bind the exact
native identity/fence, and preserve the retained output cursor. Persisted
`Agent.active_turn`, a valid open receipt, or a successful socket attach is not
independent native-state evidence.

S5's owned consumer remains
`api/crates/layer2-core/src/sandbox/agent_capacity_recovery/ports.rs::CapacityRecoveryRuntime::reattach_existing`.
Its CMA implementation will live in the allocated
`api/crates/cma/src/sandbox/agent_worker/capacity_recovery.rs` and use staged
`LiveAgentRegistry::{attach,publish_and_activate}` only after the exact durable
completion. The ordinary `AgentWorker::reattach` and
`try_finish_connection_publication` functions are explicitly excluded.

Requested allocation: let S5 implement the minimal adapter/provider handshake that returns independently confirmed native state for the retained conversation, or assign a runtime owner to supply it. Shared adapter/hosted/Codex trait and delegation files are not in S5's current manifest. New focused implementation children and their tests must stay below 500 lines; shared registrations remain with their allocated owner. No adapter files have been edited for this request.

Required producer behavior: fence the old writer; require the exact original open receipt and retained conversation/runtime/home; attach only the existing native conversation; return confirmed current state and active prompt identity; expose no Message, Steer, settings/context replay, first-message dispatch, new conversation, replacement or implicit launch fallback. Unsupported proof must refuse. The caller must preserve durable output before acknowledgement and retry the same retained target after uncertainty or crash.

S5 can continue storage-side work while this ownership is resolved. The new source checkpoint has scoped accepted-work claim/retry/completion helpers and exact work-carrier CAS tests. It still needs root transaction/outcome reconciliation, runtime assembly and real-Mongo qualification. Existing unit tests prove receipt idempotence after live completion; they do not prove post-crash production registry activation.

## Allocated contract checkpoint

The integration owner has allocated the focused Layer 3 implementation/test
children to S5 and shared trait/export/dispatch wiring to S2. The neutral contract
is now in `layer3-adapter/src/agent_capacity_recovery.rs`, with the restricted
hosted path in `hosted/capacity_recovery.rs`. Seven authored contract/transport
tests cover identity/fence/challenge changes, exact prompt state, output gaps,
invalid or absent authority, typed unavailable/unsupported results and zero input
sends. They await shared module registration before execution.

The request carries no input or settings. It pins original open receipt digest,
home, provider generation/boot, independent conversation generation, original
active turn and writer fence, plus a fresh attachment challenge. Confirmation
must bind that exact request and independently produced native state to a retained
output interval with no acknowledgement gap. A native terminal result for a still
admitted active turn is refused for separate durable output reconciliation.

The shared trait/export/dispatch edits have been integrated by S2. The consumed
patch is replaced by the executable source. Shared roots delegate runtime inspection,
finality checks and protected filesystem operations to focused children so the
adapter roots remain below 500 lines. Registration does not enable a native producer.

This checkpoint validates a producer result; it does not invent the producer.
The backend hook defaults absent and the restricted call returns typed Unsupported
without ordinary reconnect. `layer4-runtime/src/supervisor.rs::AgentProcess` holds
its original launch and bridge, while `bridge.rs::Bridge` retains output but does
not expose independently tracked prompt/approval state. `RuntimeStatus` reports
process counts and readiness, not that native state. A native producer therefore
needs the runtime owner's retained-process inspection/attachment path, including
existing-process-only fencing and atomic state/output observation. S2 must route
that exact Layer 4 protocol/dispatcher work; no Layer 4 file has been edited here.
No successful production native handshake or activation is claimed.
