# Native WhatsApp image, PDF, audio, sticker and video replies

Native WhatsApp event replies can upload JPEG/PNG images, PDF documents and the
bounded audio, WebP sticker and MP4/3GPP video profiles below under the independent `CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED` gate. It defaults to
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
| `kinds` | `image`, `file`, `audio`, `sticker`, `video` |
| `sources` | `url`, `base64` |
| `mime_types` | `image/jpeg`, `image/png`, `application/pdf`, `audio/aac`, `audio/amr`, `audio/mpeg`, `audio/mp4`, `audio/ogg`, `image/webp`, `video/mp4`, `video/3gpp` |
| `max_attachments` | 4 |
| `max_inline_bytes` | 65536 aggregate decoded inline bytes |
| `max_total_bytes` | 8388608 aggregate inline/downloaded bytes |
| `max_image_bytes` | 5000000 encoded bytes per image |
| `max_caption_chars` | 1024 for image/PDF/video; shared metadata also caps captions at 1024 UTF-8 bytes; audio/sticker captions are refused |
| `sticker` | Exact sticker dimensions, byte ceilings and animation limits described below |
| `video` | Exact video codec/profile and structural limits described below |

`image` requires JPEG or PNG, and `file` requires PDF. Supplied MIME types must
match the sniffed bytes. Supplied filename extensions must match the type
(`jpg`/`jpeg`, `png`, `pdf`, `webp`, `mp4`, `3gp`, or the audio extensions below); absent names get a
generated matching extension.
Existing filename single-component and control-character validation applies.
PNG headers must declare 8-bit RGB or RGBA. JPEG headers must declare 8-bit,
three-component baseline or progressive data. Images also retain the shared
18-million-pixel and 12000-pixel-side ceiling. These header checks do not decode
or rewrite pixels, inflate metadata or certify an entire file; Meta remains
authoritative for complete file validity and client acceptance.

Other document formats, templates, native controls mixed with
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
`api/crates/cmaeg/tests/fixtures/whatsapp-outbound`. Existing `tone.mp3`, `tone.amr` and
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

## Static and animated stickers

Use `kind: "sticker"` with `image/webp` and a `.webp` filename when those metadata
fields are supplied. WebP is refused as `kind: "image"`; the type requests sticker
presentation explicitly. Stickers share the existing default-off media gate,
source preparation, per-upload/per-send authorization, sealed phone/recipient/quote
and durable receipt rules. No new effects reference or caller-supplied Meta media
ID is introduced. Audio and sticker captions are refused before any part of the
batch is sent. Accompanying text stays a separate ordinary message.

Discovery adds the optional `native_media.sticker` object with these required
fields when native WhatsApp sticker delivery is available:

| Field | Value |
| --- | --- |
| `width`, `height` | 512, 512 pixels |
| `max_static_bytes` | 100000 |
| `max_animated_bytes` | 500000 |
| `max_frames` | 128 |
| `max_duration_ms` | 10000 per animation cycle |
| `min_frame_duration_ms` | 8 |

