# Native Lark / Feishu image cards with controls

A native Lark source can send one PNG/JPEG image and buttons or choices in one JSON 2.0 interactive card. `CMAEG_NATIVE_LARK_MEDIA_CONTROLS_ENABLED` defaults to false and the checked-in manifest keeps it disabled. It additionally requires native controls, native outbound media, native intake and durable routed admission. Deploy compatible card readers before enabling issuance. This flag does not change ordinary media or relay delivery.

For a verified original human event with current source and consumer custody, `readEventContext` and `verifyEventContext` expose `capabilities.native_media.controls` with `kinds: ["image"]`, `max_attachments: 1`, `buttons: true`, `choices: true`, `caption_max_chars: 0` and `update: true`. The broader `native_media.kinds` list still describes ordinary media; the nested kinds restrict combinations. Older capability responses omit `controls` or its optional `kinds` property without acquiring new permissions. Clients must not assume that an ordinary file/audio/video capability permits those kinds in an interactive card.

Send one existing `native_attachments` image with regular message text and `buttons` or `choice` blocks. The existing image materializer validates the full source before any native request: up to 64 KiB decoded inline, 8 MiB from pinned public HTTPS, structurally supported PNG/JPEG, matching declared MIME and filename extension. Captions remain unsupported; use message text or text blocks instead. Multiple images require the additional issuance gate described below. Files/audio/video, forms, arbitrary reply targets, native media replacement and invalid bytes are refused. The gateway never splits a requested image/control card into separate messages or falls back to a link.

The gateway prepares the ordinary opaque controls record without the raw URL or base64, verifies the exact regional app/bot, and uploads using the native image endpoint. Only a confirmed image key bound to that exact original app, tenant, chat, message, thread and source revision may enter the card. Fresh custody is checked after upload. The compact image key becomes part of the encrypted card's custody digest and is saved before sending. The resulting card contains an `img` element (`img_key`, required plain-text `alt`, `scale_type: fit_horizontal`, `preview: true`) followed by the existing text/opaque controls. The image counts toward the existing 100-element and 28 KiB card limits.

A single confirmed interactive message, its durable native receipt and the existing successful card binding are required before the caller receives a usable message reference. Upload uncertainty, send uncertainty, revoked authority or record failure produces no usable reference and no fallback send. An uploaded resource may remain at the platform after a later failure; no cleanup message or extra external call is attempted. Durable tool replay continues to retain exact confirmed/unknown outcomes through the shared delivery machinery.

`updateMessage` with ordinary text and control blocks preserves the confirmed image without another upload. It uses the existing exact-message fence, superseded old revision, one PATCH, durable confirmed marker and replacement binding. An uncertain PATCH leaves the previous revision fenced. Updates cannot add, remove or replace the image, extend its original 24-hour card lifetime or change its consumer. Stopping new image-card issuance retains editing, callback consumption and admitted-click recovery for already confirmed cards whose authority remains current. Removing the source or changing app/credentials, actor, consumer/provider or routing prevents stale actions. Historical image-free cards retain exactly their previous serialization and digest.

Local transport tests cover both regional origins, default/named consumers, ordinary/thread replies, buttons/choices, multipart upload then one interactive send, edits, callback replay and recovery, gate-off retained operations, invalid shapes, uncertainty and revocation between upload and send. These are not connected platform acceptance tests; authorized owner testing on Lark and Feishu is still required before claiming live acceptance.

## Two to four images in one control card

`CMAEG_NATIVE_LARK_MULTI_IMAGE_CONTROLS_ENABLED` defaults to `false` and additionally requires the existing image-card, outbound-media and native-control issuance gates. When enabled, verified discovery reports `native_media.controls.max_attachments: 4`; kinds remain `["image"]`, captions remain unsupported and `update: true` continues to mean text/control edits preserving the images. No new public request or SDK type is introduced. With the gate off, new combined requests retain the one-image limit.

The gateway validates every PNG/JPEG source and reserves all image elements and maximum key lengths before making any platform call. The aggregate 8 MiB preparation bound, inline bounds, 100-element limit and 28 KiB card limit remain unchanged. Uploads run in request order with current app/bot and local source/consumer custody checked between writes. Only the complete set of confirmed app-scoped image keys is saved to one durable card, followed by one interactive message containing ordered `img` elements. A failed or uncertain later upload sends no recipient message; previously uploaded resources may remain at the platform. An uncertain card send never triggers splitting, fallback or a blind resend.

Historical text cards still omit all media fields. Historical and newly issued single-image cards retain exactly their scalar `image` representation and existing custody digest. Multiple-image cards use an optional `images` field containing two to four confirmed keys, in order; `image` and `images` are mutually exclusive. Both serving and retained rollback readers must understand `images` **before** new issuance is enabled. Turning the gate off does not make an older reader compatible with retained multiple-image cards.

Text/control revisions preserve every original key in the same order, original destination, consumer, expiry and callback/recovery authority. Updates cannot add, remove, reorder or replace images. Retained operations continue when issuance is disabled, subject to fresh custody. The existing exact-message mutation fence and durable unknown outcomes still prevent repeated uncertain PATCH operations.

Focused local transport tests exercise both regions, two/four images, default/named consumers, ordinary/thread replies, buttons/choices, complete preflight, a rejected or unconfirmed second upload, source/bot revocation between uploads, edits after gate shutdown, callback replay/recovery, retained-order tampering and uncertain-PATCH fencing. These tests do not establish connected Lark/Feishu acceptance.

## Official JSON 2.0 image contracts reviewed on 2026-10-29

The image component schema was fetched directly from each regional documentation site's Markdown contract. These SHA-256 values identify the reviewed bodies; full document snapshots are not vendored.

| Region | Contract | SHA-256 |
| --- | --- | --- |
| Lark | [JSON 2.0 image](https://open.larksuite.com/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-json-v2-components/content-components/image.md) | `b78433ed8bf38fc44926c59c04b5ffdebf4cbb47f160d9cb9fce646247e8c737` |
| Feishu | [JSON 2.0 image](https://open.feishu.cn/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-json-v2-components/content-components/image.md) | `bf4a82bcb9286f360a8cf69a2ab868be7768513f42dbf3c6bb516220bd342ede` |

Upload/send/reply contracts are pinned in [native outbound media](CMAEG_Native_Lark_Outbound_Media.md). Callback, PATCH, source custody and recovery are described in [native controls](CMAEG_Native_Lark_Controls.md).

The JSON 2.0 body contracts were also fetched directly on 2026-10-29. Both specify that `body.elements` contains component JSON and renders components vertically in array order. Together with the image component contract above, this supports multiple ordered `img` components in the same card. CMA keeps its existing lower 100-element limit even though the provider body contract allows 200 elements/components.

| Region | Body contract | SHA-256 |
| --- | --- | --- |
| Lark | [JSON 2.0 structure](https://open.larksuite.com/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-json-v2-structure.md) | `1efcb67e1c7cbc804b74183a455ccf7aa3347f1b0d58e2651841c6bee5d249a6` |
| Feishu | [JSON 2.0 structure](https://open.feishu.cn/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-json-v2-structure.md) | `afd94bb6f035e930b3d40a08ad08b7109672fb850e8a0369cb80e9398a57e90f` |
