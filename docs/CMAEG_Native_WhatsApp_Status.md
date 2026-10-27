# Native WhatsApp delivery status

Authenticated Meta status callbacks annotate the exact retained outgoing message
under its original Channel Activity and consumer. A status never launches an
agent, changes routing, retries the original send or grants message authority.
This applies to native private messages and existing-group messages, including
per-participant group receipts. It is independent of the relay transport.

The existing webhook verifies the original bytes with the current app secret
before parsing status facts. App, WABA and phone identity come from the current
source. The retained confirmed-send receipt binds channel, owner, record,
destination, platform message ID and original consumer/provider/conversation.
The original admitted plan must still prove that binding. A callback for another
phone, recipient, channel or message cannot update a known message by its ID
alone. Historical receipts remain observations even if the current routing rules
have changed; they never select a replacement consumer.

`sent`, `delivered`, `read`, `played` and `failed` are the observed provider facts.
Successful send responses are labelled **Accepted by WhatsApp**. Subsequent
Activity rows distinguish **Sent by WhatsApp**, **Delivered to recipient**,
**Read by recipient**, **Voice message played** and delivery failure. A read
callback does not invent a delivered callback or timestamp; Meta may omit the
intermediate callback. Group callbacks retain the group and, when present, the
individual recipient participant; one participant's receipt is not a receipt
from every group member.

The existing public Activity delivery shape remains unchanged. The additive
`platform_status` step reports `sent`, `delivered`, `read` or `played`; the
`platform_failure` step reports `failed`. `confirmed_at` is Meta's reported
status timestamp and `observed_at` is the gateway's first observation of that
fact. Success advances monotonically, independent of callback arrival order;
failure evidence is separate and cannot erase a success fact. Repeated and older
success callbacks do not change the saved row or trigger a new stream update.
The normal seven-day, fifty-event Activity bound and sixteen recent deliveries
per consumer remain in force; this surface is not an unlimited platform archive.

A callback can race the confirmed-send receipt commit. Missing receipts retain a
120-second retry opportunity; a transient receipt/plan/Activity persistence error
returns 503 with `Retry-After: 5`. Retrying this callback only retries observation.
Older unknown or expired messages are acknowledged without acquiring a target.
Each request has at most 100 status entries, eight concurrent observations and a
15-second observation budget. The entire raw request remains subject to the
existing ingress body limit and signature check. Future, malformed or stale facts
do not invent status. Unknown status kinds have no inferred meaning. Provider
error messages, billing fields, opaque callback data and URLs are not copied into
owner Activity or used to fetch anything.

There is no new outbound issuance gate: these are additive observations of
already-confirmed sends, using the existing encrypted Activity storage and SSE
stream. No schema migration, runtime change or infrastructure resource is needed.
Old readers accept the existing string fields; older gateway versions simply do
not reconcile new callbacks. The callback response's `accepted` and `refused`
counts retain their original incoming-message admission meaning; statuses do not
increase either count.

Official reference: [Meta status messages webhook](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/status/),
updated September 14, 2026, retrieved October 28, 2026. The reviewed page documents
all five status values, read-without-delivered optimization and group
`recipient_type`/`recipient_participant_id`. HTML SHA-256:
`2cb639f45ce74595d79273bafc71e546db721cc065e24ab6a0897a817d340ce5`.
Local HTTP/storage tests prove authentication, binding, replay, restart and
reordered callback handling. They do not establish live Meta delivery or client
presentation; those remain owner platform acceptance checks.
