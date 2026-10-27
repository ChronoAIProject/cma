# Native Lark / Feishu inbound media

Native `native_lark` sources can admit image, file, audio and video messages, and the exact image/video resources embedded in an inbound rich post. Resources use the existing `readEventAttachment` pipeline: image previews, bounded text pages, configured audio transcription, configured video keyframes or platform covers, and authorized one-use downloads. No resource bytes are downloaded during webhook admission.

`CMAEG_NATIVE_LARK_MEDIA_ENABLED` is an independent, default-off issuance gate. Both it and `CMAEG_MEDIA_ENABLED` must be true to admit a new message containing resources. The checked-in manifest keeps native Lark authoring, intake and media off. Deploy compatible core, gateway, provider, SDK and rollback readers before enabling the new `AttachmentSource` variant. Disabling issuance preserves exact duplicate admission, retained work, resource reads and original-target recall/reaction notifications; the shared media service must remain enabled for reads.

## Accepted inbound shapes

The authenticated receive event's `message.content` is a JSON string. Only the documented inbound shape is interpreted; arbitrary nested objects and outbound locale wrappers are not searched for resource keys.

| Native message type | Resource facts | Native resource API type |
| --- | --- | --- |
| `image` | `image_key` | `image` |
| `file` | `file_key`, optional `file_name` | `file` |
| `audio` | `file_key`, optional millisecond `duration` | `file` |
| `media` (video) | `file_key`, optional `image_key` cover, `file_name`, millisecond `duration` | `file`; cover uses `image` |
| `post` | Flat `title` plus rows in `content`; `img.image_key` and `media.file_key`, with optional `media.image_key` cover | `image` or `file` for each exact element |

The post renderer preserves text, link text and destination, mention keys, emotion names, horizontal rules and code-block text. It accepts the documented `text`, `a`, `at`, `img`, `media`, `emotion`, `hr` and `code_block` tags. It uses the shared regional `content` representation. Feishu's newer `content_v2` Markdown does not independently grant keys; the corresponding documented `content` structure supplies them. Missing `content`, unknown tags, invalid keys, and an oversized post refuse the whole message. There is no partial resource list or fabricated placeholder.

The parser accepts at most 16 resources, 256 post rows, 1,024 post elements, 48 KiB of serialized content and 32 KiB of rendered text. Resource keys use bounded ASCII letters, digits, `_` and `-`, at most 1,024 bytes. Bare UUID-style file keys appear in both regional inbound examples and are accepted alongside `file_`/`img_` keys. Filenames are bounded to 255 UTF-8 bytes and reject control/bidirectional formatting characters. Duration is bounded to 24 hours and rounded upward to whole seconds for the shared metadata field. MIME type and byte size are not invented from keys, extensions or message type; the media pipeline classifies retrieved bytes.

Sticker resources are explicitly unavailable in the regional download contracts. Folders cannot be downloaded by this API. Cards, merged-forward messages and their nested resources, shares, unknown message types and unproved rich-post tags are refused. Image/video covers use only the exact key in the same authenticated native message.

## Authority and retrieval

Each non-secret locator is `native_lark { message_id, resource_key, resource_kind, source_revision }`. The activity digest seals that locator and the receiving region, app, tenant and bot identity. The event reference retains the original chat/thread/message and the digest of the exact source admission policy and all three native credentials. The ordinary message ID remains the durable admission key: a changed resource, sender, conversation or text under the same key conflicts instead of becoming an edit. Historical text-only normalized envelopes keep their original serialized shape.

Tool reads and download redemption additionally verify the immutable original Journal binding and retained routing plan, including the originally selected consumer and provider. They do not authorize a new consumer or replacement provider by rereading current addressing rules. Current authority is reloaded before derivative-cache reuse and source fetches; source removal, admission changes, tenant/app/bot/region changes, consumer/provider changes, or credential rotation revoke retained access. Rotation during token acquisition or resource requests is checked before any fetched body is returned. A fetch uses a fresh tenant token rather than retaining a token cache across credential revisions.

