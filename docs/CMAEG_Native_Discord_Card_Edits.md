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

Caller-supplied reply targets and new thread destinations are refused before an
edit claim or platform request. Upload replacement requires the separate
media-card replacement gate described below.
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
`IS_COMPONENTS_V2`. The separate upload replacement path supplies explicit final
attachment membership in the same PATCH.

Local REST, encrypted-store, callback and provider fixtures establish the CMA
contract. Actual Discord edit/render/callback acceptance requires a separately
authorized platform test. No NyxID change or deployment is part of this feature.

Card PATCH shares the durable exact-message mutation lock with native card and
text effects. A successor inherits adopted card-effect capability without
extending expiry and clears operation-local effect state. Unknown writes or lost
final acknowledgements keep the destination fenced across revisions. See
[native card effects](CMAEG_Native_Discord_Card_Effects.md) for reader rollout and
accepted callback recovery after deletion.

## Media-bearing control cards

The separate `CMAEG_NATIVE_DISCORD_MEDIA_CARD_EDITS_ENABLED` gate extends this
same destination lock, CAS and replay chain to cards issued with an exact
confirmed upload proof. `updateMessage` changes content/controls and preserves
the upload by omitting `attachments` from PATCH. Both the native pre-read and
write response must match retained attachment identity, metadata and presentation.
Uncertain writes do not resend or bind a new reference. Historical media cards
without proof remain callback-only. This gate alone does not allow binary/caption
replacement; the separate replacement gate is described below. Media/form
combinations remain unsupported; see [media-control edits](CMAEG_Native_Discord_Interactions.md#preserve-an-upload-while-editing-its-controls).

## Same-card media replacement

`CMAEG_NATIVE_DISCORD_MEDIA_CARD_REPLACEMENTS_ENABLED` defaults to `false` and
also requires the media-card-edit gate. When discovery reports
`native_media.controls.replace_media=true`, update an existing confirmed media
card using its current `message_ref` and a complete `message` with exactly one
`native_attachments` item, plus the complete replacement text and controls. An
empty attachment list preserves the existing file. Removing all files and multiple uploads require the separate attachment-set gate below. Forms with media remain unsupported. The replacement can
be an image, file, audio or video; existing source and byte limits still apply.

The operation keeps the original message, channel/thread, actor, consumer,
conversation and expiry. It verifies the old native message and fully prepares
the multipart request before admission, then saves a successor with compact
new-byte metadata and
`media.pending_replacement=true`. This state is readable but has no callback,
message-effect or edit authority. Source URLs and base64 are never stored on the
card. One multipart PATCH replaces the complete attachment membership and
controls together. Only an exact response for the new file and controls can
bind the successor; original reply, message flags and spoiler state must remain.
Generated previews and media dimensions may change with the new file.

Retries must retain the original `message_ref` and identical complete request.
The immutable request hash resolves exact replay before downloading an attachment
again. An accepted but uncertain PATCH stays fenced and never reuploads. A
successful result returns the successor `message_ref` for the next revision.
Turning the gate off stops fresh replacements but preserves exact replay,
callbacks and same-file edits using retained confirmed successors.

Deploy every retained reader with support for pending successors before enabling
replacement issuance. Older media-preserving readers cannot recover the new
pending state. These tests use local native HTTP fixtures; no Discord platform
messages are sent by the test suite.

## Complete attachment sets and explicit removal

`CMAEG_NATIVE_DISCORD_MEDIA_CARD_SETS_ENABLED` defaults to `false`. With the existing native-media/control issuance gates, it permits one card containing two to four image, file, audio or video uploads. With the existing media-card edit/replacement gates, it permits replacing a confirmed card with a complete set of one to four uploads, including count changes. The aggregate upload budget remains 8 MiB; there is no splitting or fallback into multiple messages. New-card issuance may be disabled while retained-card callbacks, preservation edits and otherwise enabled replacements continue.

The complete `native_attachments` array replaces all previous provider IDs. To remove every attachment, pass `clear_native_attachments: true` in the `updateMessage` message alongside the complete replacement text/controls. Clearing and nonempty replacement attachments are mutually exclusive. Omitted/false clear intent and omitted/empty `native_attachments` preserve the existing set. Clear intent is rejected on ordinary sends, other platforms and references without confirmed same-card media authority. Discovery advertises `native_media.controls.clear_media: true` only with the set/edit/replacement prerequisites and current verified custody. `controls.max_attachments` describes new-card issuance (one or four); `replace_media` and `clear_media` describe their distinct update operations. Disabling new-card issuance may omit that per-event discovery object without revoking retained operations.

Every source is materialized and the entire bounded multipart body is serialized before the original card is superseded. Discord has no separate upload operation in this path: all new files, exact final membership, content, controls and mention policy travel in one PATCH. The old callback remains usable during preparation. CMA refreshes current source/consumer/provider custody around native pre-reads and before PATCH, and revalidates bot identity after preparation. The existing exact-message fence and card CAS serialize CMA writers. There is no provider transaction, rollback or conditional revision guarantee, and another application holding the same bot credentials remains outside CMA's fence.

The response must prove the complete ordered membership, distinct new IDs, each filename, MIME/size/description and spoiler state, exact content/controls and the original message/channel/bot/reply/flags. Existing positions retain their spoiler status; newly added positions use an explicit `SPOILER_` filename to request a spoiler. Generated previews and dimensions may change with new files. A clear succeeds only with an explicit empty `attachments` response. Its successor has no media authority and supports ordinary edits and callbacks. Unknown writes, wrong/missing/extra members or persistence uncertainty never activate a replacement reference or repeat PATCH.

Historical one-file records keep the exact `media` field; only two-to-four-file records use the optional ordered `media_set` field. All serving and rollback readers must understand that field before enabling set issuance. A confirmed one-file replacement returns to the scalar shape, and a clear removes both fields and strips the request-only clear flag from the stored successor. Previously confirmed scalar records and references remain readable.

The official contracts were fetched on 29 October 2026: [Edit Message and attachment requests](https://raw.githubusercontent.com/discord/discord-api-docs/2b434c56eca46b61461bf5578499dd7810e2f6af/developers/resources/message.mdx), SHA-256 `2341b0f46a9b0afaf84918f097b38f2f0fa8aef7d519430b0946734d5e13b046`; [Uploading Files](https://raw.githubusercontent.com/discord/discord-api-docs/11df0a2b5cbc2e66fd1d891bc604157dcfd1c541/developers/reference.mdx), SHA-256 `b0ac645b5e85d1d05fbf07b7e78a25e4e22987560e05e6b0c8d238b6a1e0675d`. API v10 requires the complete final `attachments` list, removes omitted existing IDs, and matches new numeric placeholders to unique `files[n]` parts. Provider limits do not enlarge CMA's four-file/8-MiB bounds. Connected Discord acceptance remains separate; local tests send no native platform messages.
