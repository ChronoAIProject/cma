# Native Discord buttons and choice controls

Native Discord can render neutral `buttons` blocks as Discord buttons and
`choice` blocks as Discord string-select menus. Controls are
issued only when `CMAEG_DISCORD_INTERACTIONS_ENABLED=true`; the flag defaults to
false. Disabled issuance preserves the existing numbered-text rendering. Named
consumers also require `CMAEG_CONSUMER_INTERACTIONS_ENABLED=true`. Native
`multi: true` choices additionally require `CMAEG_DISCORD_MULTISELECT_ENABLED=true`,
which defaults to false. While native controls are enabled, requesting a
multi-select with that additional flag off returns `unsupported` before any card,
provider-context read, channel read or message send.

Deploy all serving and rollback readers before enabling issuance. New cards add
an optional native Discord identity, callback inbox bodies add a component
snapshot, and durable callback execution uses
`native_discord_interaction_payload_v1`. Historical card, inbox and ordinary
routing payload serialization is unchanged. Older readers refuse the new stored
callback contracts. Disabling issuance does not disable retained callback reads
or recovery.

Multi-select adds an optional `selection` array to the encrypted consumed-click
record. Deploy serving and rollback readers that understand this field before
enabling `CMAEG_DISCORD_MULTISELECT_ENABLED`. The field is omitted on historical
clicks, buttons and single-selects, preserving their stored bytes. A multi-select
click always retains the full canonical set, including `[]` for an explicit
clear. Readers validate this snapshot independently of the issuance flag.

## Supported contexts and limits

Controls require a verified human native Discord event, a ready source, durable
routing, and an executable provider binding. A provider's event-context response
must match the sealed event digest before a card can be prepared. Published agent
events remain ineligible. Ordinary DM/server channels and existing verified
threads support controls. Before creating a card, delivery verifies the exact
channel through one authenticated Discord channel read with a two-second budget.
Ordinary channels must match the verified conversation kind. A thread additionally
requires the original Activity and sealed reply address to identify that exact
existing thread, plus current authenticated type 10, 11 or 12, canonical guild
and non-self parent IDs. Missing or unavailable metadata refuses issuance. A
thread discovered only during delivery cannot upgrade an event without retained
thread proof; the original metadata gate must have admitted that proof.

Thread cards retain their authenticated channel ID, type, guild and parent as an
optional native identity field. Deploy compatible serving and rollback readers
before issuing them. Existing ordinary card serialization is unchanged. Both
ordinary and explicit `thread_reply=true` control messages go to the same thread
channel, and the parent is never an outbound destination. Disabling new thread
metadata intake does not revoke proof already retained by events or cards.

There are at most five action rows, with at most five buttons per row or one
string-select menu per row. Neutral message validation currently bounds a select
to twenty options, below Discord's twenty-five-option limit. Single-select menus
require one value. Multi-select menus allow zero through all authored options,
and reflect authored `selected` defaults. Modals, user/role/channel selectors and
multiple native attachments combined with controls remain unsupported. One native upload can accompany buttons or choices only under the separate media-controls gate below. Interactive
message edits have a separate default-off gate; see
[native card edits](CMAEG_Native_Discord_Card_Edits.md). A control message must fit
one ordinary Discord text part. Non-control blocks retain their existing text
rendering and degradation report. Mentions remain suppressed.

Discord `custom_id` fields contain only a versioned card UUID, revision and
action index. Select option values contain only opaque indices. Provider option
IDs and values are resolved from the encrypted card after authorization; client
labels and values cannot supply business action authority.

A multi-select callback submits the whole selection once. It does not toggle
saved options individually. The resulting `InteractionResult` uses the authored
choice block ID for both `block_id` and `action_id`, and `values.selected`
contains the selected business option IDs in their original card order. An empty
selection yields `values.selected: []`. Button and single-select result shapes
remain unchanged. These values use the existing public message and Activity
contracts; no SDK or OpenAPI shape change is required.

## Callback identity and acknowledgement

This implementation receives Gateway `INTERACTION_CREATE` events with
interaction `type: 3`. The Discord application's HTTP interactions endpoint must
not be configured to divert callbacks away from the Gateway. This adds no NyxID
callback endpoint and requires no relay changes.

A callback must match all retained identities before it can consume a card:

