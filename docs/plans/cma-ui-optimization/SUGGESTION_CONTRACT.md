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
An older Responses server's explicit `responses_field_unsupported` refusal closes
the scoped attempt and leaves manual input available. Uncertain errors retain the
original scope and key. No path silently strips the condition or falls back to an
unconditional send. Accepted AG-UI scope receipts also suppress the old group after
refresh when the history read has not caught up.

The suggestions UI is a separate component inserted in AgentSurface's existing
MessageStream children after the answer/artifacts. It does not change StreamEntry or
the timeline projector. The Web Style rule was committed before UI implementation
as `078b9bafe`.

## Ticket 12 editing handoff

`AgentSurface` passes the validated set into `useSuggestionEditing`; the existing
`SuggestedReplies` component exposes an optional `onEdit(text)` action. The real
`ComposerSuggestionEditor` opens the standard conflict dialog or fills an empty
composer, retains attachments and provides replacement undo. Cancel and dismissal
return focus to the composer. No generated-set fetch, generation worker, new set
identity, analytics or message transport is introduced here.

The composer submits through `messageSubmission` and the existing selected
controller. While the edited set is still eligible, this is the existing normal
send with its existing answer condition. Acceptance retires the group; editing
alone does not. An expired source or an explicit scoped refusal detaches that
association and keeps the manual draft. Only a subsequent deliberate send can
submit it as ordinary input, with current authority; there is no automatic
unconditional retry. Uncertain requests keep their original body, condition and
key. Changed text remains separate until recovery and then uses the normal fresh
intent path. Ticket 13 can continue supplying its persisted sets at the unchanged
`AgentSurface.suggestedReplies` boundary; it need not implement another editor.
