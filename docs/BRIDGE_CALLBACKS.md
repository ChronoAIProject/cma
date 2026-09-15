# NyxID bridge callback contract

`POST /api/v1/bridge-callbacks/{bridge_id}` accepts a signed NyxID callback into CMA's durable inbox. This connector endpoint uses callback authentication and is mounted separately from browser/session management routes. Its callback token cannot authenticate ordinary CMA REST operations. The connector protocol is documented here separately from the management OpenAPI inventory.

The bridge must already be enabled with a matching owner, selected existing bot/key/routes, verified encrypted key custody and valid current target/binding. Its manager configuration supplies the exact callback URL; the request body cannot select an owner, Agent or provisioning profile. A deployment without the supported configured HTTPS issuer/callback setup or optional public JWKS origin exposes an unavailable callback route and continues serving its existing sessions.

## Request authentication and bounds

Send the exact JSON bytes signed by NyxID with these single-valued headers:

| Header | Requirement |
| --- | --- |
| `Content-Type` | Exactly `application/json` |
| `X-NyxID-Callback-Token` | RS256 callback JWT; at most 16,384 bytes |
| `X-NyxID-Message-Id` | Matches the signed claim and JSON message ID; at most 256 bytes |
| `X-NyxID-Platform` | Matches the bridge, signed claim and JSON platform; at most 256 bytes |
| `X-NyxID-Timestamp` | Valid provider timestamp matching the signed issued-at time within the configured skew; at most 64 bytes |
| `X-NyxID-User-Token` | Optional separately verified relay token; at most 16,384 bytes |

The callback body is capped at 1 MiB while streaming and must arrive within five seconds. A smaller bridge-specific payload bound also applies. Duplicate sensitive headers and declared oversized bodies fail before body reading or live provider/custody work. At most 16 callback requests are admitted to the body-reading pool at once. CMA verifies exact-body SHA-256, pinned issuer/JWKS, fixed callback audience `channel-relay/callback`, token type `relay_callback`, RSA algorithm/key, time bounds and exact key/message/platform/route binding before live custody validation. JWT `jku`/`x5u` cannot choose a key endpoint.

Current deployment policy uses the configured NyxID provider base URL as issuer and relay audience, and the configured public NyxID origin's `/jwks.json` as the pinned key endpoint. Callback/relay TTL is 300 seconds, reply TTL is at most 1,800 seconds, and clock skew is 60 seconds. An upstream deployment with another JWT issuer requires an explicitly compatible deployment policy; the callback never learns its issuer from token claims.

The optional relay token corroborates the retained owner/key only. Its absence permits inbox-only acceptance when the independently verified binding is valid. It never creates a browser principal, renewable Sandbox authority or an Agent invoke grant.

## Durable acknowledgment

CMA returns `202` only after the store confirms a majority-durable matching inbox receipt. No provisioning, Agent turn, reply delivery or worker invocation occurs in the HTTP request. A typical admitted receipt is:

```json
{
  "status": "inbox_persisted",
  "receipt": {
    "inbox_id": "<64 lowercase hexadecimal characters>",
    "bridge_id": "brg_example",
    "message_id": "nyx-message-id",
    "accepted_at": 1790000000,
    "admission": "accepted",
    "execution_hold": "worker_unavailable"
  }
}
```

`execution_hold` is persisted with the admitted inbox record. It states that the callback request has not dispatched the message to an Agent worker. The background inbox/allocation workers acquire exact current invoke/allocation authority, create or reuse the pinned placement, and admit the input through the common Agent command ledger. The ordinary command worker releases an eligible hold only after a fresh current-authority check. The original callback receipt remains immutable; its hold does not describe the command's later execution status. An absent hold on an older retained receipt does not establish execution or delivery.

Accepted commands retain their authenticated source separately from expiring inbox records, so inbox TTL cleanup cannot erase an already admitted turn's identity or reply correlation. A durably revoked invoke grant or an expired queue deadline can terminally refuse an exact unleased command and release queue capacity. Suspended work remains recoverable; transient authority outages do not terminally refuse it.

A policy refusal also receives `202` after durable recording, with a typed admission such as:

```json
{
  "status": "inbox_persisted",
  "receipt": {
    "inbox_id": "<64 lowercase hexadecimal characters>",
    "bridge_id": "brg_example",
    "message_id": "nyx-message-id",
    "accepted_at": 1790000000,
    "admission": {
      "partition_denied": { "reason": "bot_sender" }
    }
  }
}
```

Denied records retain no execution hold or reply credentials. Other typed reasons include `unknown_sender`, `unknown_event`, `unsupported_conversation`, `sender_denied`, `chat_denied`, `group_denied`, `addressing_evidence_unavailable`, `message_edit`, `interactive_action_unsupported`, `callback_loop`, and missing/unsupported thread evidence. The historical `sender_denied` string remains readable.

An authenticated duplicate with the same verified business identity and exact body returns the original retained receipt, including its original admission and hold. It does not indicate a second acceptance. Same identity with different content returns `409`. A valid retry after an unknown commit can reconcile a committed receipt. Expired or otherwise invalid callback tokens cannot retrieve a retained receipt.

