# Native Lark / Feishu image cards with controls

A native Lark source can send one PNG/JPEG image and buttons or choices in one JSON 2.0 interactive card. `CMAEG_NATIVE_LARK_MEDIA_CONTROLS_ENABLED` defaults to false and the checked-in manifest keeps it disabled. It additionally requires native controls, native outbound media, native intake and durable routed admission. Deploy compatible card readers before enabling issuance. This flag does not change ordinary media or relay delivery.

For a verified original human event with current source and consumer custody, `getEventContext.capabilities.native_media.controls` declares `kinds: ["image"]`, `max_attachments: 1`, `buttons: true`, `choices: true`, `caption_max_chars: 0` and `update: true`. The broader `native_media.kinds` list still describes ordinary media; the nested kinds restrict combinations. Older capability responses omit `controls` or its optional `kinds` property without acquiring new permissions. Clients must not assume that an ordinary file/audio/video capability permits those kinds in an interactive card.

Send one existing `native_attachments` image with regular message text and `buttons` or `choice` blocks. The existing image materializer validates the full source before any native request: up to 64 KiB decoded inline, 8 MiB from pinned public HTTPS, structurally supported PNG/JPEG, matching declared MIME and filename extension. Captions remain unsupported; use message text or text blocks instead. Multiple images, files/audio/video, forms, arbitrary reply targets, native media replacement and invalid bytes are refused. The gateway never splits a requested image/control card into separate messages or falls back to a link.

The gateway prepares the ordinary opaque controls record without the raw URL or base64, verifies the exact regional app/bot, and uploads using the native image endpoint. Only a confirmed image key bound to that exact original app, tenant, chat, message, thread and source revision may enter the card. Fresh custody is checked after upload. The compact image key becomes part of the encrypted card's custody digest and is saved before sending. The resulting card contains an `img` element (`img_key`, required plain-text `alt`, `scale_type: fit_horizontal`, `preview: true`) followed by the existing text/opaque controls. The image counts toward the existing 100-element and 28 KiB card limits.

A single confirmed interactive message, its durable native receipt and the existing successful card binding are required before the caller receives a usable message reference. Upload uncertainty, send uncertainty, revoked authority or record failure produces no usable reference and no fallback send. An uploaded resource may remain at the platform after a later failure; no cleanup message or extra external call is attempted. Durable tool replay continues to retain exact confirmed/unknown outcomes through the shared delivery machinery.

`updateMessage` with ordinary text and control blocks preserves the confirmed image without another upload. It uses the existing exact-message fence, superseded old revision, one PATCH, durable confirmed marker and replacement binding. An uncertain PATCH leaves the previous revision fenced. Updates cannot add, remove or replace the image, extend its original 24-hour card lifetime or change its consumer. Stopping new image-card issuance retains editing, callback consumption and admitted-click recovery for already confirmed cards whose authority remains current. Removing the source or changing app/credentials, actor, consumer/provider or routing prevents stale actions. Historical image-free cards retain exactly their previous serialization and digest.

Local transport tests cover both regional origins, default/named consumers, ordinary/thread replies, buttons/choices, multipart upload then one interactive send, edits, callback replay and recovery, gate-off retained operations, invalid shapes, uncertainty and revocation between upload and send. These are not connected platform acceptance tests; authorized owner testing on Lark and Feishu is still required before claiming live acceptance.

## Official JSON 2.0 image contracts reviewed on 2026-10-29

The image component schema was fetched directly from each regional documentation site's Markdown contract. These SHA-256 values identify the reviewed bodies; full document snapshots are not vendored.

| Region | Contract | SHA-256 |
| --- | --- | --- |
| Lark | [JSON 2.0 image](https://open.larksuite.com/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-json-v2-components/content-components/image.md) | `b78433ed8bf38fc44926c59c04b5ffdebf4cbb47f160d9cb9fce646247e8c737` |
| Feishu | [JSON 2.0 image](https://open.feishu.cn/document/uAjLw4CM/ukzMukzMukzM/feishu-cards/card-json-v2-components/content-components/image.md) | `bf4a82bcb9286f360a8cf69a2ab868be7768513f42dbf3c6bb516220bd342ede` |

Upload/send/reply contracts are pinned in [native outbound media](CMAEG_Native_Lark_Outbound_Media.md). Callback, PATCH, source custody and recovery are described in [native controls](CMAEG_Native_Lark_Controls.md).
