# Native WhatsApp image, PDF and audio replies

Native WhatsApp event replies can upload JPEG/PNG images, PDF documents and the
bounded audio profiles below under the independent `CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED` gate. It defaults to
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
| `kinds` | `image`, `file`, `audio` |
| `sources` | `url`, `base64` |
| `mime_types` | `image/jpeg`, `image/png`, `application/pdf`, `audio/aac`, `audio/amr`, `audio/mpeg`, `audio/mp4`, `audio/ogg` |
| `max_attachments` | 4 |
| `max_inline_bytes` | 65536 aggregate decoded inline bytes |
| `max_total_bytes` | 8388608 aggregate inline/downloaded bytes |
| `max_image_bytes` | 5000000 encoded bytes per image |
| `max_caption_chars` | 1024; shared metadata also caps captions at 1024 UTF-8 bytes |

`image` requires JPEG or PNG, and `file` requires PDF. Supplied MIME types must
match the sniffed bytes. Supplied filename extensions must match the type
(`jpg`/`jpeg`, `png`, `pdf`, or the audio extensions below); absent names get a
generated matching extension.
Existing filename single-component and control-character validation applies.
PNG headers must declare 8-bit RGB or RGBA. JPEG headers must declare 8-bit,
three-component baseline or progressive data. Images also retain the shared
18-million-pixel and 12000-pixel-side ceiling. These header checks do not decode
or rewrite pixels, inflate metadata or certify an entire file; Meta remains
authoritative for complete file validity and client acceptance.

Video, stickers, other document formats, templates, native controls mixed with
media, explicit reply overrides and thread replies are refused before any
platform write. Ordinary text and attachment captions remain separate.
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

## Audio format checks

`audio` accepts the following deliberately bounded profiles. The parser inspects
materialized bytes in memory and never runs a decoder or follows a container's
reference. No FFmpeg/ffprobe runtime dependency, image input or resource budget is
added by this increment. All existing 4-attachment, 64 KiB inline and 8 MiB total
reply limits apply; the 8 MiB total is stricter than Meta's 16 MB per-audio limit.

| MIME / filename | Accepted structural profile |
| --- | --- |
| `audio/aac` / `.aac` | AAC-LC ADTS frames with one indexed sample rate and channel configuration; optional bounded ID3 prefix |
| `audio/amr` / `.amr` | Single-channel AMR narrowband storage framing, with complete permitted frame types and at least one speech frame |
| `audio/mpeg` / `.mp3` | MPEG-1/2/2.5 Layer III frames, including variable bitrates; bounded leading ID3v2 and trailing ID3v1 |
| `audio/mp4` / `.m4a` | Non-fragmented M4A/ISO BMFF with exactly one unencrypted AAC-LC audio track, one local data reference, matching AAC/sample-entry configuration and complete sample/chunk byte ranges inside local media data |
| `audio/ogg` / `.ogg` | One complete mono Opus logical stream, mapping family 0, valid page CRCs/sequence and bounded Opus headers; no chaining or multiplexing |

