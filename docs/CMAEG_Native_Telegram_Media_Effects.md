# Native Telegram upload message effects

An individually confirmed native Telegram media upload can return the existing
opaque `message_ref` for deletion and changes to the bot's own reaction. The
reference describes one exact sent message. It grants no text, caption or media
editing and does not represent a batch or an album. This is a bounded increment;
individual references for multipart sends and caption/media editing remain work
for a later public contract.

## Issuance and compatibility

`CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED` defaults to `false`, including in the
checked-in ConfigMap. It additionally requires
`CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED=true`, which retains the existing native
ingress admission proof and advertises effect capabilities. The media gate does
not change native upload availability or issue proof for unverified ingress.
No `Config` or `ToolsState` field, public API field or SDK type is added.

Every serving and retained rollback reader must understand the additive sealed
`telegram_effect.effects_only` field before media issuance is enabled. Historical
text/card authority omits the field when false, preserving its serialized form
and durable receipt hashes. Media authority sets it to true. Disabling either
gate prevents new media references; retained references remain usable until
their original 24-hour expiry or current authority revocation. Rollback must
retain compatible readers for retained references and receipts.

`updateMessage` refuses this scope before `editMessageText` or any interaction
replacement. It remains refused after the effect window expires. The marker
cannot be combined with interaction-card authority. Existing text/card reference
issuance, edits, expiry and receipt behavior remain unchanged.

## Exactly one native message

Telegram sends rendered text separately. Attachments normally send individually;
[opted-in albums](CMAEG_Native_Telegram_Albums.md) group compatible multiple uploads
without issuing effect authority. Single-upload issuance requires exactly one attachment, no separately rendered text
part, one attempted native upload operation, and its confirmed response.
Attachment captions remain on that single message and are supported at send
time. A top-level text message followed by one upload is two native messages and
receives no media reference. Two or more uploads also receive no aggregate
reference, even when the complete batch succeeds.

The existing bounded preparation, native source/token custody, sealed chat/topic,
route pacing and send gate are reused. The upload response must contain the exact
chat, exact optional forum topic and a positive message ID. A zero/scheduled ID,
wrong destination/topic, invalid response, unexpected `media_group_id`, response
loss or partial batch leaves no reference. No target is inferred from a file ID,
URL, filename, the first successful batch part or an uncertain platform write.

The adapter uses `sendPhoto`, `sendDocument`, `sendAudio` and `sendVideo`, each of
which returns its sent Message. This single-upload effect path does not use
`sendMediaGroup`; the separately gated album path issues no references. The strict
album refusal matters because Telegram may redirect a reaction on an album
member to the first non-deleted message in that group. This reference does not
authorize that possible alternative target.

Delete targets the entire exact sent message; there is no standalone attachment
deletion API in this increment. `setMessageReaction` changes the bot's chosen
reaction, accepts zero or one supported reaction, and preserves other users'
reactions. Empty input clears the bot's reaction. Paid reactions are unavailable;
custom emoji must already be present or be allowed by chat administrators.

Telegram decides whether the action is currently permitted. Deletion normally
requires a message younger than 48 hours, with additional service-message,
dice-message and channel/group permission limits. CMA's authority lasts at most
24 hours from the native send attempt, but this is not a guarantee of successful
deletion or reaction. A message edited or deleted through another client is
still identified by its original message ID; no replacement target is searched
for. Caption/media edits are separate Telegram methods and remain unsupported
through this reference.

## Current authority and durable uncertainty

The existing Telegram effect checks bind the current CMA record, owner,
consumer/provider/key, routing configuration, source bot, token prefix and native
actor/chat/group admission proof. They run before the durable claim and again
after mutation-fence acquisition, before the platform request. Named-consumer
authority cannot be borrowed by the default consumer or survive removal.

The existing effect receipt still owns the idempotency key. New media scopes also
use the existing exact-message fence algorithm, extracted into shared storage
without changing Discord's persisted namespace or record IDs. Telegram media
uses its separate `telegram_media_mutation_v1` namespace and a destination key
containing bot, chat and message. Historical Telegram text/card effects retain
their previous receipt behavior.

Different keys cannot overlap one media message. A definitive persisted receipt
releases the fence. Unknown writes, abandoned claims and failed final receipt
writes remain fenced through expiry, so restart or another key cannot make an
uncertain write safe to repeat. Replays use the durable receipt. The transport
performs one attempt and confirms only the Bot API's `true` result for delete or
reaction; rejection, rate limit and uncertainty remain distinct.

## Official API evidence and validation

Read the official [Telegram Bot API](https://core.telegram.org/bots/api) on
2026-10-27. The downloaded HTML was 860,230 bytes with SHA-256
`5d68ffad8f7106d52b656551605602fbab59fcbe7b5155708918d40731e28862`.
The official site does not provide an immutable versioned URL for this page.

| Official section | Relevant contract |
|---|---|
| [sendPhoto](https://core.telegram.org/bots/api#sendphoto), [sendDocument](https://core.telegram.org/bots/api#senddocument), [sendAudio](https://core.telegram.org/bots/api#sendaudio), [sendVideo](https://core.telegram.org/bots/api#sendvideo) | Each individual send returns the sent Message on success; uploads use multipart form data. |
| [Message](https://core.telegram.org/bots/api#message) | Message ID, chat, optional topic and media-group identity describe the confirmed native message. |
| [deleteMessage](https://core.telegram.org/bots/api#deletemessage) | Exact chat/message ID, age and permission limits, `true` on success. |
| [setMessageReaction](https://core.telegram.org/bots/api#setmessagereaction) | Up to one bot reaction, no paid reactions, custom emoji limits, and album redirection to the first surviving member. |
| [editMessageCaption](https://core.telegram.org/bots/api#editmessagecaption), [editMessageMedia](https://core.telegram.org/bots/api#editmessagemedia) | Caption and media editing use separate methods and have additional constraints. This increment grants neither. |

Local fake transports cover individual uploads and captions, exact destination
correlation, gate/proof requirements, edit refusal, current authority revocation,
named consumers, expiry, restart/replay, concurrent keys, response loss and failed
receipt writes. They do not prove live Telegram acceptance. No external send,
live flag change, push or deployment is part of this change.
