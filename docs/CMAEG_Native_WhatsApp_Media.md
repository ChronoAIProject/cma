# Native WhatsApp inbound media

Native Meta callbacks normalize private image, document, audio, voice, video and
sticker messages into the existing source-bound `Activity` attachment contract.
This is independent of the NyxID WhatsApp relay. Native text, reactions and their
reply destinations retain their existing behavior. A reaction can target a
retained incoming media message as well as a text message.

## Admission and rollout

`CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED` defaults to `false` in code and the CMAEG
ConfigMap. First media admission requires that flag and `CMAEG_MEDIA_ENABLED`.
Deploy compatible readers of `AttachmentSource::Whatsapp` and retained native
`Content::Media` everywhere before enabling issuance. Turning the native media
flag off blocks new media events but keeps already-admitted work readable and
rebatched retries idempotent. Keep the shared media service enabled to execute
retained media work. Rolling back to readers that cannot decode these variants
after admission is unsafe.

The original raw webhook must pass Meta HMAC authentication and exact configured
WABA and receiving phone checks before media facts are extracted. The app identity
comes from the configured source, whose credentials were independently verified
at authoring. Each message keeps its exact native message ID, sender and timestamp.
Admission commits the original signed envelope and routed plan before callback
acknowledgement. A duplicate ID with changed normalized media facts conflicts;
an unchanged rebatch reuses the first retained envelope and targets.

The locator stores only native message ID, media ID and canonical lowercase
SHA256. An expiring webhook download URL is ignored. Image, video and document
captions become the message text; document filenames and MIME types use the shared
bounded fact normalizer. Audio's `voice` flag selects the voice variant. Stickers
retain the sticker variant; their optional `animated` flag is validated, while
actual preview support follows the shared decoder. Dimensions, duration, file
size, sender display names and human identity are not inferred when absent.

## Retrieval authority and limits

Every source fetch requires the sealed Activity digest and exact configured
app/WABA/phone, channel record, native message, actor and private conversation.
The requested locator must be present in that Activity. Current source removal
or credential removal refuses retrieval; historical webhook possession does not
grant permanent access.

Each read resolves fresh metadata with the current Meta access token:

```text
GET https://graph.facebook.com/v25.0/<media-id>?phone_number_id=<receiving-phone-id>
Authorization: Bearer <current native Meta access token>
```

Meta documents the phone parameter as an ownership check. CMA requires matching
media ID, WhatsApp product marker, signed SHA256 and normalized MIME. Metadata
is capped at 32 KiB; the platform ceiling is 100,000,000 bytes and every caller's
smaller media cap still applies. Both network stages share a deadline of at most
30 seconds and use public DNS answers pinned to the connection, without proxy
or redirects. Credentials go only to the exact Graph origin and to HTTPS
`lookaside.fbsbx.com/whatsapp_business/attachments/` with one matching `mid`.
Provider URLs, credentials and response bodies never enter typed error output.

Metadata URLs expire after five minutes. A download 404 causes one metadata
refresh and retry within the same deadline; there is no unbounded retry loop.
401/403, 404, 429, 503, other 5xx and deadlines retain distinct shared fetch
classifications. A later authorized read resolves fresh metadata again. Meta
documents inbound media availability for seven days; CMA cannot extend it.

Complete reads verify actual size and SHA256. Streaming downloads check size
and digest at EOF and terminate with an error on mismatch; streamed bytes can
already have been delivered before final integrity verification. Text excerpts
remain limited to their caller's prefix budget: the signed and authenticated
metadata digest must match, but a truncated prefix does not claim verification
of the full byte digest. The shared MediaService owns concurrency, memory,
download, transformation, cache and per-channel rate budgets.

## Processing and validation scope

Existing attachment preparation, downloads, text excerpts, image processing,
configured speech transcription and configured video transformations consume
the new source. No separate WhatsApp decoder or unrestricted codec path is
introduced. A sticker or other format unsupported by the shared preview decoder
can still be downloaded; a usable preview is not promised. Durable routed
admission and target recovery are distinct from bounded in-process media jobs.
Failed media processing retains an explicit unavailable status and permits a
later authorized tool retry; it does not poison the native message identity.

Local regression fixtures cover source facts, signed identity conflicts, replay
after issuance is disabled, reactions on media, exact retrieval authority,
metadata mismatch, official URL policy, revocation, 404 refresh, typed upstream
failures, caps, full-body integrity and streaming integrity. These fixtures do
not establish acceptance under a connected production Meta app or client.

## Upstream references

Contracts were checked against Meta's published documentation on 27 October 2026:

- [Media metadata and download](https://developers.facebook.com/documentation/business-messaging/whatsapp/business-phone-numbers/media)
- [Messages webhook](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/)
- [Image](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/image/),
  [document](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/document/),
  [audio](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/audio/),
  [video](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/video/),
  [sticker](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/sticker/).
