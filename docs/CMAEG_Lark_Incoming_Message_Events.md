# Lark and Feishu incoming message-event contract audit

Reviewed 2026-10-28 from CMA
`149f4247c97ff83a9c29383a4ea6540864510e42` against read-only NyxID source
`2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f` and official Lark SDK
`larksuite/oapi-sdk-go` at `db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa`.

The pinned NyxID relay cannot deliver incoming Lark/Feishu recall or reaction
events to CMA. Its adapter filters them before constructing a callback. A CMA
parser change or activity declaration cannot restore those events. Official
native recall and reaction subscriptions exist, but using them requires a new
ingestion and credential-custody path. An incoming edit-delivery contract remains
unverified; an outgoing edit API and an `update_time` field do not establish one.

This review adds documentation and a bounded implementation plan. It does not
add a runtime parser, source variant, capability, gate, synthetic acceptance
fixture or live subscription. Existing relay behavior is preserved.

## Where the current path stops

All upstream source links below are pinned to the reviewed NyxID commit.

| Stage | Evidence | Consequence |
| --- | --- | --- |
| Native authentication | [`LarkFamilyAdapter` verification](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/lark.rs#L284) verifies the raw-body signature before decryption when Encrypt Key is configured, then checks the Verification Token. Plaintext configuration uses token verification. | Native authenticity is checked upstream. CMA receives the resulting NyxID-signed callback, not the original signature headers or webhook secrets. |
| Event allowlist | [`parse_inbound`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/lark.rs#L1023), specifically lines 1047–1057, accepts only `im.message.receive_v1` and `card.action.trigger`; every other type yields an empty message list. One adapter serves both regions. | Recall and reaction events produce no `InboundMessage`. |
| Dispatch | [`channel_webhooks`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/handlers/channel_webhooks.rs#L725) passes only parsed messages to [`process_inbound_messages`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_inbound_service.rs#L48), which immediately returns for an empty list. | No retained original event, routing, or callback is created for the filtered delivery. |
| Callback raw data | [`build_callback_payload`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_relay_service.rs#L797) copies accepted `inbound.raw_data` into `raw_platform_data` at line 861. | Raw data preserves accepted receive/card events; it cannot recover discarded event types. |
| Activity extension | [`PlatformAdapter`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_platform.rs#L508) defaults to no activity descriptors or metadata; Lark overrides neither. [`declare`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_activity_callback_service.rs#L70) requires a nonempty subset of supported kinds. | Declaring an activity callback is not a bypass. |
| Upstream regression | [`parse_non_message_event_returns_empty`](https://github.com/ChronoAIProject/NyxID/blob/2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f/backend/src/services/channel_adapters/lark.rs#L2077) checks this refusal using a bot-added event. | The general non-message refusal is intentional. This is not a claimed recall-specific test. |

CMA's [relay verifier](../api/crates/cmaeg/src/ingress/relay/verify.rs)
authenticates the callback JWT against the pinned issuer/JWKS, exact body digest,
platform, channel key, route and reply token. Its
[Lark observer](../api/crates/cmaeg/src/ingress/relay/lark.rs) correlates raw chat
and sender identity with the normalized envelope, accepts receive events or
bound card callbacks, and returns `system_event` for other event types.
The [callback schema](../api/crates/cmaeg/src/ingress/relay/payload.rs) ignores the
currently dormant `activity` hint. Trusting normalized text or widening that hint
would add no missing native target, operator or delivery evidence.

For accepted receive events, raw data can include `header.event_id`, `app_id`,
`tenant_key`, sender identity, native message/chat/thread identity, content and
`update_time`. CMA's ordinary observer does not interpret `update_time` as an
edit notification. No verified contract says that each later edit is delivered
again through `im.message.receive_v1`; receive-event documentation explicitly
describes receipt and recommends deduplicating ordinary messages by `message_id`.

## Verified native facts and remaining gaps

The official regional docs were retrieved on 2026-10-28. Corresponding event
models are also present in the pinned official SDK's
[`service/im/v1/model.go`](https://github.com/larksuite/oapi-sdk-go/blob/db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa/service/im/v1/model.go#L16624).

| Operation | Official delivery and facts | Scope limits |
| --- | --- | --- |
| Recall | [`im.message.recalled_v1`](https://open.feishu.cn/document/server-docs/im-v1/message/events/recalled) carries `message_id`, `chat_id`, `recall_time`, and `recall_type`; the header identifies the receiving app, tenant and event. Recall types are `message_owner`, `group_owner`, `group_manager`, and `enterprise_manager`. The [Lark regional page](https://open.larksuite.com/document/server-docs/im-v1/message/events/recalled) documents the same fields. | No operator ID, message body, chat type or thread ID is supplied. Preserve an unknown deleting actor and the stated role; do not identify the operator as the original author. This is a recall event, not proof that every form of deletion emits it. Subscription requires `im:message` or `im:message:readonly`. |
| Reaction added | [`im.message.reaction.created_v1`](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/events/created) carries `message_id`, `reaction_type.emoji_type`, `operator_type`, `user_id` or actor `app_id`, and millisecond `action_time`. The [Lark page](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/events/created) agrees. | This is one add delta. It contains no chat/thread ID, provider reaction ID, prior state or complete reaction set. Bots receive these events only for conversations they belong to. |
| Reaction removed | [`im.message.reaction.deleted_v1`](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/events/deleted) has the same identity shape and a removal action time; the [Lark page](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/events/deleted) agrees. | This is one remove delta, not clear-all or replacement. Both reaction subscriptions require `im:message.reactions:read` or `im:message:readonly`. `operator_type=user` uses user identity, while `operator_type=app` uses the actor app ID. Neither is the receiving `header.app_id`. |
| Edit | [Message receipt](https://open.feishu.cn/document/server-docs/im-v1/message/events/receive) includes `update_time`; [GET message](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/get) returns current `update_time`, `updated`, `deleted`, content and sender. [PUT message](https://open.feishu.cn/document/server-docs/im-v1/message/update) is an outgoing editing API. | The reviewed SDK message-event inventory contains receive, read, recall, reaction-created and reaction-deleted models, with no distinct edited-message model. These facts do not prove a push notification for every edit, its acting user, a previous body or an immutable edit revision. Edit propagation remains outside the proposed first increment. |

The retrieved event-list page is a dynamic `<md-event-list>` placeholder, so it
was not used as an exhaustive event inventory. The edit conclusion is an explicit
evidence gap, not a universal claim that the platform can never notify edits.
Polling current messages would be a separate snapshot-observation contract with
additional permissions, budgets and incomplete history; it must not be presented
as authenticated native edit-event delivery.

User `open_id` is app-scoped. The reaction's `user_id`, `union_id` and `open_id`
cannot be substituted for one another; the broader employee-ID permission is not
needed when an attributable event supplies `open_id`. Native IDs must remain
bound to region, receiving app and tenant. Message IDs are unique within a
tenant, not an authorization token for arbitrary reads or routing.

## Native receiver feasibility without changing NyxID

The official [webhook setup guide](https://open.feishu.cn/document/ukTMukTMukTM/uYDNxYjL2QTM24iN0EjN/event-subscription-configure-/choose-a-subscription-mode/send-notifications-to-developers-server)
states that **each app has one request URL and all its subscribed events go to
that URL**. Adding a second CMA URL to an app already delivering to NyxID is not
an available fan-out mechanism. Switching the URL would move existing traffic.
The [event overview](https://open.feishu.cn/document/ukTMukTMukTM/uUTNz4SN1MjL1UzM)
offers webhook or SDK persistent connection as subscription modes; it does not
establish that a parallel connection can leave the existing webhook unaffected.

The bounded preservation path is a separately provisioned, single-tenant custom
app and a distinct CMA native source. Its bot must be installed in the intended
conversations and receive the required subscriptions and permissions. That app
cannot inherit another app's private chats, app-scoped user IDs, relay reply
tokens or original routing authority. Migrating an existing app would require a
separate, complete receive/reply/card/media continuity design and explicit
subscription cutover; it is excluded from this plan.

CMA currently has a channel key and exact authorized proxy binding for Lark
resource reads/effects. Those capabilities do not expose the upstream webhook
Verification Token or Encrypt Key and do not grant subscription custody.
The new source needs separately supplied encrypted webhook secrets, an exact
region/app/tenant/bot binding, and source revision/revocation checks. The current
[connection authorization](../api/crates/cmaeg/src/media/source/lark.rs) can be
reused for narrowly authorized official API reads through the existing regional
bot-service proxy, including independent bot identity; it cannot authenticate an
incoming webhook on its own or turn an unknown target into an admitted message.

## Bounded implementation sequence

1. **Source and custody foundation.** Add a distinct native source for one
   explicitly enrolled custom app/tenant in one region. Default-off management
   and intake gates must preserve all existing serialized relay definitions.
   Store Verification Token and Encrypt Key only in encrypted custody. For the
   first increment, require Encrypt Key; authenticate the exact raw bytes with
   SHA-256 of timestamp + nonce + Encrypt Key + body, using constant-time
   comparison, then perform bounded base64/AES-256-CBC/PKCS7 decoding and token,
   app and tenant checks. Reject duplicate/malformed signature headers and
   cross-source evidence. Handle the authenticated URL challenge separately.
   The exact security rules come from the official
   [receiving-events guide](https://open.feishu.cn/document/server-docs/event-subscription-guide/event-subscription-configure-/encrypt-key-encryption-configuration-case).

2. **Original-message admission.** Establish bounded ordinary text intake for
   that new native app and persist authenticated native identity before provider
   dispatch. Key the retained index by region, app, tenant and message; retain
   chat/thread, original actor, source/credential revision, admitted event and
   original consumer/provider/conversation/partition selection. Deduplicate
   ordinary receive messages by native message ID, with a digest conflict check.
   Establish native reply routing and confirmed receipts explicitly; a native
   event cannot fabricate a NyxID relay reply token. Keep cards, forms, media and
   cross-app adoption outside this initial source increment.

3. **Recall and human reaction notifications.** Add the three documented event
   types only after original-message retention works. Recall must match the
   retained chat; reaction chat/thread and original routing come exclusively
   from the retained target. Unknown, expired, mismatched or revoked targets
   cannot start a new conversation. Preserve an unknown recall operator. Require
   `operator_type=user` and an exact app-scoped `open_id` for reaction intake in
   the first increment; app/self reactions remain refused to avoid introducing
   automated actor authority. Represent recall as `cma.message.deleted.v1` with
   explicit native recall facts, and reactions as add/remove deltas under
   `cma.message.reaction.updated.v1`. No complete reaction set or provider
   reaction ID is invented, and notifications grant no delete/edit/reaction
   mutation authority.

4. **Durable execution and compatibility.** Reuse
   [VerifiedMessageEvent and retained-plan binding](../api/crates/cmaeg/src/ingress/message_event.rs)
   plus the [durable notification admission path](../api/crates/cmaeg/src/ingress/routed_message_event.rs).
   Retain immutable canonical facts and a digest for each native event ID; keep
   verification secrets out of public activities and source snapshots. Persist
   admission before acknowledging, then dispatch asynchronously. The platform
   expects event completion within three seconds, retries failures after 15 s,
   5 min, 1 h and 6 h, and may duplicate successful deliveries. Do not assume
   chronological delivery or interpret missing deltas as current state. Bounded
   pending-target records may wait for an already-admitted original or in-flight
   confirmed outbound receipt; expiry must refuse rather than choose another
   target. Outbound reactions require an exact confirmed native receipt and
   narrow to its original sending consumer. Owner/source/key/provider/consumer
   revocation, restart recovery, existing retention limits and gate-off replay
   rules must match the other native message-event paths. All serving and
   retained rollback readers must support the new source before enrollment.

5. **Contract checks and controlled acceptance.** Test the actual native webhook
   route using official-shaped encrypted/signed fixtures, independent regional
   fake APIs, durable-store failures and retained original admissions. Cover
   malformed signatures/decryption, app/tenant/region confusion, unknown recall
   actors, app-vs-user reaction actors, missing chat/thread facts, add/remove/add,
   duplicated/conflicting IDs, out-of-order delivery, revocation, consumer
   isolation, receipt races and restart recovery. Verify default-off fixtures,
   historical JSON, OpenAPI and both SDKs. Live permission, subscription, client
   recall/reaction and reply acceptance remain a later explicit platform test.
   A handcrafted NyxID callback is not evidence for this native route.

An edit increment needs an independently verified trigger and revision contract
before parser or routing work. The current GET response can support an explicit
authorized current-state read, but it is insufficient to reconstruct all edits
or attribute the editing operator. No edit-completeness claim follows from this
plan or from existing outbound edit support.

## Evidence and validation

Downloaded official Markdown and SDK source were retained locally with URL,
byte length and SHA-256 in the review evidence manifest. Representative hashes:

| Official document | SHA-256 |
| --- | --- |
| Feishu recall event | `5e6e8c192e7a9b96c796b7f18a986a8face3d6f07c2c8a2e4279a46da863988c` |
| Feishu reaction created | `702b8b0a981d5eead66a4fc551d861cd235639d0de090a5eb933f8990826e8c6` |
| Feishu reaction deleted | `bef68bf226b60bf499aacfa771356fecea7e73abad0d2f4b0e4794ef9f264c18` |
| Feishu webhook setup | `41ff65bbb6f075eea1157179a1012469a43795c5cea4087a5d7815c9d7ee89db` |
| Feishu receive event | `7a76be9631c20f1e0a0a7b3ecc43b9ec968b8230550088e82f25ff7d8be9b8d2` |
| Official SDK IM models at the pinned revision | `978002449250a0d8e08bee36450d24c222caff7d2e1ab97963148ef6c6d10008` |

Source inspection, local documentation checks and a diff secret scan validate
this audit. No Cargo build, NyxID modification, deployment, credential change,
subscription change or platform send is part of it. This remains a gap audit and
implementation plan, not implemented incoming-event or all-messaging support.
