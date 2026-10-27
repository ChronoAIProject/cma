# Native WhatsApp reply buttons and lists

The native `whatsapp` adapter sends the existing public `Block::Buttons` and
single-select `Block::Choice` through Graph API `v25.0`, with authenticated,
durable callbacks in the original provider conversation. This is independent of
NyxID relay controls and adds no public message fields, routes or tool operations.
Local mocked tests establish the adapter contract; connected Meta acceptance is
not claimed by this implementation.

## Issuance and discovery

`CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED` defaults to `false`. Enable it only
after every serving and rollback Gateway understands the optional
`native_whatsapp` card authority and `native_whatsapp_interaction_payload_v1`
retained admission. Named consumers also require
`CMAEG_CONSUMER_INTERACTIONS_ENABLED`. `readEventContext` and
`verifyEventContext` advertise `buttons: "native_interactive"` only for a verified,
eligible native event while those issuance gates permit it. `edit` remains false.
The existing `CMAEG_WHATSAPP_INTERACTIONS_ENABLED` controls relay issuance only.

Gate-off preserves ordinary numbered-text rendering and already-issued native
callbacks. It does not make older readers safe rollback targets: cards last up to
24 hours, and admitted callback plans retain their normal recovery lifetime.
Callbacks do not depend on either issuance gate. Provider removal, pending state,
rebinding, or native source removal/revision changes revoke their authority;
credential rotation retains it when the original source and provider still match.
The source revision covers the complete configured native source, including its
admission policy. It excludes secrets: rotating valid Meta or provider credentials
does not revoke issued cards. Each callback must nevertheless authenticate under
the current app secret; a signature from the retired secret is refused.

## Supported public message subset

Use one `buttons` block with at most three options, or one `choice` with
`multi: false` and at most ten options. Top-level text and `text` blocks form a
body of at most 1,024 characters. Button labels are at most 20 characters and list
row labels at most 24, in addition to the public 64-byte option-label limit.
Labels must be unique, nonempty, unpadded and free of control characters.
Styles and selected appearance use the existing declared `plain_option_style`
degradation. The renderer never truncates labels or silently drops controls.

Mixed controls, multiple control blocks, multi-select, media, templates, forms,
links, arbitrary `reply_to`, thread replies and hand-offs are refused on the
native-control path. The event-bound private recipient and original message
context come only from the sealed event and currently authorized native source.
The existing ordinary text fallback remains when native issuance is disabled.
Service-window restrictions remain Meta's responsibility: these are service
messages, not approved templates for use outside the 24-hour customer window.

## Send and callback authority

The exact native app, WhatsApp Business Account and phone resource IDs must match
the current source, the sealed reply address and verified provider Activity.
The original actor is a numeric account equal to the private recipient/chat,
with no thread and `ActorKind::Unknown`; Cloud API does not establish human control.
The encrypted card retains its original event, channel record, source revision,
provider binding, actor, and `DispatchOwner` for both default and named consumers.
That owner includes the original partition and provider conversation.

Card persistence precedes the only Graph send attempt. Issued IDs are bounded
`cw1b`/`cw1l` record/revision/action locators. A successful response must confirm one
message ID for the exact recipient. CMA persists its native send receipt before
binding the card to that message. Unconfirmed acceptance or binding returns an
unknown delivery result without a usable caller reference or automatic resend.
An unbound card cannot consume a callback; pending binding and storage contention
remain retryable.
When a binding write succeeds but its acknowledgement is lost, exact read-back
of the durable card can prove confirmation. If that read is also unavailable,
delivery remains unknown to the caller. A later callback can proceed only if the
exact durable binding is then readable and current custody still matches; an
unknown caller result does not erase a binding already committed by storage.

Meta's HMAC is verified over the original complete batch bytes before extraction.
A callback must match the configured account and phone, raw sender, original
private actor/chat, exact issued control kind/revision, and `context.id` equal to
the confirmed native message. `context.from` is a phone address and is not compared
to the phone-number resource ID. Foreign/malformed control IDs are explicitly
refused. Callback titles never supply action values; `block_id`, `action_id` and
`values.selected` are recovered from the stored public message.

Consumption uses the existing compare-and-swap and same-click replay semantics.
The stored timestamp and reference remain identical on replay, including a
rebatched callback. The first signed batch is retained exactly. New clicks on
consumed, expired or superseded cards are refused. Callback admission commits a
single target from the saved owner and does not fan out through current routing;
partition or default-target changes do not move it to another conversation.

Official contracts: [reply buttons](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/interactive-reply-buttons-messages),
[single-choice lists](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/interactive-list-messages),
and [service-window rules](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/send-messages).
The public guides are rolling documentation and may show newer Graph versions;
this adapter retains the repository's v25.0 transport pin.
