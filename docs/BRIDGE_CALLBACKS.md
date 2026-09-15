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

`execution_hold` is persisted with the admitted inbox record. It states that this release has not dispatched the message to an Agent worker. A future worker must acquire exact current invoke/allocation authority and advance the durable state through its own trusted store transition. An absent hold on an older retained receipt does not establish execution or delivery.

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

Selected-bot mention/reply identity is currently unavailable from the verified connector binding. Group policies requiring that fact return `addressing_evidence_unavailable`. Deliberate owner-open group admission and matching sender/chat allowlists work independently. Stable thread policies require actual typed platform thread evidence; raw Discord interaction routing values are never stable thread IDs.

Accepted reply tokens and Discord interaction context remain encrypted in separate expiring custody. They do not appear in inbox/receipt JSON, keys, logs or UI. Group context partitioning does not change the audience of a reply. Attachment retrieval, outbound dispatch and live Telegram/Lark parity remain separate implementation and verification work.
