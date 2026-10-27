# Separate native Lark / Feishu message effects

The separately provisioned `native_lark` app supports ordinary confirmed-message text edits, recalls and its own reactions. It can also react to the exact received human message selected by a retained routed admission. This uses the native app's regional credentials and a distinct `native_lark_effect` authority inside the sealed `message_ref`. The NyxID relay `lark_effect` contract does not authorize this path.

## Issuance and supported subset

Both flags default to `false` and remain false in the checked-in manifest:

| Flag | Issued authority |
| --- | --- |
| `CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED` | One confirmed native reply message: recall and own reaction; a single text message additionally permits text editing. |
| `CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED` | `received_message` in verified event context: reaction-only authority for the exact original human message and selected consumer/provider. |

The confirmed flag also enables fresh bot-custody verification for text sends before issuing a reference. A reference is returned only after the exact outbound receipt is durably stored. A reply containing multiple native messages does not expose just its first message as an editable aggregate. A single media message grants recall/reaction authority without text editing. Native media sending remains controlled by its separate media flag.

`updateMessage` accepts one bounded rendered text part, at most 6,000 characters and 28 KiB of UTF-8 text, with a full serialized request bound of 150 KiB. Native attachments, controls, templates, explicit reply targets and thread-creation overrides are refused. Editing preserves the original chat and optional thread. Rich-post replacement, cards/forms and media replacement are outside this subset.

`deleteMessage` recalls only a confirmed message sent by this app. Received user messages never grant deletion or editing, even if the app has administrative permissions. `setMessageReaction` accepts zero or one platform `emoji_type`, such as `SMILE` or `THUMBSUP`, without `is_big` or custom emoji. It retains only the reaction ID returned for this app and never enumerates or removes another actor's reactions. Clearing without a retained reaction performs no platform mutation.

Turning either issuance flag off stops new references and their corresponding discovery. Retained valid references remain executable by upgraded readers. Retained confirmed references last at most 24 hours from their original receipt; received references last at most 24 hours from the original message and also respect the original event expiry. Resealing does not extend either lifetime. Platform age, permissions and administrator limits can be stricter; the documented message edit limit is 20 edits per message.

## Authority and mutation handling

The authority retains region, app ID, tenant key, verified bot open ID, original channel record, exact message/chat/thread, source and credential revision, selected consumer/provider, current routing revision, and the original receipt or event digest. Changing a native app credential, source policy, owner, selected consumer key/provider, channel record or routing revokes it. Unrelated channel display-name changes do not change that authority.

Received authority is issued only after `current_received_message` loads the durable admission, plan and tool binding and proves the exact selected original target. A valid event seal alone is insufficient. Recall notifications, reaction deltas, bot messages, synthetic events and a different consumer's context do not grant received-message reaction authority.

Before every platform mutation, the client obtains a fresh token from the configured regional origin, verifies the exact bot with bot-info, and gets the exact message. Metadata must match message ID, chat, optional thread, explicit non-deleted state, supported ordinary message type, sender type/ID and tenant. Confirmed targets require the exact sending app; received targets require the retained human open ID. The current local source/consumer/provider authority is checked again after the network reads, immediately before the write.

Stable effect idempotency claims exclude randomized token ciphertext and issuance timestamps. One durable exact-message fence spans text edits, recalls and reactions, including calls from different consumers and different request keys. A reaction replacement removes the retained own reaction, persists that removal, repeats native and local custody checks, then creates the new reaction. Both removal and creation receipts must match the app, emoji and retained reaction ID where applicable.

An ambiguous platform result, malformed success response, uncertain record write, interrupted operation or refusal after the removal step leaves the exact message fenced. Restart, resealing and a new request key do not permit automatic resend. `updateMessage` has no caller idempotency-key argument; its destination fence still blocks later mutations after an uncertain update. Explicit refusals before any mutation can release the fence. These records expire only after every reference for the original message has expired.

## Official regional contracts

Reviewed on 2026-10-28 using public documentation only; no authenticated native messages or mutations were sent.

| Operation | Method and path | Lark | Feishu |
| --- | --- | --- | --- |
| Verify target | `GET /open-apis/im/v1/messages/:message_id?user_id_type=open_id` | [Get message](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/get.md) | [Get message](https://open.feishu.cn/document/server-docs/im-v1/message/get.md) |
| Edit text | `PUT /open-apis/im/v1/messages/:message_id` | [Edit message](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/update.md) | [Edit message](https://open.feishu.cn/document/server-docs/im-v1/message/update.md) |
| Recall own message | `DELETE /open-apis/im/v1/messages/:message_id` | [Recall](https://open.larksuite.com/document/server-docs/im-v1/message/delete.md) | [Recall](https://open.feishu.cn/document/server-docs/im-v1/message/delete.md) |
| Add own reaction | `POST /open-apis/im/v1/messages/:message_id/reactions` | [Add reaction](https://open.larksuite.com/document/server-docs/im-v1/message-reaction/create.md) | [Add reaction](https://open.feishu.cn/document/server-docs/im-v1/message-reaction/create.md) |
| Remove retained own reaction | `DELETE /open-apis/im/v1/messages/:message_id/reactions/:reaction_id` | [Remove reaction](https://open.larksuite.com/document/server-docs/im-v1/message-reaction/delete.md) | [Remove reaction](https://open.feishu.cn/document/server-docs/im-v1/message-reaction/delete.md) |

The app requires bot capability, access to the conversation and appropriate read/update/recall/reaction permissions. The implementation uses the custom app's `tenant_access_token`; reaction operator receipts must identify `operator_type: app` and the exact `app_id`. Redirects and proxies are disabled, response bytes are bounded, and only explicit valid success confirms a mutation. Incoming edit events remain unsupported; enabling outgoing editing does not establish an incoming edit/revision contract.
