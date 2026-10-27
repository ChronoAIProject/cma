# Native WhatsApp reactions

The native integration reacts to the exact user message received by a configured
Meta source. It does not infer permission to edit or delete that message or to
react to arbitrary message IDs. NyxID relay behavior is unchanged.

When `CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED=true`, an authenticated native
message context can contain `received_message`, with a sealed `message_ref` and
its `effects`. The scope is `received_native_user_message`, `delete=false`,
`reaction=true`, and `max_reactions=1`. Pass that reference to the existing
`readMessageCapabilities` and `setMessageReaction` tools. One Unicode emoji
sets or replaces this bot's reaction. An empty reaction array clears that
reaction by sending `reaction.emoji=""`, as Meta's official Reaction Object
reference specifies. Custom Telegram emoji IDs and `is_big=true` are refused
before any platform write.

The reference is issued only for a verified original `message` Activity, matching
native source, private sender/conversation, original record and exact consumer.
An immutable tool-reply binding and retained original routing plan must match
the exact event, Activity, selected consumer and original source/provider.
Unavailable or expired metadata omits this optional capability without failing
the ordinary context response. Current credentials may mint a new reference;
the resulting reference pins that current revision.
Reaction events, relay events, handoffs and group/thread projections receive no
such authority. The reference expires with the original event, and never beyond
30 days after the source message timestamp. The source policy, Meta credentials,
provider, consumer credential/key and routing state are hashed into its current
authority; changing them revokes old references. No token is returned to agents.

Each call requires the existing printable `Idempotency-Key`. Replaying the same
key and body returns its durable receipt; changed content under the same key
conflicts. A per-target durable fence serializes different keys, including
different consumer contexts for the same native message. An ambiguous Graph
write or lost final receipt retains that fence. Both receipts and fences survive
until 30 days after the source timestamp, even when the first reference expires
earlier. Receipt identity excludes mutable credentials and authority revisions,
so a freshly minted reference cannot resend an already claimed idempotency key.
A new key does not make an uncertain reaction safe to resend. Current source custody is checked again after
the fence is acquired, immediately before the one native HTTP write.

Meta's HTTP success confirms API acceptance only. Its documented reaction
delivery can later fail, for example when the target was deleted, is itself a
reaction, is absent from the conversation, or is more than 30 days old. Reaction
messages have `sent` status webhooks but no delivered/read status webhooks.
No claim of confirmed recipient presentation follows from a successful request.

The native transport remains pinned to Graph `v25.0`; current Meta documentation
shows `v26.0` examples. Connected acceptance, including remote status outcomes,
remains a separate platform verification requirement. Local transport and composed custody/idempotency tests do not prove live Meta behavior.

Before enabling issuance, deploy compatible readers for the new optional
`whatsapp_reaction` sealed-message field and the optional `received_message`
context field. Keep a compatible Gateway image as the rollback floor after
issuance. Turning the flag off stops issuing new references; retained references
still obey their original expiry and current-authority checks. No runtime or
NyxID rotation is part of this feature.

Official contract reviewed on 28 October 2026:

- [Reaction messages](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/reaction-messages/)
- [Messages API](https://developers.facebook.com/documentation/business-messaging/whatsapp/reference/whatsapp-business-phone-number/message-api)
- [Meta's official Reaction Object reference](https://www.postman.com/meta/whatsapp-business-platform/documentation/wlk6lh4/whatsapp-cloud-api#e754eff9-df94-463b-8ff0-9addcc8f2bd1)

The official Reaction Object states: “All emojis are supported, however only one
emoji can be sent in a reaction message. Set this value to "" (empty string) to
remove the reaction.” This is the unversioned outgoing Messages Object contract;
its example response uses an older Graph version. It is evidence for the clear
request shape, not connected acceptance on the pinned v25.0 endpoint.

The published request is `POST /{version}/{phone-number-id}/messages`, with
`messaging_product=whatsapp`, `recipient_type=individual`, exact recipient,
`type=reaction`, and `reaction.message_id` plus `reaction.emoji`. The response
must contain exactly one matching `contacts[].wa_id` and one bounded `wamid.`
message identifier before CMA accepts it as a confirmed API outcome.
