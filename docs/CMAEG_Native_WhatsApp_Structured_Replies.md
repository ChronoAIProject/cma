# Native WhatsApp contact and static location replies

`CMAEG_NATIVE_WHATSAPP_STRUCTURED_REPLIES_ENABLED` defaults to `false` and also
requires native WhatsApp writes. It extends the exclusive neutral
`structured_content` request to the original retained private WhatsApp event.
Current per-event discovery advertises `structured_replies` only when the
original plan, source credentials and selected consumer still authorize it.
Use `replyToEvent` with that `event_ref` and a stable `idempotency_key`.

The three kinds map to Meta's existing Messages API:

| Neutral content | Meta message |
| --- | --- |
| `contact` | One `contacts` entry with `name.first_name`, optional `name.last_name`, their combined `name.formatted_name`, and one `phones[].phone`. |
| `location` | `location` with decimal latitude and longitude. |
| `venue` | `location` with those coordinates plus `name` and `address`. |

Meta has no vCard or horizontal-accuracy member in these request shapes. A
contact with an explicit `vcard`, including an empty one, or a location with
explicit `horizontal_accuracy` is refused before any write. All neutral string,
coordinate and total-size bounds still apply. Contact phone numbers describe
the attached contact; they never choose the delivery recipient or assert a
WhatsApp user identity. No contact lookup, geocoding, URL fetch or live tracking
occurs. Native group messages and relay transports are outside this profile.

The outgoing business phone, private recipient and quoted parent message come
only from the original sealed event and retained routed plan. Source/account,
original actor, private conversation, caller and consumer must agree. The normal
durable send gate rechecks current authority immediately before the one Graph
request. Existing customer-service-window restrictions still apply; a rejected
free-form message is not converted into a template or sent to another recipient.

A confirmed result requires exactly one message receipt and the same recipient
`wa_id`. It is saved in the existing WhatsApp receipt store, so subsequent native
delivery/read observations retain the original event, conversation and consumer.
Uncertain responses or receipt persistence failures remain unknown, with no
automatic resend. Retrying the same key recovers the durable result, including
after worker restart with issuance disabled. A changed body under the same key
is refused. Updates, initiated/default destinations and delayed structured
replies retain the neutral contract's explicit refusal.

Deploy compatible Gateway, SDK and persisted-message readers before enabling
the flag. Retain a compatible rollback reader while structured message records
exist; disabling new issuance does not make an older strict reader compatible.
Ordinary messages, existing Telegram structured replies and their hashes are
unchanged. No new public schema or custom recipient API is introduced.

## Validation and provider evidence

Local fixtures assert official request shapes, receipt matching, unsupported
fields, private source proof, default/named consumer isolation, confirmed and
unknown replay after a gate-off restart, request conflicts and source revocation.
These tests do not send to WhatsApp. Connected delivery and client rendering
remain unverified until owner testing.

Official contracts reviewed on 29 October 2026:

- [Contacts messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/contacts-messages/), retained document SHA-256 `47ce7c58618a166addf6760d8a3baf5287c07c2dfa421510b425336ffa80d176`.
- [Location messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/location-messages/), retained document SHA-256 `ef19bb8d3730dd11f1e080c085efa422ee1d08c5a3b0afc0d1b97138e65ec885`.

Current official examples use Graph v26.0. This change retains CMA's existing
v25.0 transport and does not upgrade other WhatsApp operations. Tests assert
that exact v25.0 route; connected version-specific acceptance remains unverified.
