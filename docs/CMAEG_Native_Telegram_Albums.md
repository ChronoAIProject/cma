# Native Telegram albums

With `CMAEG_NATIVE_TELEGRAM_ALBUMS_ENABLED=true`, a direct native Telegram reply
with two to four compatible `native_attachments` sends one ordered album using
`sendMediaGroup`. The existing request shape, attachment ordering and byte limits
are unchanged. The flag defaults to false. Single attachments retain their
existing upload, control, edit and effect paths.

Telegram permits photos and videos together, documents with documents, or audio
files with audio files. CMA uses those exact groups. Unsupported combinations,
such as a photo with a document, retain the existing separately authorized
uploads. A rejected or uncertain album is never retried as separate messages.
Stickers remain unsupported on native Telegram.

The optional `capabilities.native_media.albums` discovery object appears only for
a verified original native Telegram destination with the flag enabled. It reports
`min_attachments: 2`, `max_attachments: 4`, and
`kind_groups: [["image", "video"], ["file"], ["audio"]]`. It explicitly reports
`controls: false` and `message_operations: false`. Telegram's album method does
not accept `reply_markup`. Multiple attachments with controls remain refused;
existing one-upload controls continue to work.

All attachment bytes are downloaded/decoded and the entire multipart request is
encoded before any text or album is sent. Preparation uses the existing public
HTTPS address-pinning rules, four-attachment limit, 64 KiB aggregate inline limit
and 8 MiB aggregate materialized limit. Each attachment retains its caption;
top-level text remains a separately authorized preceding message.

One album request uses one route pacing slot and one current send-gate attempt.
The bot credential must still match the original sealed bot. The transport makes
one request. It only confirms delivery if the returned array has exactly the
expected number of messages, distinct positive message IDs, the original chat
and optional forum topic, the original sending bot, one common nonempty album ID,
and the expected ordered media kinds and captions. Missing, malformed, mismatched
or lost responses remain unknown. A successful preceding text message followed
by an uncertain or refused album remains partial delivery. Existing durable
reply intents retain their original idempotency/replay behavior.

No aggregate or individual operation reference is issued for the album. This
preserves the existing multi-attachment contract, which already returned no
media operation references. A delivery's first platform ID acknowledges delivery
and grants no edit, deletion or reaction authority. Single-upload references
retain their existing meaning; Telegram may redirect album reactions to another
member, so their authority must not be reused for albums. No scalar card format,
retained custody record, message-reference field or canonical digest changes.

Deploy compatible Gateway and SDK capability readers before enabling the flag.
Disabling it restores separate sends for new requests; completed intent replay
still returns the original receipt. The implementation does not enable a live
flag or send a native platform message.

## Official contract

The official [Bot API documentation](https://core.telegram.org/bots/api) was read
on 2026-10-29. The downloaded HTML is 860,230 bytes, SHA-256
`2f93d2e82fcaa215609744cc54f7a946a3d9e2326683f464162c812f667d36a8`.
The site does not provide an immutable versioned URL.

| Official section | Implemented contract |
| --- | --- |
| [sendMediaGroup](https://core.telegram.org/bots/api#sendmediagroup) | Two to ten items; photo/video mixing, audio-only or document-only; multipart media; array of sent Messages; optional forum topic; no inline keyboard parameter. CMA keeps its existing maximum of four. |
| [InputMediaPhoto](https://core.telegram.org/bots/api#inputmediaphoto), [InputMediaVideo](https://core.telegram.org/bots/api#inputmediavideo), [InputMediaAudio](https://core.telegram.org/bots/api#inputmediaaudio), [InputMediaDocument](https://core.telegram.org/bots/api#inputmediadocument) | `attach://` multipart parts and individual captions. CMA supplies prepared bytes, never a platform file locator or a URL for Telegram to fetch. |
| [Message](https://core.telegram.org/bots/api#message) | Positive ID, chat, sending bot, optional topic, media and `media_group_id` bind every confirmed member to the attempted album. |
| [setMessageReaction](https://core.telegram.org/bots/api#setmessagereaction) | Album reactions may target the first surviving member; existing single-message effect references are therefore not issued for albums. |

Local transport tests verify ordering, compatible groups, ordinary/topic sends,
all-member correlation, preparation before writes, current gate checks, disabled
behavior, incompatible-kind fallback, and no resend after ambiguous outcomes.
They do not establish live Telegram acceptance.
