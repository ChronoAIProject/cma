# Native Discord voice messages

`CMAEG_NATIVE_DISCORD_VOICE_ENABLED` independently enables explicit native
`kind: "voice"` replies. It defaults to `false` and does not depend on the
Telegram/WhatsApp presentation gate. Existing `kind: "audio"` requests retain
their original multipart payload and behavior.

## Provider contract and bounded supported profile

The official [Discord Message documentation](https://docs.discord.com/developers/resources/message.md#voice-messages),
reviewed on 29 October 2026, requires `IS_VOICE_MESSAGE` (`1 << 13`), exactly one
audio attachment, no message content or stickers, and attachment
`duration_secs` and `waveform`. Voice messages cannot be edited. The waveform is
a base64 byte array with at most 256 points, sampled no more often than every
100 milliseconds. The attachment content type must start with `audio/`.

The reviewed official Markdown has SHA-256
`d60bef5c83edabe364708acc944d8249ecbe7cc1ac5d1272c5aacbe5e4ddb235`.
Its Create Message section also documents `message_reference` for replies and
allows `IS_VOICE_MESSAGE` in `flags`. The voice section does not enumerate an
accepted codec whitelist or explicitly guarantee WAV playback; the bounded WAV
request follows its stated `audio/*` and attachment-metadata contract. Actual
Discord acceptance remains unverified and this gate stays off until verified.

Discord describes mono 48kHz 32kbps Opus in Ogg as its clients' implementation
detail, which may change. CMA preserves the original **canonical mono
signed 16-bit little-endian PCM WAV at 16kHz, from 0.1 to 120 seconds**. This
is exactly the WAV structure produced by CMA's existing sandboxed PCM helper.
It has one 16-byte PCM `fmt ` chunk, one `data` chunk, a 44-byte header, and no
extra chunks or trailing bytes. The maximum complete file is 3,840,044 bytes.
`audio/wav` is the only declared MIME type; it and `voice.wav` are inferred when
omitted. Filenames may otherwise be supplied normally.

The exact header is reconstructed from actual file length and compared before
delivery. Duration is actual PCM sample count divided by 16,000. CMA partitions
the entire recording into at most 256 contiguous bins, each at least 100ms, and
scales each bin's peak absolute sample amplitude to one byte in 0–255. Silence
is zero. The base64 waveform therefore covers the whole clip, including the end;
it is never derived from compressed byte size or caller-supplied estimates.
Peak amplitude is CMA's preview convention, not a claimed Discord encoding rule.

An additional profile accepts **one complete mono Opus Ogg stream**, at most
8 MiB, from 0.1 to **less than 120 seconds**. It requires the existing configured
media transformer (`CMAEG_MEDIA_ENABLED=true`, `CMAEG_MEDIA_FFMPEG_DIR` pointing
to the packaged helper). The original encoded bytes are uploaded unchanged as
`audio/ogg`; that MIME and `voice.ogg` are inferred only when omitted. Ogg pages,
checksums, packet framing and the mono Opus headers are validated before decoding.
Chained, multiplexed, stereo, malformed or incomplete containers are refused.

CMA decodes the original through the same `MediaTransformer::audio` instance as
inbound media, with the same process slot, watchdog, deadline and in-flight byte
budget. The decoder is capped at 120 seconds. A truncated result or a result at
that cap is refused, since it cannot prove that the whole recording was decoded.
Duration and the complete waveform come from the decoded PCM samples using the
same convention as WAV. The prepared upload owns the original bytes and derived
metadata together; the REST layer cannot substitute a different file. No encoder,
new codec build dependency or arbitrary subprocess is added. This path does not
transcode the uploaded Ogg, enforce a particular bitrate, or promise Discord's
current client profile for files supplied by callers.

Other WAV layouts, stereo, unsupported formats, partial frames, malformed lengths
and out-of-range duration are refused. A missing or failed shared decoder refuses
Ogg before any platform send; WAV does not depend on that decoder. These paths
have local fixtures; neither claims connected Discord acceptance or playback.

## Message and authority constraints

Send one voice native attachment with no `text`, blocks, caption, controls,
template or custom reply target. The exact original Discord destination and
message are used. Existing verified thread destinations remain supported; a
request cannot substitute a parent channel or another thread. The native source
and bot token must still be valid, and the retained event must belong to the
current channel/record with a verified activity digest. Forwarded lineage is
ineligible.

The adapter validates bytes before the delivery commit, verifies the bot, then
makes one multipart message POST with the voice flag and derived attachment
metadata. It requests a reply to the original message with
`fail_if_not_exists: true`, suppresses mentions and never falls back to a text
message or an ordinary audio upload.

A confirmed receipt requires the exact returned bot, destination, original reply
target, voice flag, single attachment and declared presentation metadata. A lost
or inconsistent result is `unknown`; it does not cause an automatic resend.
Discord normalization of MIME, waveform or duration also yields `unknown` under
this deliberately strict check, even if the platform accepted the message.
The ordinary durable operation receipt and request-hash rules still apply:
retry the same operation ID, never create a new ID to retry uncertainty. Voice
and Audio remain distinct request identities. Disabling issuance does not prevent
replay of a retained receipt.

No edit, reaction, control, media-replacement or public message-operation
reference is issued. This path does not create an outgoing reaction target.
It changes neither NyxID relays nor Telegram, WhatsApp or Lark behavior.

## Discovery, rollout and verification

An authenticated, verified original Discord event advertises a `voice`
`presentation_profiles` entry with `audio/wav`, the byte limit,
`max_attachments: 1`, `text: false`, `caption: false`, and
`message_operations: false`. When the shared decoder is configured, a second
`voice` profile advertises `audio/ogg`, the 8 MiB limit, and the strictly bounded
Opus range. The original WAV profile and old request serialization stay unchanged.
Both profiles are absent when the writer gate is off or the destination does not match. General media/control capabilities retain
their existing meanings; their explicit kinds exclude voice.

Deploy compatible readers before enabling the independent gate. Existing
operation receipts retain the same schema, and optional discovery fields are
additive. Local regressions cover PCM amplitude/duration/bounds, gate independence,
exact destination/thread, revoked sources, denied commit, mixed-message refusal,
uncertain responses without resend, absent effect/control authority and unchanged
ordinary Audio payloads. Opus regressions cover unchanged encoded bytes,
PCM-derived duration/waveform, missing/failed/truncated decoder results, decoder
budgets, invalid Ogg inputs, denied commit and unknown receipts without resend.
The existing image self-test decodes the committed Opus fixture through the real
sandbox; the new composed tests inject its decoder result. Live provider
acceptance and client playback remain unverified; the writer gate stays off.
