# Native Lark / Feishu separate-app source

The `native_lark` source receives ordinary text, independently enabled [inbound media](CMAEG_Native_Lark_Media.md), recall notifications and human reaction add/remove notifications directly from a separately provisioned, single-tenant custom app. It has its own encrypted credentials, callback URL and native text/media reply path. Existing NyxID sources and their webhooks are independent.

Both `CMAEG_NATIVE_LARK_WRITES_ENABLED` and `CMAEG_NATIVE_LARK_INTAKE_ENABLED` default to `false` in code. The deployment manifest enables owner setup and testing after the accepted PR #1157 compatible-reader rollout; connected native acceptance remains unverified. See the [activation record](CMAEG_Operations.md#native-lark-and-whatsapp-owner-testing-activation-2026-10-28). The source also requires durable routed admission/recovery, routing authoring and providers configured for response replay. Local validation is not live platform acceptance.

## Provision a separate app

Create a distinct custom app in the chosen region, enable its bot, publish/install it in the intended tenant, and add that bot to the conversations it should receive. Do not replace the webhook of an existing NyxID app: Lark/Feishu permits one request URL per app, and changing it redirects every subscribed event. A separate app does not inherit private chats, app-scoped `open_id` values, credentials or routing authority from another app.

The management source shape is:

```json
{
  "type": "native_lark",
  "separate_app": true,
  "identity": {
    "region": "feishu",
    "app_id": "cli_separate_app",
    "tenant_key": "exact_tenant_key",
    "bot_open_id": "ou_separate_bot"
  },
  "admission": { "type": "allowlist", "sender_ids": ["ou_allowed_human"] }
}
```

Use `region: "lark"` for the international API. App identities, tenant keys and open IDs must come from that region and that app. Source authoring exchanges the app ID/secret at the region's custom-app token endpoint and independently verifies the bot `open_id` at `/open-apis/bot/v3/info`. The receiving tenant is checked against every authenticated event header. The first increment refuses bot/app and anonymous message/reaction actors and cross-tenant senders.

Supply an atomic `credentials.native_lark` object containing `app_secret`, `encrypt_key` and `verification_token`. All three are mandatory. Credentials are stored only inside the sealed channel record and are omitted from public definitions, activities and provider requests. A stable canonical UUID v4 `record_id` is required when creating the channel, so creation can recover after an uncertain response. The API returns `endpoints.native_lark_webhook_url`; configure that URL only on this separate app. The console source details and source diagnostics repeat this provisioning requirement. There is no automatic platform subscription or webhook mutation.

Configure the required Encrypt Key and Verification Token on the app, then subscribe to:

- `im.message.receive_v1` for text and enabled media intake.
- `im.message.recalled_v1` for recall notifications.
- `im.message.reaction.created_v1` and `im.message.reaction.deleted_v1` for human reaction deltas.

Grant the documented receive/recall and reaction-read permissions for the conversations the bot needs, plus the send-message permission for native replies. The audit in [incoming message events](CMAEG_Lark_Incoming_Message_Events.md) records the official permissions and evidence. The bot receives reactions only in conversations it belongs to. App publication, installation, membership, effective permission approval and actual event delivery require platform acceptance with the separately provisioned app.

## Authentication and durable admission

`POST /v1/native-lark/{channel_id}` requires `application/json`, `x-lark-request-timestamp`, `x-lark-request-nonce`, and `x-lark-signature`. The gateway verifies SHA-256 of the exact timestamp, nonce, Encrypt Key and encrypted request bytes before AES-256-CBC/PKCS#7 decryption. It bounds the body, requires a timestamp within five minutes, checks the Verification Token, then checks schema 2.0, app ID and tenant key. The encrypted URL-verification challenge is authenticated separately; a challenge grants no message authority.

Only verified bounded message facts are retained; the verification token is removed before normalization. The endpoint returns successful acceptance only after durable routing admission. A 2.5-second local deadline leaves room within the documented three-second platform acknowledgment window. Unavailable storage or paused durable admission returns HTTP 503. Platform retries and duplicate delivery are expected; successful delivery can also repeat.

An original message is keyed by region, app, tenant, bot and platform `message_id`. Duplicate receive envelopes reuse the committed selection. Different text, media resource keys or sender/chat facts under that key conflict; the gateway does not convert them into an edit. Notifications use their provider event ID, retain the original conversation and consumer targets, and never rerun current addressing rules to expand that selection. Current source, credential revision, provider and consumer authority are rechecked before execution and sending. Removing the source, rotating any of its credentials, changing its admission policy, or removing the selected consumer revokes retained work.

Recall notifications become `cma.message.deleted.v1` with `recall_type` and an **unknown actor**, because the platform does not provide the deleting operator ID. Reactions become `cma.message.reaction.updated.v1` with `operation: "add" | "remove"` and `reaction.emoji_type`. They are deltas, with no inferred previous state, complete replacement set, clear-all operation or provider reaction ID. These events confer no delete/edit/reaction mutation authority.

Unknown targets receive HTTP 503 for a bounded two-minute window after the event timestamp, allowing an original admission or confirmed outbound receipt to settle. Older unavailable targets are acknowledged as refused. Original plans and confirmed targets have bounded retention; this is not a historical message archive. Event arrival order is not assumed.

## Native replies and confirmed outbound targets

Text replies use the exact retained app and chat through the region's official API. Non-thread replies send to the retained chat, so a recalled original can still receive a conversational response. A thread reply uses the retained message with `reply_in_thread`; failure to address that thread is reported and never falls back to another conversation. Native thread contracts, history tools and thread creation are not advertised.

The sender checks current source/credential authority before each send, respects the shared delivery gate and pacing, and requires a successful official receipt with an exact chat and new message ID. It then seals a receipt bound to the original plan, sending consumer, provider, partition, conversation, app identity and credential revision. Reactions or recalls on a confirmed outbound target reach only its original sending consumer. Missing or malformed send responses remain uncertain and are never blindly retried. Partial multipart delivery and uncertain receipt persistence are reported as unknown.

The regional endpoints follow the official [v3 custom-app token](https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal.md), [create-message](https://open.feishu.cn/document/server-docs/im-v1/message/create.md), and [reply-message](https://open.feishu.cn/document/server-docs/im-v1/message/reply.md) contracts. The platform marks this v3 token endpoint as historical; its newer OAuth token endpoint has a different request/response contract and is not used by this increment. Text is rendered into chunks of at most 6,000 characters; the client additionally bounds UTF-8 text at 28 KiB and the full serialized request at 150 KiB. Redirects are disabled and response reads are bounded.

The native source supports text and independently gated [bounded native outbound media](CMAEG_Native_Lark_Outbound_Media.md). Inbound image/file/audio/video and rich-post resources use the independent [native media gate](CMAEG_Native_Lark_Media.md) and existing attachment read pipeline. [Ordinary native message effects](CMAEG_Native_Lark_Message_Effects.md) independently enable confirmed text edits, recalls, own reactions and reactions to retained received human messages. Cards/forms, cross-app adoption and incoming edit propagation remain outside this increment. Native output attachments are advertised only under their independent default-off gate; controls remain unsupported. No verified incoming edit delivery/revision contract has been established; `update_time` alone is insufficient.
