# Native split-text receipts

`CMAEG_NATIVE_SPLIT_TEXT_REFS_ENABLED` defaults to false, including the deployment
manifest. With issuance enabled, ordinary native Telegram and Discord replies
that render into more than one text part can return `IntentReceipt.message_parts`.
Each entry contains a zero-based rendered `index` and an opaque `message_ref` for
that exact confirmed destination. The array is omitted when empty. Existing
single `message_ref` behavior is preserved: Telegram retains its first-part
reference, while Discord's multipart reply has no singular reference.

This increment covers ordinary text and text blocks, plus Discord links rendered
as text. Telegram link controls keep their existing single-part path. Existing
native edit and effect gates still determine the authority carried by each reference.
Discord text reference issuance requires its existing edit gate. Telegram may
add its existing scoped effect authority to each confirmed part when the effect
gate and original admission proof permit it. Controls, degraded cards, media,
albums, multiple attachments, and relay messages do not acquire per-part authority.

Delivery remains sequential. A confirmed earlier part retains its reference when
a later part fails, times out, or is refused. The aggregate result remains
`unknown` with `partial_delivery`; confirmed parts do not prove completion of the
whole reply. A part with no confirmed platform receipt receives no reference.
Discord also requires its reaction-target custody confirmation before issuing a
part reference. Telegram verifies the exact returned chat, topic, and positive
message ID, and refuses album-shaped receipts. Never resend an unknown delivery,
change its idempotency key to retry it, or infer the ID of an unconfirmed part.

For example, a three-part reply whose third request is unconfirmed can return:

```json
{
  "delivery": {"outcome": "unknown", "reason": "partial_delivery"},
  "message_parts": [
    {"index": 0, "message_ref": "<opaque first confirmed reference>"},
    {"index": 1, "message_ref": "<opaque second confirmed reference>"}
  ],
  "rendering": {"outcome": "degraded", "blocks": []}
}
```

Retained receipts exceeding the existing 16 KiB inline budget store their exact
reference tokens in bounded encrypted pages in the existing interaction-record
store. Every page is confirmed before its receipt pointer is stored. The pointer
binds the owner, operation scope, original retention deadline, part count, and
complete receipt digest; replay verifies all pages before restoring the receipt.
Missing, changed, incomplete, or unreadable pages fail closed. No new collection,
grant, platform request, or capability renewal is involved. Replays retain the
original opaque tokens and their original authority expiry. Existing send limits
are unchanged; expanded receipt size does not reject an otherwise valid message.

A partial receipt remains an unresolved delivery checkpoint, retaining its known
parts without advancing the automatic cursor or allowing a resend. The existing
operation/fence retention rules remain in force. A worker loss or outer deadline
before the final receipt is persisted still yields an unknown outcome; this
increment does not infer or reconstruct unrecorded platform acknowledgements.

Deploy every gateway reader and a compatible rollback image before enabling the
writer gate. Old strict gateway readers cannot read the new receipt field or
paged retained form. Gate-off replay must remain compatible with already issued
receipts. The TypeScript and Rust SDKs accept an omitted field and preserve all
new parts, including partial receipts. Older SDKs can discard additive fields on
reserialization and should be upgraded before callers depend on the references.
The CMA API does not deserialize `IntentReceipt`; the runtime tool proxy forwards
bounded response bytes. Its 128 KiB request and 4 MiB response bounds accommodate
the expanded public tool result, including the MCP structured/text envelope.
