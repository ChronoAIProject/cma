# Suggestions handoff: tickets 09, 11, 12 and 13

Ticket 11 uses the same `ConversationMessage.finalAnswer` boolean being implemented
by ticket 09. The immutable canonical answer identity is Agent ID + durable message
`sequence`, associated with `runId` (the turn sequence). No second completed-answer
cache is retained in `ConversationState`. Both durable replay and transcript hydration
must mark only the final nonempty assistant message of a successfully completed turn.
Worker 14's operation-output fields remain independent.

`web/src/agent/suggestions.ts` defines `SuggestedReplySet`: stable set ID, viewer ID,
Agent ID, answer ID, numeric `{turn_sequence, answer_sequence}` scope, source
`{kind: template | generated, version}`, and ordered `{id, text, rank}` items.
Ticket 11's initial scenario is an explicit English `Compare … and/versus/vs …`
request followed by a durable successful answer. It produces one comparison-criteria
template, labelled Scenario templates. Unmatched requests produce zero items.
Template identity and text rebuild deterministically; no model generation is claimed.

Ticket 12 consumes the same set/items for editing, preserving draft and attachments.
Once the answer expires, edited text remains ordinary manual input. Ticket 13 supplies
persisted contextual sets through this contract, with a new source/version and stable
item IDs; its separately authorized generation and quality validation are not implemented
by ticket 11. Invalid or stale sets must fail closed before rendering/sending.

The public conditional-send shape is `answer_context: {turn_sequence, answer_sequence}`
on Responses v2 creation, and `forwardedProps.answerContext` with that same object on
AG-UI run. These are normal sends, with normal authority and retained request keys.
The condition is part of the canonical command fingerprint. New admissions validate
the latest successful final answer atomically with queue and permission fencing;
same-key recovery returns the original authorized receipt before checking staleness.
Public contract additions are additive; legacy clients omit the condition.

The suggestions UI is a separate component inserted in AgentSurface's existing
MessageStream children after the answer/artifacts. It does not change StreamEntry or
the timeline projector. The Web Style rule was committed before UI implementation
as `078b9bafe`.