M4A accepts ordinary AAC-LC AudioSpecificConfig with no core-coder dependency,
960-sample-frame option, SBR/PS or other extensions; the common explicit
“SBR absent” extension is accepted. Other sample entries, encryption, external
data references, fragmented media and video tracks are refused. Ogg channel
count is checked from `OpusHead`; changing the label or extension cannot make a
stereo stream eligible. AMR-WB uses the separate `audio/amr-wb` MIME type
([RFC 4867 §8.2](https://www.rfc-editor.org/rfc/rfc4867#section-8.2)) and is refused
because Meta documents `audio/amr`. WAV, FLAC, WebM, unsupported AAC profiles and
malformed or truncated framing are refused before the first text/upload in the whole batch.

These checks establish the declared container and codec profile, not complete
compressed-payload decodability. Meta can still reject a structurally eligible
file. Audio captions, including captions on an otherwise valid file, are refused
before any platform effect. Put accompanying text in `OutboundMessage.text`.
The native audio object carries only the upload result's ID; the existing
attachment contract exposes no `voice` flag, so this is basic audio delivery.

## Local audio evidence

The audio increment uses real tiny media fixtures in
`api/crates/cmaeg/tests/fixtures/media`. Existing `tone.mp3`, `tone.amr` and
`voice.ogg` are reused. On 2026-10-28 the installed Homebrew FFmpeg/ffprobe 8.1.1
independently identified them as MP3, AMR-NB mono and Opus mono, and identified the
new fixtures as AAC-LC mono. This host inspection is fixture evidence, not proof
of the separately pinned production FFmpeg binary, which this path does not use.

The two new fixtures are synthetic, 44.1 kHz mono 440 Hz tones, generated locally
without external media:

```sh
ffmpeg -v error -f lavfi -i 'sine=frequency=440:duration=0.12' -ar 44100 -ac 1 -c:a aac -b:a 32k -f adts whatsapp-tone.aac
ffmpeg -v error -f lavfi -i 'sine=frequency=440:duration=0.12' -ar 44100 -ac 1 -c:a aac -b:a 32k -movflags +faststart whatsapp-tone.m4a
```

| Fixture | SHA-256 |
| --- | --- |
| `whatsapp-tone.aac` (663 bytes) | `8778f01715ba8b12f05d0b305abdd85a85d630fe2b713c1b51c78f95d197af14` |
| `whatsapp-tone.m4a` (1449 bytes) | `6ced03ebeba88dacde11247594f3aecc9d5a50766a7ed85f66c75bd01128ff49` |
| `tone.mp3` | `42c72043da971fef9cf1cffbba48514590d9a93f9228b716cccbc00fdca5d8e2` |
| `tone.amr` | `7e8ae71067bf012e1ae9dd4214732dbcc8f9c71508600ef9bf5a074e81ae2e2c` |
| `voice.ogg` | `bcba1fda20ea872129af8da818bea822976cddc6aa53fc353393d95559998099` |

Local coverage includes each audio type alone and a mixed text/image/PDF/audio batch,
exact upload bytes and audio message fields, sealed recipient/quote and durable
receipts, whole-batch validation before any write, caption/type/extension and
stereo Opus refusals, malformed/truncated/oversize inputs, external M4A references,
unknown upload/send responses, partial delivery, receipt loss and revocation
between upload and send. Connected Meta acceptance remains a separate requirement.

## Upload, send and revocation

The sender uploads each bounded attachment using authenticated multipart
`POST /v25.0/{phone_number_id}/media`, with `messaging_product=whatsapp`, MIME
`type`, and the file bytes. A successful upload must return one valid numeric
media ID. Only that upload result can construct the subsequent `image`,
`document` or `audio` message; callers cannot supply a platform media ID or
hosted-media link to bypass validation. The media result remains bound to its upload phone.

`POST /v25.0/{phone_number_id}/messages` uses the sealed original private recipient
and context message ID, with the uploaded media ID and original caption. Document
messages also include their validated filename. Audio messages contain only the
uploaded `audio.id`, with no caption, filename, link or voice flag. Each upload
and message send enters the existing per-attempt authorization gate. Native sources require routed
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

Read-only official documentation checked on 2026-10-27 and 2026-10-28:

- [Meta Media](https://developers.facebook.com/documentation/business-messaging/whatsapp/business-phone-numbers/media): multipart upload, bearer authentication, numeric upload ID, supported MIME types, image/color limits, media retention, and audio/video codec restrictions.
- [Meta Image messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/image-messages): uploaded ID messages, recipient confirmation and 1024-character captions.
- [Meta Audio messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/audio-messages): basic uploaded-ID audio messages, the five supported audio MIME types and 16 MB provider ceiling; Ogg requires mono Opus and voice presentation is a separate flag.
- [Meta Document messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/document-messages): document ID, filename and caption fields, supported PDF type and provider 100 MB ceiling.

The current examples use Graph v26.0. CMA deliberately retains its pinned v25.0
endpoint; these documentation checks and local v25-shaped transport tests do not
prove connected v25 acceptance. CMA uses a conservative decimal 5,000,000-byte
image ceiling and a stricter 8 MiB total reply ceiling than Meta's 16 MB audio
and 100 MB document limits.
Free-form media replies remain subject to Meta's customer service window and
current bot permissions. An authorized connected test of each supported type,
captions, receipts and incoming reactions remains required before activation.

## Remaining media implementation

This increment does not complete native WhatsApp media. The next work can reuse
`media/outbound.rs` for bounded materialization and `media/sniff.rs` for MIME
recognition, while keeping the existing upload/send authorization and receipt
pipeline:

- Audio beyond the bounded profiles above needs additional demonstrated codec
  and container checks. Voice presentation needs a separate typed request and
  capability contract; accepting Ogg Opus does not currently request it.
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