## Failure responses

Failures contain a safe `code` and `acceptance` value, with no raw body, token or private platform metadata:

```json
{"code":"callback_acceptance_unknown","acceptance":"unknown"}
```

| HTTP status | Codes | Meaning |
| --- | --- | --- |
| 400 | `callback_payload_invalid` | Invalid bounded callback payload or inconsistent normalized/raw source |
| 401 | `callback_authentication_failed` | Missing, ambiguous or invalid callback authentication |
| 408 | `callback_body_timeout` | Body did not finish within the read deadline |
| 409 | `callback_replay_conflict` | Previously accepted identity has a different body |
| 413 | `callback_bounds_exceeded` | Payload or supported field bounds exceeded |
| 415 | `callback_json_required` | Unsupported content type |
| 429 | `callback_capacity_exceeded` | Callback concurrency, queue or rate capacity exceeded |
| 503 | `callback_ingress_unavailable`, `callback_keys_unavailable`, `callback_binding_unavailable` | Required configured ingress, pinned keys or current binding unavailable |
| 503 | `callback_acceptance_unknown` | Store/timeout result cannot establish whether acceptance committed |

Responses use `Cache-Control: no-store`; `429` and `503` also carry `Retry-After: 1`. Only the unknown-acceptance failure uses `acceptance: "unknown"`; the other failures use `not_accepted` for the submitted request. A conflict does not erase the earlier accepted record.

Pinned NyxID retains message metadata but does not provide generic callback-body redelivery after callback failure. Loss or uncertainty before durable CMA acceptance is distinct from recovery of an accepted inbox. A provider may mint another NyxID message ID on redelivery. Stored provider message references support later scoped duplicate handling but do not provide an end-to-end exactly-once/no-loss guarantee here.

## Platform facts and reply custody

Signed raw metadata establishes actual chat/sender/conversation facts before it is discarded. NyxID route IDs can be shared by many chats; the actual raw conversation type corrects route-level metadata. Normalized keys exclude display names, message bodies, config revisions, generic thread context and credentials.

Telegram requires explicit human sender evidence and an original text message. Lark/Feishu requires the version 2 message-receive event, a user sender and matching decoded text. Discord requires an explicit create dispatch, human flag and supported actual channel evidence. Missing evidence fails closed. Edits, interactive actions, unknown/nontext events, platform bots/system events and callback loops do not start turns.

Group `reply_to_bot` and `mention_or_reply_to_bot` admission can prove a reply to a prior CMA message when the signed provider parent reference exactly matches a permanent **Sent** outbox receipt for the same issuer, owner, Bridge, selected bot, platform and actual chat. Another Bridge using the same bot does not qualify. The raw documented parent field must match the signed normalized parent: Telegram also requires the parent message's chat, Discord requires an ordinary same-channel reply reference rather than a forwarded reference, and Lark/Feishu requires `event.message.parent_id`. This proof changes only reply addressing; sender, chat, event, loop and current authority checks still apply.

Pending, Claimed, Sending, Unknown, missing and foreign-scope messages do not prove a reply to the selected bot. Messages sent outside CMA or lacking a confirmed platform message ID do not qualify. A matching corrupt record or unavailable lookup returns unavailable rather than persisting a new denial. The inbox stores the confirmed item ID, revision and full-record digest, and revalidates and write-fences that exact Sent row in the fresh acceptance transaction. Removing or substituting the evidence between lookup and acceptance prevents all fresh acceptance writes. An item ID alone is never admission authority. Older stored observations without a reply receipt remain readable.

An exact existing callback receipt retains its original admission, hold and deadline if reply evidence later disappears or lookup fails; current signature, Bridge and custody checks still apply. Neither delivery recovery nor a later confirmed send upgrades a previously denied callback on replay.

Selected-bot **mention** identity remains unavailable from the pinned NyxID callback contract. Mention-only policies still return `addressing_evidence_unavailable`. Textual `@names`, generic bot flags and application hints cannot identify the selected bot. Deliberate owner-open group admission and matching sender/chat allowlists work independently. Stable thread policies require actual typed platform thread evidence; raw Discord interaction routing values are never stable thread IDs.

This is a CMA source-contract implementation against pinned NyxID commit `85607255ee0beadca460a350e0aa6d177edd2fd2`, with signed fixtures for Telegram, Discord, Lark and Feishu and disposable Mongo tests. It does not establish live platform parity. It adds the `bridge_sent_reply_addressing_v1` index over existing permanent outbox fields; it requires no provider resource mutation or data backfill.

Accepted reply tokens and Discord interaction context remain encrypted in separate expiring custody. They do not appear in inbox/receipt JSON, keys, logs or UI. Group context partitioning does not change the audience of a reply. Channel-created sessions can call the [reply tools](CHANNEL_REPLY_TOOLS.md) multiple times during an active turn; the durable outbox sends each admitted message through NyxID to its original recipient. Final answers are not automatically forwarded. Attachment retrieval and live Telegram/Lark parity remain separate implementation and verification work.
