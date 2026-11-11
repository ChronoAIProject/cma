# S5 restricted runtime handoff

Source inspection at e9b134d8c, Wed 2026-11-11. This is an interface/ownership request, not a production recovery claim.

The S5 model port already requires `reattach_existing(&RecoveryClaim) -> (Attachment, RecoveryConfirmation)` and acknowledges only after an exact durable completion for the same attempt. `RecoveryConfirmation` binds the original request, native/home/runtime target, native writer fence and confirmed Idle/Working/WaitingApproval state. Its model runtime is explicitly a stub.

The production `ConversationChannel` in `api/crates/layer3-adapter/src/conversation_channel.rs` contains only an input sender and optional conversation ID. `SandboxAdapter::reconnect_agent_with_prompt` in `adapter.rs` returns that channel and has no native-state confirmation. The hosted adapter validates the original open receipt and native identity, but that does not independently prove current Idle/Working/WaitingApproval. Inferring native state from the persisted active-turn field would turn a model assumption into false production evidence.

Ordinary `agent_worker/startup_attachment.rs::reattach` also sends pending managed-context input and settings, then `agent_worker/connection.rs::try_finish_connection_publication` can schedule queued first input or pending skill work. Capacity recovery must not use either as its fallback. The live registry has staged attachment/activation primitives, but after a crash at durable completion the exact retained output must be reattached and activated without that ordinary route.

Requested allocation: let S5 implement the minimal adapter/provider handshake that returns independently confirmed native state for the retained conversation, or assign a runtime owner to supply it. Shared adapter/hosted/Codex trait and delegation files are not in S5's current manifest. New focused implementation children and their tests must stay below 500 lines; shared registrations remain with their allocated owner. No adapter files have been edited for this request.

Required producer behavior: fence the old writer; require the exact original open receipt and retained conversation/runtime/home; attach only the existing native conversation; return confirmed current state and active prompt identity; expose no Message, Steer, settings/context replay, first-message dispatch, new conversation, replacement or implicit launch fallback. Unsupported proof must refuse. The caller must preserve durable output before acknowledgement and retry the same retained target after uncertainty or crash.

S5 can continue storage-side work while this ownership is resolved. The new source checkpoint has scoped accepted-work claim/retry/completion helpers and exact work-carrier CAS tests. It still needs root transaction/outcome reconciliation, runtime assembly and real-Mongo qualification. Existing unit tests prove receipt idempotence after live completion; they do not prove post-crash production registry activation.
