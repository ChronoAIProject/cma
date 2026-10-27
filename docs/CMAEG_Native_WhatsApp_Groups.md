# Native WhatsApp existing-group replies

The optional `group_ids` allowlist on a native `whatsapp` source opts that source
into specific existing Meta groups. Absent or empty `group_ids` preserves the
private-only wire shape and behavior. The maximum is 16 unique opaque group IDs,
512 ASCII bytes each (letters, digits, `_`, `-`, `+`, `/`, `=`); CMA preserves IDs
without interpreting or decoding them. The Web producer editor exposes a
structured list only when group authoring is advertised or saved groups exist.

```json
{
  "type": "whatsapp",
  "app_id": "12345",
  "waba_id": "23456",
  "phone_number_id": "34567",
  "group_ids": ["opaqueMetaGroupId"],
  "admission": {
    "type": "scoped",
    "senders": { "type": "allowlist", "ids": ["447700900123"] },
    "chats": { "type": "allowlist", "ids": ["opaqueMetaGroupId"] },
    "groups": "all"
  }
}
```

The source allowlist and admission policy both apply. Sender rules use the
original `messages[].from` participant. Chat rules use `messages[].group_id`;
conversation kind is `group`. CMA never treats the participant's private chat as
the group destination. Actor kind remains `unknown`. Group mentions and replies
to the bot are not inferred; scoped group policy should be `all` or `deny`.
Thread partitions remain unsupported. Existing default/named consumer routing and
unknown-actor policy apply.

## Eligibility and current official contract

Meta's [Groups overview](https://developers.facebook.com/documentation/business-messaging/whatsapp/groups/),
updated **15 October 2026**, says groups are open to all businesses with an
**Official Business Account (OBA)**. The exact sending phone must be approved;
a verified business portfolio alone does not establish phone OBA status. Current
[OBA guidance](https://developers.facebook.com/documentation/business-messaging/whatsapp/official-business-accounts/)
also lists 30 days registered, two-step verification and an approved display
name, with Meta approval required. The current Groups pages do not impose the
older 100,000-message-tier eligibility condition. Test accounts and WhatsApp
Business app accounts are excluded. Groups are unavailable for Coexistence and
Multi-solution Conversations.

The documented limits are eight participants per group, 10,000 groups per
business phone and one Cloud API business per group. The current group guide
supports text, media and text/media templates; it excludes interactive,
authentication, commerce, view-once, disappearing messages, calling, message
editing and message deletion. CMA's subset is text, JPEG/PNG images, PDF, the
existing bounded audio/video profiles and approved templates with header/body
components. Stickers, reaction updates, controls and URL-button templates are
not enabled for groups by this implementation.

The [Groups changelog](https://developers.facebook.com/documentation/business-messaging/whatsapp/groups/)
introduced the API on 6 October 2025. Graph v25 was released on 18 February 2026
and is supported through 29 July 2028. Current examples use v26; the reviewed
pages do not establish a v26-only gate. CMA retains its pinned **v25.0** transport.
This audit establishes documented endpoint and payload shapes, not connected
v25 acceptance under a real OBA phone.

## Separate policy gates and custody

Both new flags default to false in code and the source ConfigMap inventory:

- `CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED` permits new owner group opt-ins.
  Discovery reports `relay_authoring.native_whatsapp_groups_writes_enabled` only
  with native authoring, durable admission/recovery and routing authoring support.
  Missing capability fields do not grant authoring. Existing exact groups can be
  retained or removed with this and the native authoring gate disabled.
- `CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED` permits new group admission and group
  sends. Turning it off preserves reads/replays of already admitted work but
  refuses new effects. It does not turn on native media or templates.

New or changed source/credential authoring first proves the exact app, WABA and
phone using the existing credential checks, then performs read-only group proof:

1. `GET /v25.0/{phone}?fields=official_business_account` must return that exact
   phone ID and `official_business_account.oba_status: "APPROVED"`.
2. `GET /v25.0/{phone}/groups?limit=256` must contain every configured group in
   `data.groups`. At most four pages are read. Pagination uses a bounded opaque
   `after` cursor on CMA's fixed endpoint, never a provider-returned URL. A group
   absent from the first 1,024 inspected entries remains unproved and is refused.
3. `GET /v25.0/{encoded-group}?fields=suspended,participants,total_participant_count`
   must return that exact group, product `whatsapp`, typed `suspended: false`, a
   nonempty bounded unique numeric participant list and the matching count.

This read sequence has a 12-second total bound. It runs again before **each**
native effect, including media upload and the subsequent recipient send. Send
proof additionally requires the original participant to remain a member. Current
source/group allowlist, credential revision and original consumer/provider/key
custody are checked again by the routed fence after those reads and before the
write. Membership or authority loss stops the effect. This is a bounded snapshot
proof; external membership changes cannot be made atomic with Meta's send API.
Removing allowed groups requires no provider availability. Credential changes or
other source changes still require verification. No group management mutations
are performed.

## Sealed destination, confirmation and uncertainty

Signed ingress retains the exact group, participant and original message in a
separate sealed `whatsapp_group` reply address. A source/credential revision digest
in that address revokes retained and delayed group work after credential rotation. Media access requires those same
facts, the current source allowlist and the original signed attachment locator.
Private reply-address validation remains numeric and unchanged.

Native replies post to `/{phone}/messages` with `recipient_type: "group"`,
`to: <exact group ID>` and the original context message. A usable acceptance
requires the `whatsapp` product marker, exactly one valid `wamid` and the exact
`messages[0].group_id`. Contact/participant IDs alone cannot confirm a group send.
The [general send guide](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/send-messages/)
documents that group response field, while the generated Message API schema
omits it. This discrepancy remains an explicit connected-acceptance requirement;
CMA fails uncertain rather than weakening destination confirmation.

Each confirmed message has a durable `native_whatsapp_group_receipt_v1` record
with its group/original participant, original admission, partition, conversation,
consumer and provider. The storage namespace is separate from private reaction
receipts. A group receipt grants no edit/delete/reaction authority. Send ambiguity,
receipt loss and partial sends remain unknown and retained retries do not resend.
A Graph success confirms acceptance only, not recipient delivery or reading.

## Media and templates

Inbound group media additionally requires `CMAEG_MEDIA_ENABLED` and
`CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED`. Outbound group media additionally requires
`CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED`. Context discovery advertises
`image`, `file`, `audio`, `video`, the existing MIME/profile limits and no sticker
or `image/webp` capability. Existing aggregate, caption and transport bounds apply.

Templates additionally require `CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED`.
Eligible group contexts advertise `whatsapp_templates: true` with additive
`whatsapp_template_components: ["header", "body"]`. Approved text/image/video/
document headers and text/currency/date-time body substitutions retain the typed
contract; button components are refused before any provider call. Media IDs and
public HTTPS references retain the existing template policy. Meta remains
responsible for approval, language and placeholder compatibility.

## Validation and acceptance limits

Local signed-webhook, fake-Graph, record-store and routed-engine fixtures cover
source opt-in, separate participant/group policy, default/named consumer custody,
exact payloads, read-only eligibility proof, pagination confinement, group loss,
unsupported combinations, durable receipts, uncertain outcomes and replay.
They do not establish connected Meta acceptance. No live platform messages, app
settings, deployed infrastructure or NyxID changes are part of this work.
Group creation/deletion, invite links and member management are outside scope.
Outbound message edits/deletes remain unavailable: incoming edit/revoke events
are documented only for Coexistence and do not establish an outbound API.