- The authenticated native source, application/bot ID and current source revision.
- The original human actor, conversation, exact existing thread when present, and delivered bot message ID/author.
- The card revision, button or string-select component type, and exact option
  indices from that component's own block.
- The original consumer and provider binding, including the default consumer.
- The card's expiry, confirmed delivery reference and unconsumed state, or the
  exact already-consumed callback during recovery.

A fresh thread click also makes one authenticated `GET /channels/{thread_id}`
within the original acknowledgement budget. Its channel ID, type, guild and
parent must equal the retained card proof. The callback's guild and any supplied
channel facts must agree; a Gateway channel object alone cannot grant thread
authority. The read uses the existing 64 KiB cap, no redirects, conservative
rate-limit cooldown and no automatic retry. Failed or changed metadata cannot
consume a fresh card. An exact already-consumed click can finish a lost inbox
write using its retained proof after the deadline; final authorization still
requires that same callback, actor, option, source and consumer.

Multi-select input must contain an explicit array of unique canonical decimal
indices within its own choice block. Missing values, duplicate or noncanonical
indices, cross-block indices and client business IDs are refused. Input order
does not affect the saved set. If a card was consumed but its inbox write was
lost, recovery requires the same callback ID and the same complete selected set;
a changed set cannot replace the winner. Once an inbox exists, Gateway replay
restores its immutable original snapshot. Two competing callbacks can produce
only one winning selection and one acknowledgement claim.

Current source admission is applied to the callback's authenticated actor and
conversation. A button click supplies explicit addressing evidence. Credential
rotation or a changed source revokes the retained source revision; removing or
rebinding its consumer also refuses the click.

The Gateway records the consumed click and sanitized native inbox before any
provider execution. It then attempts a type-6 `DEFERRED_UPDATE_MESSAGE`
acknowledgement at the exact official
`/api/v10/interactions/{interaction_id}/{interaction_token}/callback` endpoint.
The request carries no Bot authorization header. The interaction token is removed
from retained inbox bytes and never appears in Activity or provider context.

The acknowledgement budget ends 2.5 seconds after the interaction snowflake's
creation time, leaving margin within Discord's three-second deadline. A new late
click cannot consume fresh authority. An exact click that was consumed before a
crash can finish its inbox write after the deadline. Storage or network delays
can prevent an acknowledgement; durable work remains recoverable. A create-only
acknowledgement claim prevents retries after an uncertain response or process
restart. No message content is sent through this acknowledgement endpoint.

## Durable execution and rollback

Native callback admission is a separate typed entry into the routed scheduler.
It restores one retained consumer, provider binding, conversation and partition;
it does not run default targets, addressing rules or fan-out again. The source
inbox and Activity receipt precede provider execution. Repeated Gateway dispatch
restores the same stored activity and admission, and it cannot fall back to the
legacy direct launcher. Ordinary HTTP routed selection remains unchanged.

No new callback is authorized by an unconfirmed outbound send. If Discord accepts
a control message but its receipt or card binding cannot be confirmed, delivery
is reported as unknown and its card cannot authorize a callback. No automatic
resend follows an uncertain message write or acknowledgement.

To pause new multi-select cards, turn off `CMAEG_DISCORD_MULTISELECT_ENABLED`.
To pause all new controls, turn off `CMAEG_DISCORD_INTERACTIONS_ENABLED`. In both
cases, retain compatible readers until existing cards and callback work have
expired or settled. Thread controls
support existing threads only: there is no thread creation, join, reopen, archive,
follow or listing operation. Bounded Discord history is available separately
through `readEventContext`; it does not grant thread lifecycle authority.
Local authenticated HTTP and routed provider fixtures cover buttons,
single-selects and multi-selects, all three thread types, metadata refusals,
canonical whole sets and explicit clears, exact retained replay, issuance rollback,
and replies to the same thread. Production activation
and live platform proof are separate from those local tests.