The client obtains a tenant token from `/open-apis/auth/v3/tenant_access_token/internal`, verifies that token's bot at `/open-apis/bot/v3/info`, and requests only:

```text
GET https://open.feishu.cn/open-apis/im/v1/messages/{message_id}/resources/{resource_key}?type=image|file
GET https://open.larksuite.com/open-apis/im/v1/messages/{message_id}/resources/{resource_key}?type=image|file
```

The configured source region chooses the origin; neither event content nor resource keys can choose a host, path, query, proxy connection or credential. The resource connection pins public DNS results, disables redirects and proxies, and marks the authorization header sensitive. There is no NyxID proxy, generic URL fetch or fallback to a different key or message. HTTP 200 is the documented binary success contract; other statuses fail with bounded classifications. API bodies, tokens and resource identifiers are omitted from errors and tracing.

The shared regional profile fetches whole resources strictly below 100,000,000 bytes, with a 30-second source deadline and the existing, usually smaller media/job/download limits. Oversized declared bodies fail before reading; all reads and streams count actual bytes and stop at the cap. Feishu now documents Range support for larger files, but the Lark regional page does not; Range requests and resource assembly are outside this shared profile. Reads do not change or normalize the original resource format. Unsupported codecs remain explicit media-pipeline refusals.

The app must have bot capability, belong to the message's conversation and have one of `im:message`, `im:message:readonly` or `im:message.history:readonly`. This is additional to receive-event permissions, publication, installation and source admission. The API can refuse deleted, restricted, invisible or mismatched resources. No live platform acceptance, permissions grant or connected retrieval is implied by local validation.

## Official evidence pinned on 2026-10-28

The following official Markdown paths were fetched directly for both regions. SHA-256 values identify the exact reviewed bytes; the files are not vendored into the repository. Both receive-event pages link to the inbound-content contract.

| Evidence | Feishu SHA-256 | Lark SHA-256 |
| --- | --- | --- |
| [Receive event](https://open.feishu.cn/document/server-docs/im-v1/message/events/receive.md) / [Lark](https://open.larksuite.com/document/server-docs/im-v1/message/events/receive.md) | `7a76be9631c20f1e0a0a7b3ecc43b9ec968b8230550088e82f25ff7d8be9b8d2` | `4e043191a4e43ddfb46a154ca92357320799ddc756193a94feab3b00432aa465` |
| [Inbound content](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/events/message_content.md) / [Lark](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/events/message_content.md) | `24bb90bd1cdba3e4c68a708e665b48892282afba2dfdbc8d9dd39faf89ebbd54` | `7d2dcabba8f310cb7afe52190a2bb4c50b83f8eaa8d5ec811db191b44af14df1` |
| [Message resource GET](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-resource/get.md) / [Lark](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-resource/get.md) | `6907081a1b27e7a366492ee8cff71ff4468cf92c5ce65f94c9424025baa53167` | `4fb3c9cfcbb918be7b4beded7b0d9214eda83b7acf157a3801b2779f12d0f328` |
| [Custom-app tenant token](https://open.feishu.cn/document/ukTMukTMukTM/uADN14CM0UjLwQTN.md) / [Lark](https://open.larksuite.com/document/ukTMukTMukTM/uADN14CM0UjLwQTN.md) | `21c1a34c62492dce4a59b5c823e94a60a9ce259aca69f1f635ff2dedfeaf22fb` | `eb959977c97e9bff4479e972c42c8e9546ea3c8e72a9e650b09e080d244bc50d` |

The v3 custom-app token endpoint is marked historical by Feishu; the newer OAuth endpoint has a different contract. This increment retains the existing native source's proved v3 endpoint. Feishu's resource page explicitly permits Range only for `type=file`, caps each range at 32 MB and requires whole image downloads below 100 MB. Lark's resource page supports resources below 100 MB and excludes merged-forward submessage resources. The implementation uses their shared whole-resource contract.
