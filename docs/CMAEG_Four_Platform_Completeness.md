# Four-platform messaging audit

This audit compares CMA routing integration base `f611b2f29` (including the
merged messaging work from PR #1118) with read-only NyxID `origin/main` pinned at
`2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f`. The matrix describes implemented
code paths, not production enablement or a claim of live platform verification.
No NyxID code, deployment or external messages are changed by this audit.

| Platform | Implemented ordinary ingress | Implemented replies and controls | Remaining boundary |
| --- | --- | --- | --- |
| Telegram | Native webhook and authenticated NyxID relay; private/group text, raw sender/addressing checks, image/file/audio/video/sticker facts and media retrieval; album identity | Native and relay text; native buttons and single/multiple choices; gated native reaction/delete effects; native Bot API image/file/audio/video uploads; gated relay native media | Native media is locally verified, not live-platform verified. Relay attachments require the native-media rollout gate and compatible NyxID replicas. Relay effects do not inherit native Bot API authority. |
| WhatsApp | Opted-in raw Cloud API verification for private messages; text, image/file/audio/video, voice/stickers, locations/contacts, context correlation | Text; gated native media, reply buttons, single-choice lists and approved template replies | The legacy normalized source is text-only. Enable raw verification through its write gate first. No editing or group support is claimed; the Cloud API does not prove that the sending account is a human. |
| Lark / Feishu | Verified private/group text; raw rich-post flattening; standalone image/file/audio/video retrieval; thread contracts | Text; gated native media, JSON 2.0 cards and text edits | Embedded post image/video remains a visible placeholder: NyxID stores `embedded_attachments`, but its external relay callback and indexed attachment contract expose only `inbound.attachments`. Stickers remain refused. Thread behavior needs the declared thread contract and exact current source authority. |
| Discord | Raw verifier for signed relay slash commands and Gateway-shaped `MESSAGE_CREATE`; correlated image/file/audio/video attachments and explicit mention/reply evidence | Relay text; gated native media and ordinary text edits | A parser is not live Gateway intake. Pinned NyxID accepts signed Interactions HTTP webhooks and has no Discord Gateway WebSocket receiver; CMA previously had none either. Native Gateway intake can be owned by CMA. Discord interactive consent is not ready: pinned NyxID treats type 4 as a component, whereas Discord components are type 3 and type 4 is autocomplete. CMA correctly refuses type 4. |

The matrix is supported by these concrete paths:

- CMA `ingress/relay/facts.rs` selects raw observers independently from the
  explicitly trusted normalized observer. `discord.rs` and `whatsapp.rs`
  correlate native actor, conversation, parent and attachment identities.
- CMA `delivery/relay_client/media.rs` constructs native attachment requests for
  Telegram, Lark/Feishu, Discord and WhatsApp. `send.rs` refuses them before
  delivery when the gate or destination contract is absent.
- CMA `interactions/`, `telegram/effects.rs`, `delivery/edit.rs` and
  `delivery/relay_client/whatsapp.rs` implement the bounded controls above.
- NyxID `channel_adapters/discord.rs` verifies `X-Signature-Ed25519` and
  `X-Signature-Timestamp`, registers an Interactions endpoint and parses slash
  commands. Its `parse_gateway_message` helper has no Gateway socket producer.
  Its `INTERACTION_MESSAGE_COMPONENT` constant is 4 at the audited pin.
- NyxID `channel_adapters/lark.rs` stores post `embedded_attachments`, while
  `channel_relay_service.rs::build_callback_payload` enumerates only
  `inbound.attachments` into the external attachment download indices.

The checked-in CMA deployment enables inbound media but leaves new native
relay sends, raw-verifier writes, relay text edits, Lark cards, WhatsApp
templates/interactions, named interaction issuance and native Telegram effects
off. These capabilities can be enabled on the CMA side after all serving and
rollback readers understand their retained schemas, upstream contracts are
verified, and an authorized controlled platform test confirms the result.
Their existence in code is not permission to send test messages to recipients.
None of these flags creates ordinary Discord Gateway intake or fixes the
upstream Discord component-type mismatch.

## Native Discord transport slice

`cmaeg/src/discord/` adds a concrete Discord v10 JSON Gateway client using the
workspace's existing pinned `tokio-tungstenite`. It opens the real WebSocket
transport when explicitly invoked; no runtime bootstrap or configuration invokes
it in this change. It never fabricates a NyxID callback or borrows relay authority.

The client supports HELLO, IDENTIFY, RESUME, immediate/requested heartbeats and
ACK liveness, reconnect requests, invalid-session handling and typed close-code
refusals. It requires the privileged MESSAGE_CONTENT intent for this ordinary
message use case; a missing intent is refused locally and a 4014 close is a
terminal refusal. Only message-related intents are accepted. Sharded operation
is not implemented; a platform sharding requirement is a typed refusal.

Frames are bounded to 1 MiB, the pending dispatch queue to four entries, and
outbound Gateway commands to 110 in a rolling minute, below Discord's 120 limit.
Compression is not requested. The bot token is private memory and never a public
field or diagnostic. Resume endpoints must be Discord Gateway WSS origins with
no userinfo, alternate port or arbitrary path. READY verifies the expected bot
identity before committing any event. A slow admission does not block heartbeats;
queue pressure ends the connection with its last durable checkpoint for replay.

`NativeDiscordJournal` is a mandatory boundary, not a production implementation.
Before connecting it must validate current native source ownership and credential
revision. Fresh connections must reserve IDENTIFY against current
`GET /gateway/bot` session-start limits and the five-second concurrency buckets.
Resume never silently falls back to an unreserved IDENTIFY. Every dispatch is
processed in order. Its checkpoint advances only after the journal confirms
durable admission. Message admission must be unique by native source record and
Discord message ID, independently of Gateway session/sequence; a fresh session
can redeliver an old message. A failed or canceled commit can have succeeded
remotely, so reconnect replay must restore the original admission idempotently.

Mock WebSocket tests exercise the real framing and protocol commands, admission
ordering, replay and native-ID deduplication, heartbeat failure, bounded
backpressure, invalid sessions, intent/authentication close codes and URL/identity
refusals. The mock journal demonstrates the required contract; it is not evidence
that persistent source management is already implemented.

## Required integration before native Discord activation

1. Add an explicit native Discord source, Activity source kind and reply authority.
   Extend channel validation and public schemas additively and gate new writes
   until all readers and rollback targets support them. Do not encode a native
   socket event as `NyxidRelay` or a custom producer.
2. Store the bot token only in the encrypted channel credential schema. Verify
   the owner's bot identity and requested intents; redact public reads, rotation
   diagnostics and logs. Use durable per-bot ownership to prevent multiple CMA
   replicas from independently identifying for one source.
3. Implement current `/gateway/bot` discovery and durable session-start quota
   reservations, including reset windows and concurrency buckets. Persist source-
   bound resume checkpoints and fence every admission with current ownership.
4. Map ordinary messages through native Discord validation and the shared source
   admission policy. Bind bot/guild/channel/author identities, refuse system and
   disallowed bot messages, retain native media handles and use stable message
   idempotency before launching any provider work. Preserve original routing on
   retry rather than routing again after a policy edit.
5. Add native Discord REST text/media replies and media retrieval with exact
   source credential custody, bounded responses and platform rate limits. Keep
   uncertain sends unknown; do not blindly repeat a send after a lost response.
6. Wire owned connection supervision into CMAEG startup and source lifecycle;
   stop on source revocation, drain on rollout, recover durable work and obey
   reconnect delays. Test restart/owner takeover, token rotation, intent denial,
   message replay and media delivery before any production activation.

Ordinary Discord messaging is therefore an achievable CMA implementation path,
not a permanently blocked dependency on a NyxID change. This transport slice
alone does not yet expose that capability to users.


## Native source and journal follow-up

The next slice adds native `discord` source and Activity reader variants, a
sealed optional `discord_bot_token`, and the independent default-off
`CMAEG_NATIVE_DISCORD_WRITES_ENABLED` management gate. `DurableJournal::acquire`
reads the actual channel record, verifies the bot and Gateway discovery using
read-only Discord API calls, and acquires a globally bot-bound encrypted CAS
lease. `run_connection` executes the real transport with lease renewal and
shutdown fencing. Identify quota/bucket reservations and resume checkpoints are
persistent. MESSAGE_CREATE payloads plus public channel/routing snapshots are
chunked into an immutable encrypted inbox before checkpoint advancement, with
stable native message deduplication and seven-day retention. No NyxID envelope
or relay authority is involved.

The application still does not start native source supervision, dispatch the
native inbox into routed admission/execution, or provide native REST text/media
replies. Those are the remaining activation prerequisites; the source write gate
stays disabled in the deployment manifest. Bot transfer between channels also
requires explicit source lifecycle management. The initial transport-only
boundary above describes its original commit; this follow-up replaces its
credential/ownership/quota/journal gaps without claiming full messaging delivery.

## Native normalization, REST and media follow-up

The native adapter now has genuine MESSAGE_CREATE normalization/admission and an
immutable inbox-to-LaunchRequest reader, native sealed reply addresses, and exact
native attachment locators. Native REST text and multipart replies use current
bot custody, bounded preparation, disabled mentions, route gating/pacing and
conservative unknown outcomes. Inbound media refreshes exact message metadata
with the current bot credential and downloads only matching official CDN URLs
without forwarding that credential. These capabilities are covered by local HTTP
fixtures and do not require NyxID changes or native relay flags. The protocol's
native Discord section records the supported bounds and refusal behavior.

Startup source supervision and durable inbox scheduling into routed execution
remain separate composition work. The source write gate remains off; this slice
performs no live bot activation or external messaging. The earlier sections
record the preceding slices and their then-open integration boundaries.

## Native Telegram outbound media follow-up

The native Telegram path now accepts the existing explicit `native_attachments`
contract and sends actual Bot API multipart `sendPhoto`, `sendDocument`,
`sendAudio` and `sendVideo` calls. It shares Discord's unchanged bounded public
HTTPS/base64 materializer, prepares every attachment before any text or media
write, and retains the sealed bot/chat/topic, current source credential,
per-effect gate and pacing. Native capability discovery is independent of the
NyxID-only relay media gate; no configuration default or deployment is changed.

Local HTTP fixtures verify upload bytes, captions, filenames and MIME types for
all four methods, inline and fetched content, unsafe URLs and byte limits, bot
custody, topic-bound receipts, partial writes, rate limits and response loss.
Incomplete batches stay unknown and are never automatically resent. Media sends
issue no editable or effect-bearing message reference; ordinary text, interactive
cards and existing effects retain their previous code path. Outbound platform
file IDs/cross-platform locators are explicitly unsupported. Existing routed
lease/send windows remain in force and can end a media batch before its 60-second
outer ceiling. No live Telegram send, activation or deployment was performed.
