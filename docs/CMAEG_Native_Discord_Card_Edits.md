# Native Discord card edits

`updateMessage` can replace an issued native Discord card on its original bot
message when `CMAEG_DISCORD_CARD_EDITS_ENABLED=true`. The flag defaults to false,
including in the checked-in CMA manifest. The existing public request and receipt
shapes are unchanged. This path uses the card's sealed interaction reference;
ordinary native text-edit references retain their existing text/link behavior.

## Supported replacements

An original native card can be updated before or after its human callback.
Buttons, single-choice menus and multi-choice menus use the existing bounded
component renderer: at most five rows, twenty options per choice, and one message
of at most 2,000 UTF-16 code units. Ordinary structured text degradation remains
as documented for native controls. A text/link-only replacement explicitly sends
`components: []`, removing all old controls. Its returned card reference permits
subsequent edits to that same message.

Native attachments, caller-supplied reply targets, new thread destinations and
multipart replacements are refused before an edit claim or platform request.
A retained thread card stays in its original thread even when the replacement
omits `thread_reply`; setting it on an ordinary card is unsupported. Editing does
not grant thread creation, membership, history, delete or reaction authority.

Starting an edit chain for a named consumer additionally requires the existing
consumer-interaction issuance gate. Adding a multi-choice control requires the
multi-select issuance gate unless the prior card already has a multi-choice
control. These are gates for new authority: retained successor edits and exact
operation replays remain available after card-edit issuance is disabled, and
existing multi-choice cards can still be replaced with multi-choice controls.

## Authority and concurrency

The stored card binds the original actor, bot, consumer, provider, conversation,
partition, source revision, platform message, current card revision and original
24-hour expiry. Replacements preserve these values. The authenticated tool caller
must still own the reference's consumer, resolved from the current channel
record. Source/provider/consumer revocation is checked before both new edits and
saved-result replay. Bot-token custody and exact current channel/thread metadata
are authenticated before a new edit. A different thread parent, guild or type is
refused; an ordinary channel cannot silently become a thread destination.

A compare-and-swap on the prior card simultaneously marks it superseded and saves
an optional edit claim containing the canonical request hash and successor card
ID. Competing edits and human clicks use the same record version. A losing edit
cannot PATCH Discord. Once an edit wins, new callbacks for the old revision are
refused. A callback already consumed before replacement retains its complete
selected result and can finish exact inbox recovery after replacement; a changed
callback ID, action or selection cannot reuse that exception.

The successor starts with no consumed result and no bound message reference. CMA
sends one original-author PATCH containing the complete `content`, `components`
and `allowed_mentions: {parse: [], replied_user: false}`. It checks the returned
message ID, channel, bot author, content and complete authored components before
binding the successor. Discord-generated component IDs and omitted false option
defaults do not change the authored component comparison. A confirmed successor
keeps the same platform message and original expiry; replacement never renews it.

## Retry and uncertain writes

The old card retains the final `IntentReceipt`. Retrying the same original
reference and canonical replacement body returns that exact saved receipt,
including the same successor reference. A different body on the old reference is
refused. Further updates use the successor reference returned by the confirmed
operation. The claim, result and revision chain survive gateway restart.

A claim without a final receipt is reported as unknown and is never resent.
Timeouts, lost or malformed platform responses, mismatched components, failed
successor binding and failed receipt persistence do not reactivate the old card.
A retry may recover a committed receipt whose storage acknowledgement was lost;
it never repeats the PATCH. Definite rejected/rate-limited attempts also retain
the old revision fence and their outcome, so retrying the same reference does not
start another platform write. A new ordinary reply is required when no confirmed
successor reference exists.

## Reader deployment and evidence

Deploy compatible serving and rollback readers before enabling the gate. The
optional `discord_edit` record field is absent from historical cards. Native
successors with controls retain the existing locator and selection formats;
text-only successors require the new reader because older native renderers
require at least one component. Stopping issuance preserves existing callbacks,
claims, receipts and successor edits through the original expiry. Keep compatible
readers and encryption keys until those records expire.

The reviewed official contract is Discord API v10
[`PATCH /channels/{channel.id}/messages/{message.id}`](https://docs.discord.com/developers/resources/message#edit-message),
pinned to `discord/discord-api-docs` revision
`6128d99116af8357db1e4966ebd7f099817a0e04`,
`developers/resources/message.mdx` (SHA-256
`c296fe9263b887f0f3cf840927466826cd4e5c9182aebe6613decc28e5d80611`).
The original message author may edit content
and components; edits return a Message object, and `allowed_mentions` must be
supplied again because an edit does not inherit the original mention policy.
CMA uses the existing action-row component format and does not set
`IS_COMPONENTS_V2` or edit attachments.

Local REST, encrypted-store, callback and provider fixtures establish the CMA
contract. Actual Discord edit/render/callback acceptance requires a separately
authorized platform test. No NyxID change or deployment is part of this feature.
