# Confirmed native Telegram and Discord media edits

This additive `/v1` contract is limited to one confirmed native attachment in one whole platform message. New references require durable upload receipt storage and explicit `native_media_edit` authority. Historical effects-only references remain ineligible for editing, including after issuance is enabled. Multipart per-message references, Telegram albums and Discord messages with multiple attachments remain separate pending work.

`CMAEG_NATIVE_TELEGRAM_MEDIA_EDITS_ENABLED` and `CMAEG_NATIVE_DISCORD_MEDIA_EDITS_ENABLED` default to false and remain false in the manifest. Deploy compatible readers before enabling issuance. Turning issuance off does not revoke a valid retained reference. Provider acceptance is a separate deployment prerequisite; source and fixture validation alone do not establish connected acceptance.

Use the existing REST `POST /v1/messages/{message_ref}/update` or MCP `updateMessage` with an exclusive `native_media_update` object. Supply `Idempotency-Key` for REST or `idempotency_key` for MCP, using the existing visible-ASCII 1–128-byte bound. This key is required for media updates; existing text/card requests remain compatible without it.

```json
{"native_media_update":{"caption":"","text":"Updated visible Discord text"}}
```

Omitted members preserve their current value; an explicit empty string clears; null is rejected. `caption` is the visible Telegram caption and Discord attachment description (alt text). `text` is Discord visible message content and is refused for Telegram. Caption is bounded to 1,024 UTF-16 units and 4,096 UTF-8 bytes; Discord content to 2,000 UTF-16 units and 8,000 bytes. Ordinary send-caption validation remains unchanged.

```json
{"native_media_update":{"replacement":{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"report.bin","mime_type":"application/octet-stream"}}}
```

Replacement requires the original media kind: image, file, audio or video. The source uses the existing bounded public HTTPS/base64 materializer, with 64 KiB aggregate inline content and 8 MiB aggregate prepared bytes. URLs are fetched with existing address-pinning rules; arbitrary native file locators are not accepted. The patch cannot be combined with ordinary text, blocks, native attachments, templates, reply targets or thread flags. Send/reply and other-platform paths refuse it before platform writes.

Telegram uses `editMessageCaption` or multipart `editMessageMedia`, preserving the exact chat/message/topic; the edit API has no topic destination parameter, so the topic is checked in the returned Message. Discord uses one API v10 PATCH: content-only edits omit attachments, description edits retain the exact attachment ID, and replacement supplies only the new placeholder plus `files[0]` in the final attachment list. Content edits suppress mention parsing on every PATCH.

Each reference binds the original admitted Activity, selected default or named consumer, channel record, owner, provider binding and credential, complete native source and routing, exact destination, original media receipt, revision and fixed expiry of at most 24 hours. Same-bot token rotation is permitted only with fresh bot identity verification; mid-flight token changes refuse before writing. Provider credential, consumer-key, source or routing changes revoke the authority. New Telegram channel-post and anonymous-sender references are not issued by this first increment.

The operation claims its key durably and uses the existing exact-message mutation fence shared with delete/reaction. It freezes the prepared file digest and intended content before writing, then checks current custody again after slow reads and encoding. A confirmed edit persists current state by CAS and an exact next sealed reference before releasing the fence. Use that next reference for a new operation. Old revisions with new keys refuse; replay of the original key returns the saved receipt and next reference without refetching a source URL.

Timeouts, malformed success bodies, wrong response identities, post-write storage failures and other uncertain outcomes keep the destination fence. Changing a key, consumer, token or process cannot make an uncertain write safe to retry. No automatic reconciliation or resend is provided.

`capabilities.native_media_edits` advertises eligibility for new single-upload references. Per-reference `readMessageCapabilities` returns optional `media_edit` only for a valid current media revision. Neither field changes legacy `edit` semantics or grants delete/reaction permission.

Telegram provides no generic Bot API message-read endpoint. Discord performs an exact message/channel pre-read. Neither reviewed edit API offers a conditional revision or If-Match contract; local fences coordinate CMA replicas, not independent applications using the same credentials. Provider metadata and transformed media are not a server byte-digest proof. The official contracts are [Telegram Bot API](https://core.telegram.org/bots/api#editmessagemedia) and [Discord Edit Message](https://docs.discord.com/developers/resources/message#edit-message).
