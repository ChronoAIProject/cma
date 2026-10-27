# Native Lark / Feishu interactive cards

The separate `native_lark` app has a direct JSON 2.0 card path for cards, buttons, choices and the bounded form subset. It uses the app and credentials already recorded on the native source. NyxID relay card and form settings retain their separate transport and authority. This implementation must pass the gateway validation suite and separately authorized platform acceptance before activation; source and injected transport checks do not prove live platform acceptance.

## Activation and discovery

`CMAEG_NATIVE_LARK_CONTROLS_ENABLED` and `CMAEG_NATIVE_LARK_FORMS_ENABLED` both default to `false`, including the checked-in manifest. Controls require native intake and durable routed admission/recovery; forms also require controls. Deploy compatible readers before enabling issuance. Authenticated channel discovery exposes `native_lark_controls_enabled` and `native_lark_forms_enabled`, after service prerequisites are applied. The native-source editor displays these values and the callback subscription instructions.

For a verified original human event, `getEventContext.capabilities.buttons` and `.cards` become `native_card`; `.forms` explicitly reports the native form gate. Default and named consumers each retain their exact original provider, key, partition and conversation. The generic relay consumer-interactions flag does not authorize this native path. Gate-off stops new card issuance and discovery; already confirmed cards retain callback reading, recovery, update, recall and own-reaction behavior while their stored authority remains current.

## Separate callback configuration

In the separate native app, open **Events & Callbacks > Callback configuration > Send callbacks to developer server**. Use the channel's existing `/v1/native-lark/{channel_id}` HTTPS endpoint and subscribe to **`card.action.trigger` only**. This is distinct from event subscriptions such as `im.message.receive_v1`. Do not enable the legacy `card.action.trigger_v1` subscription for this path: subscribing to both versions delivers two callbacks per action. Saving a channel does not change the app's subscriptions.

Configure the same Encrypt Key and Verification Token recorded on this source. The App Secret supplies tenant access tokens and is not the webhook signature key. CMAEG verifies the exact signed encrypted bytes, decrypts the v2 envelope, and checks app, tenant, bot, human operator and exact message/chat context. `header.token` is the verification credential; `event.token` is a separate short-lived card-update credential. Both are removed before callback facts are retained. This implementation never uses the delayed-update credential and never returns a replacement card in its callback response.

A URL-verification challenge has a one-second platform deadline and does not write interaction or routing records. Card callbacks have a three-second platform deadline; CMAEG bounds the request to 2.5 seconds and returns `{}` only after durable routed admission. Lark does **not** automatically retry these callbacks. CMAEG therefore persists the canonical winning click and the exact admission data in one encrypted card CAS and continuously scans unadmitted cards after interruption or restart. A failed or timed-out admission returns a non-success status. A distinct repeated click cannot replace a committed winner.

## Supported card and form shape

Send the existing `OutboundMessage` blocks to `replyToEvent`. The shared renderer retains JSON schema `2.0`, `config.update_multi:true`, a 28 KiB card bound, at most 100 rendered elements/actions and 4,000 display characters. Native transport additionally checks the encoded request against 30 KiB. Native attachments, WhatsApp templates, arbitrary reply targets and native-media/card mixtures are refused.

The original human alone may click in the exact retained private chat or group. Existing group threads are supported only when the original native event retained that exact thread ID, the native send response confirms it, and later metadata matches it. This does not create threads or change relay thread behavior. Native `cnl1` and `cnlf1` locators carry only record/revision/index; provider action IDs and form values are resolved from the encrypted schema. Relay locators cannot consume native cards.

Forms reuse the bounded `form` block: one form, 1–12 fields, one submit, no independent buttons, opaque field names, text/number/date/single-choice validation and a 16 KiB submitted-value bound. Extra, missing required, malformed and out-of-range values are refused before consumption. The accepted wire event is an official v2 `button` callback with `name:submit` and `form_value`. URL previews and unsupported input/select callback variants cannot dispatch work.

## Confirmed references and mutations

A successful native create/reply must first persist an exact native outbound receipt and then bind the card and its stable sealed message reference. Unconfirmed sends or receipt/bind failures issue no usable controls reference. Source and credential changes, owner/provider changes, key rotation and routing changes revoke stale card authority at issuance, binding, callback consumption, recovery, dispatch and mutation boundaries.

`updateMessage` uses `PATCH /open-apis/im/v1/messages/{message_id}` with serialized JSON content. It rechecks fresh app/bot custody and own-app `interactive` metadata, including exact tenant, chat, message and thread. A successful PATCH returns empty `data:{}`; CMAEG preserves the original message identity. Replacement never extends the original 24-hour lifetime or changes the saved consumer. The old revision is durably superseded before PATCH. A durable successful PATCH marker precedes binding the replacement.

Card update, recall, own-reaction replacement/clear and callback consumption share the same exact-message fence. Local-only callback locks can recover a crashed CAS writer; an ambiguous external mutation lock cannot be recovered by a timer or a new request key. Delete supersedes the card before remote DELETE. Timeout, malformed success or uncertain state persistence keeps callbacks and replacement authority fenced. Rejected updates also leave the superseded revision closed; applications should send a fresh card only as a new explicit operation. A reference derived internally for native effects is never returned to callers, so it cannot bypass card revocation.

## Official contracts

- [Lark v2 card callback](https://open.larksuite.com/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-callback-communication)
- [Feishu v2 card callback](https://open.feishu.cn/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-callback-communication)
- [Lark card PATCH](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/patch)
- [Feishu card PATCH](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/patch)

The implementation review pins the fetched callback, security, configuration and PATCH documents for both regions. The callback creation timestamp is in microseconds. Local authority lasts at most 24 hours, which is stricter than the provider's 14-day message update window and 30-day callback window.
