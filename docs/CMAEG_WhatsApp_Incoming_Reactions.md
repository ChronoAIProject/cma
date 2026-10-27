# WhatsApp incoming reaction contract

Reviewed 2026-10-27 from CMA commit
`87331bbf515e02a69ad0d0bd3b80aa3624239bfa` against NyxID commit
`2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f`, inspected read-only.

Incoming WhatsApp reactions cannot be implemented through that existing NyxID
relay contract by changing CMA alone. The upstream adapter discards reactions
before creating an inbound message or callback. CMA receives neither the reaction
nor its target, sender, event identity or removal signal. Accepting a handcrafted
reaction callback in a local test would not establish an upstream-emittable path.
This is a limitation of the pinned integration, not a claim that WhatsApp itself
does not provide reaction webhooks.

## Where the event is lost

All upstream links below are pinned to the reviewed NyxID commit.

| Stage | Source evidence | Consequence |
| --- | --- | --- |
| Meta authentication | [`WhatsAppAdapter::verify_webhook`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/whatsapp.rs#L863) verifies `x-hub-signature-256` against the raw body; `prepare_webhook` at line 888 filters the configured account and phone number | The upstream receives and authenticates the Meta webhook before parsing |
| Message extraction | [`parse_message`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/whatsapp.rs#L102) explicitly says reactions must not start conversations and excludes `reaction` from its allowlist | A reaction produces `None`, before sender, message ID and `raw_data` are copied |
| Webhook parsing | [`parse_inbound`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/whatsapp.rs#L932) pushes only messages accepted by `parse_message` | No reaction `InboundMessage` reaches routing |
| Handler dispatch | [`channel_webhooks`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/handlers/channel_webhooks.rs#L713) handles delivery receipts separately, then forwards only the parser's message list to `process_inbound_messages` | Status receipts do not provide a reaction bypass |
| Callback construction | [`build_callback_payload`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_relay_service.rs#L797) copies `inbound.raw_data` at line 861 | `raw_platform_data` preserves accepted messages; it cannot recover discarded ones |
| Existing upstream regression | [`parse_buttons_interactive_location_contacts_and_order`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/whatsapp.rs#L1412) includes `reaction` in the types that must return `None` | The exclusion is intentional and covered by upstream source tests |

The generic activity mechanism is also unavailable for this purpose.
[`PlatformAdapter::activity_descriptors` and `activity_metadata`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_platform.rs#L508)
default to an empty descriptor list and `None`; the pinned WhatsApp adapter
overrides neither. [`declare`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_activity_callback_service.rs#L71)
requires a nonempty subset of that adapter's supported kinds. Declaring or enabling
an activity callback cannot manufacture WhatsApp reaction support.

## Current CMA behavior

[`ingress/relay/whatsapp.rs`](../api/crates/cmaeg/src/ingress/relay/whatsapp.rs)
continues to refuse reaction input as `system_event`. Accepted messages require
the raw `from` account to match both normalized sender and private-conversation
identity. The observer retains `ActorKind::Unknown`: a WhatsApp account does not
prove that its sender is a human. The existing rejection coverage is in
[`tests/relay/verified_platforms.rs`](../api/crates/cmaeg/tests/relay/verified_platforms.rs).

Enabling raw verification, trusting normalized text, changing reply-completion
flags, or interpreting an emoji in ordinary message text cannot restore the
missing event. Normalized text has no authenticated target correlation or
add/remove/replace meaning. A Custom CloudEvent is authenticated as its configured
Custom producer; it does not inherit WhatsApp sender or native message authority.

No reaction parser, fabricated acceptance fixture, public schema, source flag or
runtime behavior was added by this review. No upstream code or deployment was
changed. Cargo builds and platform sends were not needed for this source-contract
finding.

## CMA-only implementation path

The separate native implementation is described in
[Native WhatsApp text and incoming reactions](CMAEG_Native_WhatsApp.md). It leaves
this pinned relay contract unchanged and remains a bounded, default-off source.

If NyxID must remain unchanged, CMA needs a separate native Meta webhook source
that receives the original reaction delivery before the relay's filter. This is
a new ingestion and provisioning path, not a flag on the existing relay. It
requires an explicitly configured Meta callback, account/phone binding and
encrypted verification-secret custody. A code-only change cannot cause Meta to
send those deliveries to CMA.

The native path should reuse the existing durable inbox and additive
`ActivityKind::Event` representation. A shared neutral event such as
`cma.message.reaction.updated.v1` can identify the target in its CloudEvent
`subject`; the exact schema should follow the native messaging-event contract
that is integrated with it. Before implementation or acceptance testing, preserve
these constraints:

- Verify the signature over the original request bytes and bind the configured
  WhatsApp account and receiving phone number before extracting a message.
- Validate the reaction's own immutable delivery/message ID, raw `from`, target
  message ID and provider timestamp. Bind the sender to the same private chat;
  retain unknown human/bot identity. Do not accept a target or actor from display
  text or a normalized callback hint.
- Resolve the target against retained admitted-message or confirmed outbound
  receipt evidence for that exact source, bot/account and private conversation.
  Retain the original consumer, provider, partition and conversation. A reaction
  must not start work on a newly configured route or establish authority over an
  unrelated message. Missing or expired target evidence needs an explicit
  unavailable outcome.
- Deduplicate using the authenticated provider delivery identity in the exact
  source scope. Persist the notification and its routing snapshot before provider
  execution; exact replays recover that snapshot without another turn. Preserve
  conflicts and uncertain outcomes instead of generating a fresh key.
- Treat a nonempty emoji as the sender's replacement reaction state and an empty
  emoji as removal only when that meaning is established by the native provider
  contract. Do not invent an earlier emoji or identify a replacement as a first
  addition when no prior state is known. Preserve the authenticated timestamp and
  define stale/out-of-order handling before maintaining any materialized state.
- Verify addition, replacement, removal, replay, target mismatch, sender/chat
  mismatch, revoked source, changed routing and unknown target with actual native
  webhook shapes. Keep transport verification distinct from real client/platform
  acceptance.

The smaller alternative is a future NyxID contract that actually forwards these
authenticated facts, followed by CMA propagation using the same durable target
and routing rules. That option requires upstream work and was not performed here.
