# Native Discord upload message effects

A confirmed native Discord upload can return the existing opaque `message_ref`
for deletion and replacement of the sending bot's own reaction. This is authority
over one exact Discord message, including all its attachments. It does not grant
content editing, attachment replacement/removal, or independent file deletion.
REST, MCP and SDK request/response types remain unchanged.

## Issuance and rollout

`CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED` defaults to `false`; the checked-in
ConfigMap keeps it `false`. It requires
`CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED=true`. Text edit issuance is
independent: `CMAEG_NATIVE_DISCORD_EDITS_ENABLED` can remain off. Event context
can advertise message effects while `capabilities.edit` remains false. Clients
must retain the actual confirmed reference and query its effect capabilities.

Enable media effect issuance only after every serving and retained rollback
reader supports `discord_message.effects_only`. Older destination readers reject
unknown fields. Existing text destinations omit this field, preserving their
serialized form and durable effect-receipt hashes. New media destinations set it
to `true` and require an effect window of at most 24 hours. No new record kind,
public request field, or arbitrary platform-ID input is added.

Turning either effect issuance flag off stops new media references. Retained
references still authorize effects until expiry, subject to current authority.
Rollback must retain a reader that understands the new scope while these
references and their persisted receipts remain retained; disabling issuance is
not a migration for old readers. Enabling text edits later cannot widen the
scope of a previously issued media reference. `updateMessage` returns
`unsupported` without a Discord PATCH for this scope, even after effect expiry.

## Exact target and limits

Issuance requires a valid ordinary native message with one to four prepared
`native_attachments`, optionally accompanied by text/link content. All files
must be in the same Discord POST and the whole delivery must be confirmed as
one Discord message. Attachment-only messages qualify. The returned message ID
must be canonical, with the requested native channel and the authenticated bot
as author. The existing native upload preparation, byte limits, route pacing
and transport deadlines remain in effect.

Long content can render into several Discord messages. Files go on the first
message, but the public reference describes only one target. Such delivery gets
no media effect reference, even when every part succeeds. Partial sends,
unreadable or uncorrelated responses, response loss, and rejected sends also
get no reference. No reference is reconstructed from a guessed ID, a filename,
an attachment URL or a later search. Cards, degraded controls, relay uploads
and handoff lineage do not receive this ordinary media authority. Incoming
reaction-event custody and its separate feature flag remain separate contracts.

Delete removes the entire exact bot-authored message. Reactions affect only the
bot's own ordinary reactions: zero or one Unicode/custom emoji, with individual
`/@me` removals and additions. Other users' reactions are preserved. Custom emoji
IDs resolve in the destination guild. Current Discord permissions and message
existence still determine whether an operation can succeed.

The reference remains bound to the message if its attachments later change or
are removed through another authorized client. There is no promise about a
particular attachment's continued presence or CDN availability. A missing
message is refused by the existing current-message read; an earlier confirmed
effect with the same idempotency key still replays its durable receipt.

## Current authority and uncertain writes

Media references reuse the text-message authority binding: current CMA channel
record, owner, exact Discord source/policy, consumer, provider binding and
consumer key. Changes revoke authority. A replacement bot token is accepted only
if it still authenticates as the original bot. Each effect re-resolves current
authority after acquiring the existing exact-message mutation fence, then reads
the exact Discord message and verifies its author before a write.

The existing durable effect claim, idempotency key and mutation fence apply
unchanged. Confirmed/rejected receipts survive process restart. Unknown writes,
abandoned claims and lost receipt writes remain fenced through reference expiry;
another key cannot retry a possibly accepted mutation. The transport makes no
automatic retry and uses its existing five-second effect deadline. A partial
reaction replacement remains `unknown` if a later write fails.

## Official Discord evidence

Read on 2026-10-27 from `discord/discord-api-docs` commit
`6128d99116af8357db1e4966ebd7f099817a0e04`:

| Official section | Contract used here |
|---|---|
| [Create Message](https://github.com/discord/discord-api-docs/blob/6128d99116af8357db1e4966ebd7f099817a0e04/developers/resources/message.mdx#create-message) | File upload uses multipart form data and returns a Message object. The ID identifies the entire uploaded message. |
| [Uploading Files / Editing Message Attachments](https://github.com/discord/discord-api-docs/blob/6128d99116af8357db1e4966ebd7f099817a0e04/developers/reference.mdx#uploading-files) | Indexed `files[n]` belong to one message. Editing attachments must account for retained and new files; omitted existing files in the attachment list are removed. |
| [Edit Message](https://github.com/discord/discord-api-docs/blob/6128d99116af8357db1e4966ebd7f099817a0e04/developers/resources/message.mdx#edit-message) | Removing/replacing attachments uses the `attachments` field, and API v10 requires the complete retained/new list. This implementation issues no edit authority for uploads. |
| [Delete Message](https://github.com/discord/discord-api-docs/blob/6128d99116af8357db1e4966ebd7f099817a0e04/developers/resources/message.mdx#delete-message) | Deletes the exact message and returns 204. This implementation additionally requires the current bot to own it. |
| [Create Reaction](https://github.com/discord/discord-api-docs/blob/6128d99116af8357db1e4966ebd7f099817a0e04/developers/resources/message.mdx#create-reaction) and [Delete Own Reaction](https://github.com/discord/discord-api-docs/blob/6128d99116af8357db1e4966ebd7f099817a0e04/developers/resources/message.mdx#delete-own-reaction) | PUT/DELETE use exact message IDs and `reactions/{emoji}/@me`, return 204, and remain subject to permissions. |

Downloaded UTF-8 evidence SHA-256:

- `developers/resources/message.mdx`: `c296fe9263b887f0f3cf840927466826cd4e5c9182aebe6613decc28e5d80611`
- `developers/reference.mdx`: `b0ac645b5e85d1d05fbf07b7e78a25e4e22987560e05e6b0c8d238b6a1e0675d`

Validation uses local HTTP fixtures and sealed-reference tests. It covers
attachment-only and four-file sends, gate combinations, refusal of multi-message
or uncertain sends, edit refusal, exact effect endpoints, own-reaction scope,
current authority revocation, expiry, durable replay and uncertain-write fences.
No real Discord send, live configuration change or deployment is part of this
change. Multi-message media effects and attachment editing remain unavailable.