Discord API references: [channel types and channel reads](https://discord.com/developers/docs/resources/channel#get-channel),
[message components](https://discord.com/developers/docs/components/reference),
and [interaction receipt and acknowledgement](https://discord.com/developers/docs/interactions/receiving-and-responding).

Native card replacement uses the existing `updateMessage` operation and a
separate `CMAEG_DISCORD_CARD_EDITS_ENABLED` gate. Existing native card references
remain unchanged. See [native card edits](CMAEG_Native_Discord_Card_Edits.md) for
exact control removal, retained ownership and durable replay semantics.

## One upload with native controls

`CMAEG_NATIVE_DISCORD_MEDIA_CONTROLS_ENABLED` defaults to `false`. After all
serving and rollback readers understand the optional native identity `media`
receipt, enable it alongside `CMAEG_DISCORD_INTERACTIONS_ENABLED`; named consumers
also require their existing issuance gate. One image, file, audio or video upload
can then share **one** Discord multipart message with buttons or string-select
choices. It retains one durable card and the exact same delivered message ID.
A multi-select additionally keeps its existing multi-select gate. More than one
upload, forms mixed with media, or a text body needing multiple Discord parts
returns `unsupported`; no separate control message is silently sent.

`context.capabilities.native_media.controls`, when present for the verified
human/source/consumer destination, reports `max_attachments: 1`, `buttons: true`,
`choices: true` and `caption_max_chars: 1024`. `update` is false unless the separate media-card edit gate and ordinary card-edit gate are enabled. Shared attachment
metadata byte limits, inline 64 KiB, downloaded aggregate 8 MiB, URL protection,
text limits and mention suppression still apply. Discord attachment captions
are attachment descriptions; the ordinary text remains message content.

The upload is materialized within the existing bounded fetch policy before the
delivery commit and card creation. Its encrypted card retains only SHA256 of the
actual uploaded bytes, length, kind, filename, MIME and caption. It does not
retain source URLs or base64 file bytes, so ordinary uploads cannot overflow the
64 KiB interaction-record limit. Callback recovery never downloads the media.
The same actor/source/consumer/thread/message authority checks govern clicks.
Unknown or lost send receipts leave the card unbound and do not cause a resend.

Media-bearing cards without a retained confirmed upload proof refuse text/card
`updateMessage` before mutation; no standalone media-edit authority is issued. Disabling the media-controls flag stops new
issuance but leaves retained clicks and recovery available. An older reader that
rejects the native identity's `media` field cannot serve as a rollback after
issuance. Local multipart, callback replay, authority and uncertain-send fixtures
prove this contract; external Discord activation and live proof are separate.

## Preserve an upload while editing its controls

`CMAEG_NATIVE_DISCORD_MEDIA_CARD_EDITS_ENABLED` defaults to `false` and also
requires `CMAEG_DISCORD_CARD_EDITS_ENABLED`. Deploy all retained readers before
enabling it. New media-control sends then verify the native message/control echo
and one uploaded file's exact filename, MIME, size and caption before binding the
card. Its compact `media.confirmed` proof contains the native attachment ID and a
hash of bounded normalized file/presentation facts, never a source URL or bytes.
Historical cards without that proof remain callback-readable and do not acquire
edit authority from a later lookup.

`updateMessage` can revise content and buttons/choices, including removing all
controls, while preserving that one upload. It uses the existing destination
lock, card CAS and successor/replay chain. A fresh native read verifies the current
text, controls and exact upload before the card revision changes. The one PATCH
omits `attachments` and rich-embed fields; the response must confirm unchanged
file identity/metadata/presentation plus the new content and controls. A mismatch
before admission refuses the edit; an ambiguous write or mismatched write echo
keeps the revision fenced and never blindly resends. Retained replay and subsequent
revisions remain readable when issuance gates are disabled.

For this contract `native_media.controls.update: true` means content/control
edits that preserve the upload. Upload replacement additionally requires
`native_media.controls.replace_media: true` and the separate replacement gate;
see [same-card media replacement](CMAEG_Native_Discord_Card_Edits.md#same-card-media-replacement).
Caption-only updates, adding uploads to text-only cards and forms mixed with
uploads remain unavailable. The independent default-off
`CMAEG_NATIVE_DISCORD_MEDIA_CARD_SETS_ENABLED` gate extends native cards to complete
sets of 1–4 uploads and explicit `clear_native_attachments: true` on supported
updates. Empty/omitted attachment lists without that flag still preserve files.
See [complete attachment sets](CMAEG_Native_Discord_Card_Edits.md#complete-attachment-sets-and-explicit-removal)
for replacement membership, prerequisites, reader compatibility and provider limits. Current source/consumer custody,
original actor, exact destination/thread and expiry checks still apply.
