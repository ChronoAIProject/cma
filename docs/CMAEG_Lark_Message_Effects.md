# CMA Lark/Feishu message effects contract audit

Pinned NyxID: `2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f`.
Pinned official Lark SDK: `larksuite/oapi-sdk-go` `db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa`.

NyxID `backend/src/routes.rs:133` registers ANY `/api/v1/proxy/{service_id}/{*path}`. `backend/src/handlers/proxy.rs:819` requires REST proxy access; lines 897–981 enforce scope/operation preflight and exact `_nyxid_via` UserService selection constrained to the catalog ID in the route. The exact-selection path fails rather than falling back when the connection does not resolve. `backend/src/services/provider_service.rs:5005` and the corresponding Feishu seed define official regional `api-lark-bot` and `api-feishu-bot` services with server-side tenant token exchange. The service description explicitly allows arbitrary official open API paths. The typed Lark bot OpenAPI does not enumerate deletion/reactions; availability here follows the generic proxy, not a claimed channel-relay extension. No upstream change is needed.

Official `service/im/v1/resource.go:1220` defines tenant-token-capable DELETE `/open-apis/im/v1/messages/:message_id`. Lines 1642 and 1670 define POST `/open-apis/im/v1/messages/:message_id/reactions` and DELETE `/open-apis/im/v1/messages/:message_id/reactions/:reaction_id`. `model.go:15774` uses `reaction_type: {emoji_type}` and returns a reaction ID, operator and reaction type. Deletion documentation restricts reaction removal to the calling user's or bot's own reaction. Message recall remains subject to app permissions, conversation membership, and tenant recall age policy.

CMA implementation uses separate `lark_message_effects` source configuration containing an exact bot-service connection and independent `delete`/`reaction` booleans. The read-only `lark_resources` setting never grants mutation authority. New shape enrollment defaults off via `CMAEG_LARK_MESSAGE_EFFECTS_WRITES_ENABLED`. The same flag controls advertisement of Lark effect tools in MCP and the owner tool catalog; constructing the retained client alone does not advertise them. Existing saved shape readers/effects are retained independently of enrollment, and disabling discovery does not revoke an already authorized effect reference. The source manifest declares the enrollment flag as false; no live configuration has been changed.

Only exact confirmed CMA reply receipts receive an authority token, using the original verified activity, channel/record/owner/consumer/provider/key/source/routing/credential revision, bot and conversation. Plain text must be a single bounded platform message; cards must already have durable binding. Threaded contexts are excluded in this first adapter. Owner-initiated messages and arbitrary platform IDs gain no authority. Tokens expire 24h after issuance. Effects re-resolve current custody, independently recheck regional official service metadata and bot identity, and GET the exact platform message to check app sender, chat and thread before mutation.

Reactions use Lark emoji type names such as THUMBSUP. CMA retains the provider's returned reaction ID durably. Replace/clear operates only on that retained CMA-added reaction; it does not list or remove reactions added independently, even by the same bot. Every effect first obtains a durable idempotency claim and a durable per-message fence. Lost/malformed write responses, ambiguous HTTP statuses, partial replacement and failed final persistence remain unknown; no automatic resend and no new-key bypass after uncertainty. The same scope is shared by delete/reaction to reject key reuse for a different action.

This is source and local HTTP contract verification. No live platform send, mutation, deployment, production configuration change, or new serving-version observation occurred.

## Source configuration

After compatible readers are installed, the owner may opt a verified Lark/Feishu source into either operation. Keep the independently pinned `bot_id: ou_...` and the existing channel key with access to this exact connection. The proxy app must have message-read and selected mutation permissions.

```json
{
  "lark_message_effects": {
    "connection": {
      "user_service_id": "connection-one",
      "catalog_service_id": "catalog-one",
      "app_id": "cli_source"
    },
    "delete": true,
    "reaction": true
  }
}
```

Use the existing `readMessageCapabilities`, `deleteMessage` and `setMessageReaction` tools. The REST operation names and request shapes remain unchanged. A reaction request is `{"reaction":[{"type":"emoji","emoji":"THUMBSUP"}]}`; `{"reaction":[]}` clears the retained CMA reaction. `is_big` and custom emoji are refused. Each request requires a stable idempotency key.

Card deletion durably fences fresh callbacks before platform dispatch. A callback consumed before the deletion claim may recover only its exact original callback/event/action/submitted result. Deletion and both text/card edits share the exact-message mutation lock. A known no-write rejection can restore controls; uncertain deletion leaves the marker and lock retained. The platform cannot retract a callback already durably accepted.
