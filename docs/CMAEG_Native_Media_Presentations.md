# Explicit native voice, animation and sticker presentation

`CMAEG_NATIVE_MEDIA_PRESENTATIONS_ENABLED` independently enables new native
presentation sends. It defaults to false. Deploy every serving, retained-record
and rollback reader before enabling it: `voice` and `animation` are additive
`NativeMediaKind` values. Existing `audio`, `video`, source bytes, request hashes
and operation references retain their meanings. No NyxID change is involved.

Clients discover supported kinds and `presentation_profiles` through the existing
event context, then send them through `replyToEvent` or a retained reply target.
The existing `native_attachments` URL/base64 materializer retains its 8 MiB
aggregate limit, 64 KiB aggregate inline limit, four-attachment limit and bounded
public-address download rules. Bytes are forwarded unchanged; no transcoding,
credential exposure or platform media-ID authoring is added.

| Destination | Explicit kind | Supported profile | Native operation |
| --- | --- | --- | --- |
| Telegram exact original chat/topic | `voice` | One complete mono Opus Ogg stream; `audio/ogg`, `.ogg`; caption allowed | `sendVoice` |
| Telegram exact original chat/topic | `animation` | GIF up to 1920×1080 and 256 frames, or the existing bounded Baseline H.264 MP4 profile without audio; matching `.gif`/`.mp4`; caption allowed | `sendAnimation` |
| Telegram exact original chat/topic | `sticker` | Static WebP, exactly 512×512, up to 100000 bytes; `.webp`; no caption | `sendSticker` |
| WhatsApp original private recipient/message | `voice` | One complete mono Opus Ogg stream; `audio/ogg`, `.ogg`; no caption | Existing media upload followed by `audio: {id, voice: true}` |

The existing WhatsApp outbound-media gate and credential/source custody checks
also apply to voice messages. Ordinary WhatsApp `audio` omits `voice`, exactly as
before. Its existing static/animated WebP sticker support is independent of this
new gate. WhatsApp groups, Discord, Lark, relay and initiated/default-destination
sends do not acquire these new presentation capabilities.

Preparation validates the entire batch before text or media effects. Each native
upload/send retains the existing per-effect authority checks and uncertainty
handling. A mismatched Telegram chat/topic or missing expected presentation in
the response is unconfirmed. A lost upload/send response is never retried
automatically. Durable keyed replies keep their confirmed or unknown receipt
across restart, including after issuance is disabled; changing `audio` to `voice`
under the same key conflicts rather than causing another send.

These presentations do not combine with controls or albums and issue no public
edit/delete/reaction operation references. Media patches and existing card
replacement paths reject their kinds. GIF animation, voice and static stickers
must be requested explicitly; ordinary generic attachments are never silently
reclassified. Structural checks do not certify full media decodability or actual
provider/client acceptance.

This slice does not add Telegram animated WebP/TGS/WebM stickers, video notes,
Lark received-key sticker sends, or Discord voice under this shared gate.
[Native Discord voice](CMAEG_Native_Discord_Voice.md) has an independent gate and
a canonical PCM WAV profile with derived duration/waveform, exactly one file,
no text/stickers and no edits; generic audio uploads do not satisfy that contract.

Validation is covered by Rust wire, invalid-profile, gate, capability and retained
receipt tests, plus additive Rust/TypeScript SDK schema tests. Rust execution and
the code-derived OpenAPI snapshot equality check run in GitHub CI. Connected
platform acceptance remains separate; this implementation does not claim it.
