# S4 response projection facts

Part of #1350; consumed by the #1348/P1-4 indexed readers. Narrow dependency
branch: `tl/cma-log-segments/response-facts`, based on shared projection commit
`11f4fcc6984ab9cf0f2099563e61035e53eb0f6b`. No writer activation is enabled.

The public Layer2 facts are populated by the existing
`AgentLogProjectionState::observe` fold. Both `plan_publication` and
`BoundedAgentLogFold` use that exact implementation, so original publication,
bootstrap and audit produce the same indexed Turn evidence.

| Field | Contract |
| --- | --- |
| `AgentLogProjectionState::settings_changed: bool` | Monotonic: any earlier `OperationDeliveryChanged(UpdateSettings, ..)` sets it, regardless of delivery state. Mirrors historical model replay. |
| `AgentLogTurnFacts::legacy_model_known: bool` | Captured as `!settings_changed` at TurnStarted. Never changes later. An explicit `model` always wins; absent model may use the immutable creation profile only when this is true. Otherwise report unknown. |
| `response_usage: Option<AgentTurnUsage>` | Latest response usage tokens inside `[this TurnStarted, next TurnStarted)`. Accepts legacy Usage and UsageUpdated matching this turn, including events after its terminal. A later turn's interval never updates this response fact, even if UsageUpdated explicitly refers to the older turn. Existing transcript/lifetime usage fields keep their semantics. |
| `response_usage_sequence: Option<u64>` | Original global sequence of that exact usage event. Present iff response_usage is present; point-prove the original row when consuming it. |
| `prewrite_failure_sequence: Option<u64>` | Original SendMessage/PreWriteFailed event immediately before this TurnFailed, same timestamp, with operation_sequence+1 equal to this turn. The original terminal reference remains `end_sequence`. |

Deferred failure references are evidence, not permission to classify a command as
queued. Consumers must still compare original failure class/detail against the
command's deferred-failure binding and current nonterminal command status,
preserve visibility/redaction, and recheck authority. No synthetic terminal or
model/usage value is inferred from today's aggregate or lifetime totals.

A private scalar pending pre-write reference survives a publication boundary and
is cleared on the next event. No growing history collection is added. All new
serialized fields are required, including nullable fields: old/missing shapes
fail closed instead of silently supplying false provenance or missing usage.
Existing unactivated projection generations need rebuild; this is not a rolling
compatibility or activation claim. The generic Mongo record envelope is unchanged.

Tests cover historical settings timing, explicit models, post-terminal usage,
next-turn cutoff, mismatched prior-turn usage, exact/foreign/nonadjacent/different-
time pre-write references, serialized omission/forgery refusal, bootstrap versus
publication parity, and refused publication preserving its source state.
