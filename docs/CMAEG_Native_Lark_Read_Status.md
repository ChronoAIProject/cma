# Native Lark / Feishu private-message read status

The native separate-app webhook accepts authenticated
`im.message.message_read_v1` notifications as delivery observations for exact
confirmed bot messages in private chats. They update the original sending
consumer's channel activity. They do not create an activity event, reroute an
event, launch a provider turn, send a message, or retry the original send.

## Reviewed provider evidence

The 2026-10-29 review uses already retained official documentation and SDK
artifacts; it makes no platform API call or connected-acceptance claim.

- The official [Feishu messaging index](https://open.feishu.cn/llms-docs/zh-CN/llms-messaging.txt)
  describes the [message-read event](https://open.feishu.cn/document/server-docs/im-v1/message/events/message_read.md)
  as firing after a user reads a bot-sent private-chat message. Its retained
  SHA-256 is `12681c2f68e21098da7038a5a6543b338e9563268686e3c90bb5d988ea756e1e`.
  The [Lark index](https://open.larksuite.com/llms-docs/zh-CN/llms-messaging.txt)
  links the corresponding regional event; SHA-256
  `da4c2945e79ed88de6576da22fcff8941475eba93d746f319cc664e92c25fde1`.
- Official Go SDK `larksuite/oapi-sdk-go` at
  `db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa` maps
  [the exact event type](https://github.com/larksuite/oapi-sdk-go/blob/db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa/event/dispatcher/im_v1_event_dispatch.go#L138)
  to `P2MessageReadV1`.
- Its [event model](https://github.com/larksuite/oapi-sdk-go/blob/db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa/service/im/v1/model.go#L16624)
  contains `reader` and `message_id_list`. The
  [reader model](https://github.com/larksuite/oapi-sdk-go/blob/db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa/service/im/v1/model.go#L3029)
  defines `reader_id`, `tenant_key`, and `read_time` as a millisecond timestamp
  string. The model file's retained SHA-256 is
  `978002449250a0d8e08bee36450d24c222caff7d2e1ab97963148ef6c6d10008`;
  dispatcher SHA-256 is
  `0b86e7b1f1683e2f484dee36fb0c583eebef0af5565d211bf35345861c713c53`.

The event does not provide a chat ID. The implementation never infers a chat
from a reader ID, uses another message in the batch as evidence, or queries
platform history to fill this gap. The original receipt and admission must
already prove the destination and private recipient.

## Matching and isolation

The existing encrypted webhook verifier checks the exact signature, request
timestamp, Verification Token, schema, receiving app and tenant before parsing
the status. Parsing requires a safe event ID, matching reader tenant, human
`reader.reader_id.open_id` distinct from the source bot, canonical positive
millisecond timestamp, and between one and 32 safe `om_` message IDs. The
32-item limit is a local work bound, not a claimed provider maximum. The whole
batch is validated before any observation; repeated IDs are deduplicated.

Each ID independently requires an unexpired `native_lark_receipt_v1` confirmed
send receipt, selected by the current channel and complete region/app/tenant/bot
source identity. No incoming message, send intent, uncertain transport result,
or successful neighboring item establishes this evidence.

The receipt's original owner and channel record must match the current channel.
Its retained admission must prove the same source, provider, sending consumer,
partition and conversation. The original conversation must be private with no
thread, and its retained human actor must exactly equal the read event's
`open_id`. The native reply must retain the same app identity, chat and credential
revision as the receipt. Missing human-recipient evidence is ignored; this
increment does not infer a recipient for unknown-actor notifications or groups.
The source/credential revision, owner, record and sending consumer/provider are
checked again against a fresh channel read before recording the observation.

Read times more than five minutes in the future, at least seven days old, or
more than five minutes before the confirmed receipt are not recorded. Receipt
and original-plan retention remain unchanged. Expired, foreign or unmatched
records cannot acquire read status. Corrupt or unavailable storage remains an
explicit retryable observation failure.

## Existing activity protocol

The original consumer receives the existing delivery shape:

```json
{
  "step": "platform_status",
  "destination": "origin",
  "outcome": "read",
  "observed_at": "<gateway receipt time>",
  "confirmed_at": "<provider read time, preserving milliseconds>",
  "platform_message_id": "om_confirmed",
  "recipients": [{"kind": "native_lark_chat", "id": "oc_original", "thread_id": null}]
}
```

The established status ordering preserves the strongest observation and its
first recorded timestamps on duplicates or reordered callbacks. Consumer
execution state and response ID remain unchanged. Activity contains no message
body or raw provider envelope. Its existing bounded retention and recent-item
limits apply; this is not a complete read-history archive.

A recent unknown receipt has a two-minute retry opportunity, allowing a callback
to race receipt persistence. Known messages in the same batch can record their
own observations while the callback returns HTTP 503 for pending receipt work.
After that window, absent receipts are ignored. Persistence failure also returns
503. Retried observations cannot trigger a send or a turn. An authenticated,
fully processed callback returns the existing `{"accepted":true}` response;
that acknowledgement does not assert that every supplied ID was eligible.

## Configuration and compatibility

Use the existing native separate app and `CMAEG_NATIVE_LARK_INTAKE_ENABLED` gate.
Subscribe that app to `im.message.message_read_v1` if read observations are
wanted. App installation, event subscription, effective permission approval and
actual event delivery remain owner-controlled platform setup. This source change
does not alter any subscription, credentials, deployment or NyxID source, and
does not claim a particular new permission is required beyond the provider's
effective subscription configuration.

No new issuance, source field, capability, environment variable, database
collection or migration is introduced. Confirmed receipts retain their exact
existing schema and bytes. `platform_status`, `read` and `native_lark_chat` are
already supported activity values, so no SDK schema or older receipt-reader
upgrade is required. Older gateway versions ignore this event type. Turning off
native intake stops ingestion without changing stored receipts or status data.

Regression coverage includes both regions, default and named consumers,
signature rejection, invalid reader/tenant/time/batch facts, duplicate and
delayed replay after observer restart, pending receipt races, mixed batches,
private-reader/channel/owner/chat/thread/provider/revision isolation, expired and
corrupt receipts, unavailable persistence, and credential rotation. Tests use
local fixtures with no usable turn launcher; no platform acceptance is implied.