Byte ceilings use conservative decimal KB. The Cloud API documents the two byte
ceilings; [WhatsApp's own sticker guidance](https://github.com/WhatsApp/stickers/blob/main/Android/README.md)
documents the canvas and timing requirements. The 128-frame ceiling is a stricter
CMA parsing bound. Infinite looping is allowed; the duration bound applies to one
encoded cycle. Existing four-attachment, 64 KiB aggregate inline and 8 MiB total
reply caps still apply. A static file over 64 KiB or animated file over that inline
budget must use an eligible public HTTPS source.

The bounded parser checks exact RIFF size and zero padding; unique ordered WebP
chunks; VP8 keyframe or VP8L header dimensions/version; alpha header consistency;
and every animated frame's bounds, dimensions, flags, duration and frame count.
It accepts lossy and lossless static/animated data, including compressed or raw
alpha, without invoking the inbound image normalizer or any pixel decoder.
Extended files may contain declared ICCP before image data and declared EXIF/XMP
following it; these chunks remain opaque bounded bytes. Unknown chunks, duplicate
bitstreams, inconsistent feature flags, trailing data, malformed framing and
unsupported metadata order are refused. Structural eligibility does not prove
compressed-payload validity or actual transparency; Meta remains authoritative
for complete decoding and client acceptance.

The additive `sticker` media kind and `NativeStickerCapabilities` schema are
synchronized in OpenAPI and both SDKs. Legacy discovery omits `sticker`; unsupported
native transports, NyxID replies and initiated sends refuse the new kind before
any text/upload/write. Deploy compatible readers before enabling the gate.

Local fixture generation used libwebp 1.6.0 (`cwebp`, `img2webp`, `webpmux`) solely
as developer tools. No binary/dependency/image/resource change is required in
production. The source PNGs were synthetic 512×512 transparent RGBA canvases: for
frame `i` in 1,2, pixels with `80+i*30 < x < 300+i*30` and `100 < y < 400` have RGBA
`(30+i*80,100,220,255)`; other pixels are transparent black. Commands were:

```sh
cwebp -quiet -lossless frame-1.png -o whatsapp-static.webp
cwebp -quiet -q 80 frame-1.png -o whatsapp-static-lossy.webp
img2webp -loop 0 -d 100 -lossless frame-1.png -d 100 frame-2.png -o whatsapp-animated.webp
```

`webpmux -info` independently reported a 512×512 canvas for all three, transparent
static lossless/lossy data and two lossless 100 ms frames for the animated fixture.
Fixtures reside in `api/crates/cmaeg/tests/fixtures/whatsapp-outbound`:

| Fixture | Bytes | SHA-256 |
| --- | --- | --- |
| `whatsapp-static.webp` | 66 | `f3d9421332d7af880a1dc40c22ea4a3d43b7f23eb206a46fbb4ecf49f27fcb69` |
| `whatsapp-static-lossy.webp` | 860 | `7acb1691d0e5ac396bc2c0979cdb5060d31c31485f951992645f4dc7ed706199` |
| `whatsapp-animated.webp` | 184 | `b008ac1e52a1652160fe5a4e6bbb9c17dc4932a2ee819ad5d0bd95b3c818a8da` |

Tests cover both presentations, exact multipart bytes and ID-only message shape,
sealed targets and stored receipts, no placeholder for sticker-only sends,
whole-batch rejection before text/upload, exact byte limits, frame/count/timing
boundaries, truncation/adversarial headers, unsupported transports, unknown
upload/send and source revocation between upload and send. Connected v25 Meta
acceptance remains pending.

## Native video profile

`kind: "video"` accepts `video/mp4` with `.mp4` and `video/3gpp` with `.3gp`.
The major brand establishes the MIME type: MP4 accepts `isom`, `iso2`, `mp41`,
`mp42` or `avc1`; 3GPP accepts `3gp4` through `3gp9`. Supplied MIME/extension
must match. Missing metadata is derived from the validated container. Video
messages carry the returned upload ID and optional caption, with no filename or
hosted-media link. The existing caption and whole-reply byte limits apply; the
8 MiB aggregate ceiling is stricter than Meta's 16 MB per-video limit.

Discovery adds optional `native_media.video` (`NativeVideoCapabilities`):

| Field | Value |
| --- | --- |
| `video_codec`, `video_profile` | `h264`, `baseline` |
| `audio_codec`, `max_audio_tracks` | `aac_lc`, 1; a silent video is allowed |
| `max_width`, `max_height` | 1920, 1080 display pixels |
| `max_frame_rate` | 60 frames per second |
| `max_duration_ms` | 600000 per track/movie |
| `max_samples` | 65536 total across video and audio |
| `progressive_only` | true |
| `max_slices_per_frame` | 1 |
| `fast_start_required` | true (`moov` before `mdat`) |

The accepted container is self-contained, unencrypted and nonfragmented, with one
`avc1` video track, zero or one AAC-LC `mp4a` track and one local media-data box.
Playback rate and pixel aspect ratio must be one. No other tracks, external
references, alternate descriptions or track-matrix transformations
are admitted. Sample sizes, chunk mappings and every sample span must fit local
media data; spans cannot overlap within or between tracks. Decode-time counts and
durations, optional zero composition offsets, sync-sample indices and ordinary
rate-one edit lists must agree. AAC uses the existing matched sample-entry and
AudioSpecificConfig checks. Only the demonstrated AAC priming `roll` sample group
is accepted. Unsupported boxes are refused in structural containers; bounded
movie user metadata, video color/bitrate metadata and SEI payloads remain
opaque and do not confer extra decoding support.

AVC configuration requires one SPS and one PPS, at most 1024 bytes each, with
matching Baseline profile/dimensions and bounded Exp-Golomb fields. Progressive
8-bit 4:2:0 with square pixels uses CAVLC, POC type 0 or 2, no frame-number gaps, no slice groups,
weighted prediction, redundant pictures, HRD or scaling/transform extensions.
Every length-prefixed NAL span and emulation-prevention sequence is checked.
Each video sample has one complete admitted I/P slice header; the first sample
must be IDR and sync-table entries must match IDR samples. B/SP/SI slices, in-band
parameter replacement, partition/extension NALs, reference-list modifications and
adaptive reference marking are refused. Optional AUD and bounded framed SEI can
precede the slice. There are at most 256 boxes per structural container and 256
NALs per sample. This is intentionally narrower than all valid Baseline streams.

These checks establish the supported structure and header profile. They do not
entropy-decode slices or AAC payloads, reconstruct pixels, prove reference-frame
correctness or certify full H.264 level conformance. Meta remains authoritative
for complete compressed-payload validity and client acceptance. No new production
binary, subprocess, dependency, deployment or resource configuration is introduced.
The independent outbound-media gate remains default-off.

The official [Meta video guidance](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/video-messages),
read on 2026-10-28, permits H.264 with zero or one AAC stream and warns about
High-profile/B-frame Android acceptance. This implementation chooses the bounded
Baseline subset above and rejects High/Main/HEVC and other codecs. Graph remains
pinned to v25.0; current documentation examples use v26.0 and are not connected
v25 acceptance evidence.

### Local video evidence

Video fixtures reside in `api/crates/cmaeg/tests/fixtures/whatsapp-outbound`,
separate from the transform self-test inventory. FFmpeg/ffprobe 8.1.1 was used
only as a developer fixture tool. Both synthetic
fixtures were independently identified as 64×48 Constrained Baseline, progressive,
10 fps, 0.4 seconds and zero B-frames. The 3GPP fixture additionally has one
44.1 kHz mono AAC-LC track. `trace_headers` independently exposed the SPS/PPS and
slice syntax used by targeted negative mutations.

```sh
ffmpeg -v error -f lavfi -i 'testsrc2=size=64x48:rate=10:duration=0.4' -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -movflags +faststart -an whatsapp-baseline.mp4
ffmpeg -v error -f lavfi -i 'testsrc2=size=64x48:rate=10:duration=0.4' -f lavfi -i 'sine=frequency=440:duration=0.4' -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -c:a aac -b:a 32k -ar 44100 -ac 1 -movflags +faststart -f 3gp whatsapp-baseline-aac.3gp
```

| Fixture | Bytes | SHA-256 |
| --- | --- | --- |
| `whatsapp-baseline.mp4` | 2637 | `4086e78f34a2f8ecc5c902a5439df79ca1a561201347247bb16f349e7953cdac` |
| `whatsapp-baseline-aac.3gp` | 4883 | `72c0eb191b0835d5c28402e8253dd3c2dce9af6f96ca9425986cae39eb6ba435` |

Tests include both real files, unlabeled MIME/extension derivation, media-only and
mixed text/image/audio/video delivery, captions, sealed targets, per-write gates
and stored receipts. Rejections include every truncated prefix, every sample's
length/header corruption, overlapping ranges, external references, multiple audio
tracks, fragment/encryption markers, timing/edit/sync inconsistencies, unsupported
SPS/PPS modes, real High/B-frame, HEVC and VP9 files, and a B slice disguised by
unchanged Baseline configuration. Whole-batch preflight, unknown upload/send,
receipt loss and revocation stop later effects. Existing audio tests also cover
the shared BMFF extraction. No live Meta send/client acceptance is claimed.

## Upload, send and revocation

The sender uploads each bounded attachment using authenticated multipart
`POST /v25.0/{phone_number_id}/media`, with `messaging_product=whatsapp`, MIME
`type`, and the file bytes. A successful upload must return one valid numeric
media ID. Only that upload result can construct the subsequent `image`,
`document`, `audio`, `sticker` or `video` message; callers cannot supply a platform media ID or
hosted-media link to bypass validation. The media result remains bound to its upload phone.

`POST /v25.0/{phone_number_id}/messages` uses the sealed original private recipient
and context message ID, with the uploaded media ID and original caption. Document
messages also include their validated filename. Audio messages contain only the
uploaded `audio.id`, with no caption, filename, link or voice flag. Sticker
messages likewise contain only `sticker.id`. Each upload
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
- [Meta Sticker messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/sticker-messages): static/animated WebP, 100 KB/500 KB ceilings and ID-only sticker messages.
- [Meta Video messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/video-messages): MP4/3GPP, H.264 with zero or one AAC stream, captions, provider 16 MB ceiling and client profile guidance.
- [Meta Audio messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/audio-messages): basic uploaded-ID audio messages, the five supported audio MIME types and 16 MB provider ceiling; Ogg requires mono Opus and voice presentation is a separate flag.
- [Meta Document messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/document-messages): document ID, filename and caption fields, supported PDF type and provider 100 MB ceiling.

The current examples use Graph v26.0. CMA deliberately retains its pinned v25.0
endpoint; these documentation checks and local v25-shaped transport tests do not
prove connected v25 acceptance. CMA uses a conservative decimal 5,000,000-byte
image ceiling and a stricter 8 MiB total reply ceiling than Meta's 16 MB audio/video
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
- Video beyond the bounded profile above needs additional demonstrated codec,
  parameter-set, sample and slice handling; general MP4 acceptance is not implied.
- Other documents need exact UTF-8/plain-text and Office OLE/OOXML type checks,
  including bounded archive inspection where required, matching filename/MIME
  rules and fixtures. Merely labeling arbitrary bytes with an Office MIME type
  does not establish that support.

Each extension needs exact capability claims, SDK/OpenAPI synchronization where
its public shape changes, negative and uncertainty tests, and separately
an authorized connected Meta acceptance test.
