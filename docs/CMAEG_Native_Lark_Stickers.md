# Native Lark and Feishu received stickers

Native sticker intake records that an authenticated human sent a sticker. The receive event supplies a `file_key`, without image pixels, caption, emoji or visual meaning. CMA keeps that key only in the encrypted original routing plan. Agents see a descriptive message and an `other` attachment with `variant: sticker`; its reference hashes the original message ID and key. It has no download source or thumbnail. This support makes no claim that an agent can see or interpret the sticker.

Lark and Feishu explicitly prohibit downloading sticker resources. They permit sending only stickers the bot already received, using the key from a received-message event. Their file-upload API has no arbitrary sticker upload type. CMA therefore adds no sticker URL/base64 upload, resource-download fallback or invented media metadata.

## Reply contract

When the exact retained event is currently authorized, `readEventContext` or `verifyEventContext` may advertise `capabilities.reply_with_received_sticker: true`. Call `replyToEvent` with the original event reference, a stable idempotency key and this message:

```json
{"reply_with_received_sticker": true}
```

The request is exclusive of text, blocks, ordinary native attachments, templates, media updates, clear, explicit reply targets and thread creation. It selects only the original event's retained sticker. It accepts no `file_key`, arbitrary recipient or URL. CMA uses the original bot, source revision, chat and already-established thread. `updateMessage`, initiated sends and default-recipient routes refuse the request.

The original Journal target binding, admission/request hash, activity digest, sealed source bytes, provider, owner, consumer, destination and current source credentials must match. Current authority is checked before the platform effect through the existing routed delivery gate. The send uses the ordinary durable reply intent and confirmed-message receipt. An unconfirmed platform response remains `unknown`; replay never blindly sends another sticker. The feature does not add a separate retry loop.

## Rollout and rollback

Both switches default to false:

- `CMAEG_NATIVE_LARK_STICKERS_ENABLED`: issue new retained sticker facts, independent of downloadable media admission.
- `CMAEG_NATIVE_LARK_STICKER_REPLIES_ENABLED`: advertise and perform explicit exact-event resend.

Deploy compatible gateway, SDK and retained-source readers before enabling either writer. Existing retained native source structs reject unknown fields, so do not roll readers back after admitting sticker facts. Disable issuance instead; exact retained duplicate admission and durable results continue to replay. The optional outbound field is omitted when false and the optional context capability is absent unless proved, preserving legacy serialized bodies and hashes. Generic `native_media` capabilities do not advertise Lark sticker uploads.

Native connected verification is still required before activation. Tests use local native transports and record servers and send no real platform messages.

## Official contract evidence, reviewed 2026-10-29

Both regional documentation sets were checked:

| Contract | Lark | Feishu |
|---|---|---|
| Incoming sticker has only `file_key` | [Receive content](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/events/message_content.md) | [Receive content](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/events/message_content.md) |
| Send only a sticker already received by the bot | [Message content](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/create_json.md) | [Message content](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/create_json.md) |
| Sticker resource download unsupported | [Resources](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-resource/get.md) | [Resources](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-resource/get.md) |
| Sticker accepted by the reply endpoint | [Reply](https://open.larksuite.com/document/server-docs/im-v1/message/reply.md) | [Reply](https://open.feishu.cn/document/server-docs/im-v1/message/reply.md) |
| No arbitrary sticker upload type | [File upload](https://open.larksuite.com/document/server-docs/im-v1/file/create.md) | [File upload](https://open.feishu.cn/document/server-docs/im-v1/file/create.md) |
