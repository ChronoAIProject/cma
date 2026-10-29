# Native Telegram contact, location and venue replies

`CMAEG_NATIVE_TELEGRAM_STRUCTURED_REPLIES_ENABLED` independently enables three
static reply kinds on an original, retained native Telegram event. It defaults
to `false`; native media, albums and live location remain separate. The caller
must have current authority for the original routed default or named consumer.

`readEventContext` and `verifyEventContext` advertise
`capabilities.structured_replies: ["contact", "location", "venue"]` only when
the gate, current credential and retained original event are available. An absent
field does not imply support. The advertised capability performs no platform send.

Use `replyToEvent` with the original `event_ref`, a stable `idempotency_key`, and
one exclusive `structured_content` value:

```json
{"structured_content":{"type":"contact","phone_number":"+6512345678","first_name":"Alice","last_name":"Example"}}
```

```json
{"structured_content":{"type":"location","latitude":1.3521,"longitude":103.8198,"horizontal_accuracy":10}}
```

```json
{"structured_content":{"type":"venue","latitude":1.3521,"longitude":103.8198,"title":"Meeting point","address":"Singapore"}}
```

The variants map to Telegram `sendContact`, `sendLocation` and `sendVenue`.
Contact accepts optional `last_name` and `vcard`; vCard is at most 2,048 bytes,
matching Telegram's documented limit. CMA bounds phone numbers to 128 bytes,
names and venue titles to 1,024 bytes, and addresses to 4,096 bytes. These are
CMA limits where Telegram does not state one. Text must be nonempty and contain
no control/bidirectional characters; vCard may be empty and permits CR, LF and TAB.
Coordinates must be finite, with latitude −90…90 and longitude −180…180.
Optional horizontal accuracy is 0…1,500 meters.

These payloads grant no recipient authority. CMA copies only the bot, chat and
existing topic from the original sealed, retained event. The payload has no
chat/topic selectors, explicit reply target, arbitrary default destination,
business identity, paid broadcast, ephemeral send, controls or attachments.
Location is static: live periods, headings and proximity tracking are rejected.
Venue data is supplied by the caller; there is no geocoding, place lookup or
provider place-ID surface. vCard is opaque data and causes no URL fetch.
Text, blocks, templates, uploads and other message selectors cannot be combined
with structured content. Updates, initiated sends and delayed reply targets
refuse it. This slice issues no message-edit or message-effect capability.

## Durable delivery and rollout

The original activity digest, request hash, plan, target, consumer and provider
must agree. The normal routed send gate checks current authority before the
single Bot API attempt. Receipt validation requires the original chat/topic,
positive message identity, expected content kind and consistent bot/channel
sender. A mismatched response or uncertain transport is `unknown` and is never
blindly retried. Retry the same key and content to recover the durable result;
different content with the same key is refused. A distinct intentional send uses
a new key. Source or consumer revocation remains authoritative on replay.

Deploy compatible Gateway, SDK and durable message readers before enabling the
writer. The new optional field is absent from legacy serialization, preserving
old message bytes and hashes. Older strict readers cannot read a persisted
structured message, so retain a compatible rollback image while these records
exist. Disable new issuance instead of reverting to an incompatible reader;
confirmed and unknown keyed results remain recoverable without another send.

Local fixtures exercise schemas, bounds, exact Bot API payloads, destination
checks, keyed confirmed/unknown replay after a gate-off worker restart, missing
keys, delayed sends, changed requests and revocation. They do
not send to Telegram. Connected delivery and client rendering remain unverified.

## Official contract evidence

Reviewed 29 October 2026: [sendContact](https://core.telegram.org/bots/api#sendcontact),
[sendLocation](https://core.telegram.org/bots/api#sendlocation), and
[sendVenue](https://core.telegram.org/bots/api#sendvenue). The retained Bot API HTML
has SHA-256 `2f93d2e82fcaa215609744cc54f7a946a3d9e2326683f464162c812f667d36a8`.
Each method returns a Telegram Message on success and accepts the existing
`chat_id` and optional `message_thread_id`; CMA supplies those from custody.
