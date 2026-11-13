# Explicit native typing

The existing `sendTyping` MCP operation and `POST /v1/events/{event_ref}/typing`
REST operation can send one short-lived typing indicator for an original native
Discord message or an original private WhatsApp message. The input and receipt
are unchanged: `{event_ref}` and `{outcome:"sent"|"unsupported"}`. Typing is never
a reply, never suppresses the provider's eventual answer, and never runs as an
automatic status heartbeat.

When absent, `CMAEG_NATIVE_TYPING_ENABLED` defaults to `false`. The checked-in
ConfigMap explicitly sets it to `true`: the owner decision relayed on 2026-11-11
is to preserve production typing indicators and WhatsApp read receipts (#1624,
separated from the Secret-reference work in #1588). The manifest contract now
requires this approved activation deliberately.

The corresponding native writer and durable admission/recovery gates still
apply; Discord also requires its native runtime gate. This does not activate a
disabled channel or qualify connected-platform acceptance. Local fixtures do
not prove that a real recipient saw an indicator. Release-train application
uses the normal ConfigMap/Deployment fences and restart/readback checks.
Disabling the gate restores `unsupported` and `capabilities.typing:false` for
these native transports without affecting Telegram's existing behavior. Any
later behavior change needs a separate decision.

| Source | Native operation | Successful response | Duration and side effect |
| --- | --- | --- | --- |
| Discord API v10 | `POST /channels/{original_channel_id}/typing` | HTTP 204 | Expires after 10 seconds. An original thread uses that thread's own channel ID. |
| WhatsApp Cloud API v25.0 | `POST /{original_phone_number_id}/messages` | JSON `{"success":true}` | Marks the original incoming message **read** and shows typing until a response or 25 seconds, whichever comes first. |
| Native Lark/Feishu, NyxID relay, Custom, WhatsApp groups | None | `unsupported` | No verified native typing endpoint is implemented for these sources. |

WhatsApp's exact request is:

```json
{
  "messaging_product": "whatsapp",
  "status": "read",
  "message_id": "<retained original incoming message ID>",
  "typing_indicator": { "type": "text" }
}
```

Call only when preparing a response. There is no caller-supplied destination,
phone, account, message ID, or typing duration. Context reports `typing:true`
only when the enabled implementation can prove retained native authority.
Context discovery performs no platform write.

## Authority and failure behavior

An unexpired event must match the immutable routed plan, original ordinary
message, source, owner, channel record, selected consumer, and provider binding.
Both default and named consumers use their original selected target. Arbitrary
sealed IDs, synthesized activity without the retained native bytes, lineage
events, interactions, changed native message identities, revoked keys, removed
consumers and changed providers do not authorize typing.

Discord requires the exact retained human message and channel/thread. Immediately
before writing, CMAEG verifies the current bot identity and re-reads that message;
bot, system and webhook authors and non-message events are refused. WhatsApp
requires the exact retained app, WABA, phone, sender and ordinary text/media
message. It verifies current token app/scopes and phone membership, restricts
typing to private conversations, and conservatively permits only an original
message timestamp within the previous 24 hours, never a future timestamp. This
age restriction is CMA policy; Meta's typing page does not itself specify that
window. The activity timestamp must match the retained message timestamp.

After these reads, CMAEG reloads owner/source/provider/consumer/agent-key and
native-credential custody immediately before the single platform write. An
in-flight credential or authority change refuses the write. New requests use
current credentials. Native execution has a seven-second budget bounded by event expiry and shares
a 32-operation process limit; context authority discovery has a two-second
budget. Rate limits return HTTP 429. An uncertain, malformed, timed-out or lost
platform response returns an error, never `sent`; a definitive authority refusal
returns HTTP 403. No background task, automatic retry or periodic refresh is
started. Callers must not blindly retry an uncertain write.

## Provider contract evidence

Reviewed on 2026-10-29:

- Discord's official [Trigger Typing Indicator contract](https://github.com/discord/discord-api-docs/blob/2b434c56eca46b61461bf5578499dd7810e2f6af/developers/resources/channel.mdx#trigger-typing-indicator)
  pins the source commit, POST route, ten-second lifetime and HTTP 204 response.
  SHA-256 of the raw document is
  `13adf0e407678d5bdc76d4c9dc7e28bde382021d07f1bd3cfef5cbe74a7abfb1`.
- Meta's official [Typing indicators](https://developers.facebook.com/documentation/business-messaging/whatsapp/typing-indicators/)
  documents the read receipt, exact message payload, success response and
  25-second maximum. The implementation uses the existing pinned Graph v25.0
  client. SHA-256 of the retrieved page is
  `bbb9afee7613a4b18d8989f48828483344787d5f8ba161106112759603c68d8d`.

Local contract tests exercise both public entry points, default/named consumers,
original thread targeting, exact WhatsApp payload, stale/future/native-byte
mismatches, authority and credential changes during preflight, fresh native
identity checks, disabled gates, capacity, errors, and bounded timeout behavior.
They use local HTTP servers and injected Graph transport, with no external sends.
