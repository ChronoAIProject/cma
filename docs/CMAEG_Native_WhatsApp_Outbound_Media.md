# Native WhatsApp image and PDF replies

Native WhatsApp event replies can upload JPEG/PNG images and PDF documents under
the independent `CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED` gate. It defaults to
false in code and the checked-in ConfigMap. This gate does not depend on native
inbound media or NyxID relay-media flags. Local fake Graph and record-store tests
establish the composed implementation; no live Meta send or client acceptance
has been performed. Overall WhatsApp support remains incomplete.

## Supported request and limits

Use the existing `OutboundMessage.native_attachments` contract only after
`readEventContext` or `verifyEventContext` advertises `native_media`. For a native
WhatsApp event, the enabled capability reports:

| Field | Value |
| --- | --- |
| `kinds` | `image`, `file` |
| `sources` | `url`, `base64` |
| `mime_types` | `image/jpeg`, `image/png`, `application/pdf` |
| `max_attachments` | 4 |
| `max_inline_bytes` | 65536 aggregate decoded inline bytes |
| `max_total_bytes` | 8388608 aggregate inline/downloaded bytes |
| `max_image_bytes` | 5000000 encoded bytes per image |
| `max_caption_chars` | 1024; shared metadata also caps captions at 1024 UTF-8 bytes |

`image` requires JPEG or PNG, and `file` requires PDF. Supplied MIME types must
match the sniffed bytes. Supplied filename extensions must match the type
(`jpg`/`jpeg`, `png`, `pdf`); absent names get a generated matching extension.
Existing filename single-component and control-character validation applies.
PNG headers must declare 8-bit RGB or RGBA. JPEG headers must declare 8-bit,
three-component baseline or progressive data. Images also retain the shared
18-million-pixel and 12000-pixel-side ceiling. These header checks do not decode
or rewrite pixels, inflate metadata or certify an entire file; Meta remains
authoritative for complete file validity and client acceptance.

Audio, video, stickers, other document formats, templates, native controls mixed
with media, explicit reply overrides and thread replies are refused before any
platform write. Audio/video require further codec/container validation before
they can be advertised. Ordinary text and attachment captions remain separate.
A media-only reply generates no placeholder message. The existing 128 KiB tool
request ceiling still applies to the entire JSON request.

Public HTTPS sources are materialized through the existing bounded, pinned public
address transport. Redirects, local files, private-address fetches and arbitrary
destination authority are not added. Meta credentials never accompany a source
download or enter a multipart file body. All attachments are fetched and validated
under the existing shared 30-second preparation budget before any text or upload
is sent. Native delivery has a 60-second outer budget; each Graph exchange retains
its 8-second timeout, the non-routed tool wait is 65 seconds, and the routed send
window remains independently limited to 20 seconds. These limits can stop a valid
batch before it finishes.

## Upload, send and revocation

The sender uploads each bounded attachment using authenticated multipart
`POST /v25.0/{phone_number_id}/media`, with `messaging_product=whatsapp`, MIME
`type`, and the file bytes. A successful upload must return one valid numeric
media ID. Only that upload result can construct the subsequent `image` or
`document` message; callers cannot supply a platform media ID or hosted-media
link to bypass validation. The media result remains bound to its upload phone.

`POST /v25.0/{phone_number_id}/messages` uses the sealed original private recipient
and context message ID, with the uploaded media ID and original caption. Document
messages also include their validated filename. Each upload and message send
enters the existing per-attempt authorization gate. Native sources require routed
admission; its gate checks current source/provider authority and the channel
revision before each effect, so source removal or credential rotation between
upload and send blocks the next effect.

Message confirmation requires the WhatsApp product marker, exactly one matching
recipient and one usable `wamid`, then encrypted durable receipt storage. Each
rendered text part and media message records its own original source, consumer,
provider, conversation and partition. Confirmed media receipts therefore support
the existing incoming-reaction correlation; they grant no text-edit, outbound
reaction or arbitrary-message authority.

An unknown upload response never proceeds to a message send. Unknown send
acceptance, receipt-storage loss and a failure after earlier confirmed messages
stop the batch and remain unknown. There is no automatic retry, fallback, media
deletion or continuation with later attachments. A successful upload followed by
revocation can leave an unsent media asset at Meta; Meta documents 30-day uploaded
asset retention. Existing durable reply claims fence uncertain effects and retain
their original idempotency keys. Turning the issuance gate off does not delete
confirmed receipts.

## Provider reference and acceptance

Read-only official documentation checked on 2026-10-27:

- [Meta Media](https://developers.facebook.com/documentation/business-messaging/whatsapp/business-phone-numbers/media): multipart upload, bearer authentication, numeric upload ID, supported MIME types, image/color limits, media retention, and audio/video codec restrictions.
- [Meta Image messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/image-messages): uploaded ID messages, recipient confirmation and 1024-character captions.
- [Meta Document messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/document-messages): document ID, filename and caption fields, supported PDF type and provider 100 MB ceiling.

The current examples use Graph v26.0. CMA deliberately retains its pinned v25.0
endpoint; these documentation checks and local v25-shaped transport tests do not
prove connected v25 acceptance. CMA uses a conservative decimal 5,000,000-byte
image ceiling and a stricter 8 MiB total reply ceiling than Meta's document limit.
Free-form media replies remain subject to Meta's customer service window and
current bot permissions. An authorized connected test of each supported type,
captions, receipts and incoming reactions remains required before activation.

## Remaining media implementation

This increment does not complete native WhatsApp media. The next work can reuse
`media/outbound.rs` for bounded materialization and `media/sniff.rs` for MIME
recognition, while keeping the existing upload/send authorization and receipt
pipeline:

- Audio needs exact AAC/AMR/MP3/MP4/OGG container and codec validation; OGG must
  prove mono Opus. The existing isolated `media/transform/ffmpeg.rs` probe,
  `argv.rs` allowlists and `probe_json.rs` parser provide the execution boundary,
  but currently expose only `codec_type` and duration. They do not yet prove
  codec names, profiles, channels or stream cardinality.
- Video needs MP4/3GPP plus H.264 and at most one AAC audio stream, with profile
  and B-frame checks for the client compatibility restrictions in Meta's docs.
  Extend the bounded probe contract before advertising `video`; preserve the
  current aggregate reply cap unless resource budgets are independently changed.
- Stickers need an additive typed outbound kind, exact static/animated WebP
  validation and per-kind size/dimension/frame limits. The chunk walker in
  `media/image/webp.rs` is a reusable bounded parsing pattern, but its current
  purpose is avoiding unsafe lossless image decoding, not certifying a Meta
  sticker. It must not be used as evidence of sticker support as it stands.
- Other documents need exact UTF-8/plain-text and Office OLE/OOXML type checks,
  including bounded archive inspection where required, matching filename/MIME
  rules and fixtures. Merely labeling arbitrary bytes with an Office MIME type
  does not establish that support.

Each extension needs exact capability claims, SDK/OpenAPI synchronization where
its public shape changes, negative and uncertainty tests, and separately
an authorized connected Meta acceptance test.
