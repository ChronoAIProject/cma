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
# Ticket 13 metric handoff (2026-11-03)

Ticket 13 owns `web/src/agent/suggestionMetrics.ts`. It emits the local browser
`cma:suggestion-metric` CustomEvent with distinct `exposure`, `send_confirmed`,
and `task_outcome` kinds. Exposure requires a current enabled set at least half
visible in a visible document. Confirmation requires the existing command/run
receipt; task outcomes require that receipt's actual durable turn outcome.
No raw message text, credentials, or inferred task success is emitted. Dedupe
and pending correlation are bounded to 100 entries / 30 minutes in this tab.
This is instrumentation, not an analytics collection service or durable funnel.

Ticket 12 may call `confirmSuggestionSend(set, itemId, 'edited', receipt)` only
after normal-send admission, including recovered admission. Original sends use
`'original'`. `messageSubmission` now forwards its existing optional
`MessageAcceptance` to the accepted callback; no wire format changes. Ticket 13
does not implement append/replace/undo/cancel or edited recovery. Literal edited
UI wiring and its runtime evidence belong to 12. `useSuggestionOutcomes` remains
on AgentSurface after the suggestion group disappears.

For an uncertain edited request, 12 may retain the existing journal key with
`rememberSuggestionRequest(set, itemId, 'edited', requestKey)`. This records no
confirmation event. The surface's metric observer confirms it only when that
same retained request exposes an accepted receipt; it never retries or changes
the request. Original recovery uses this same bounded tab-local correlation.

Metric denominators: one exposure per viewer/Agent/set within the tab's dedupe
window; confirmations are deduped by that scope, item and admitted command/run.
Original versus edited share the confirmed-send denominator. Task completion
rates use only receipt-correlated resolved outcomes; pending/unobserved outcomes
remain unknown. Failed generation and mere clicks contribute no exposure or
confirmation. These events have no remote collector and do not claim durable
cross-tab or cross-reload analytics.
