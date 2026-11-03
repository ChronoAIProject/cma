# Suggestions handoff: tickets 09, 11, 12 and 13

Ticket 11 uses ticket 09's `ConversationMessage.finalAnswer` and the backend's
`AgentTranscriptTurn::final_answer()`. The immutable canonical answer identity is Agent ID + durable message
`sequence`, associated with `runId` (the turn sequence). No second completed-answer
cache is retained in `ConversationState`. Both durable replay and transcript hydration
must mark only the final nonempty assistant message of a successfully completed turn
with no missing output frames. `latestTurnId` prevents an old answer from becoming
eligible again after a newer empty or failed turn; it is not a second answer identity.
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

`AgentSurface.suggestedReplies` is the delivery seam: omitted uses the bounded
template, `null` means unavailable/loading/no suggestions, and a supplied set must
match the current viewer, Agent, turn and final answer. Sets contain one to three
unique nonempty items with strictly increasing nonnegative integer ranks. Text is
rendered and sent without trimming, including newlines and trailing spaces; empty
or oversized text is rejected. A supplied invalid set hides only the suggestion
region. Ticket 13 owns generation, persistence, cancellation and quality evidence;
ticket 11 does not fetch or manufacture generated candidates. The three-candidate
U test is explicitly a supplied boundary fixture, not production generation.

Ticket 12 must assign a fresh request identity to a changed edited intent and must
not replace an unconfirmed request's retained text. Recovery uses the original
text, scope and key. Any edit after the source answer expires is ordinary composer
input, not permission to bypass conditional admission.

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
