# Native Lark / Feishu outbound media

The separate `native_lark` source supports the bounded image, document, audio and video profiles below through `OutboundMessage.native_attachments`. `CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED` independently gates discovery and delivery, defaults to `false`, and remains false in the checked-in manifest. Inbound native media, NyxID relay media, Lark card/form flags and the WhatsApp media flag do not enable it. The existing native source and durable routed authority are required.

| Kind | Accepted MIME | Profile and filename extensions | Native upload / message |
| --- | --- | --- | --- |
| Image | `image/jpeg` | 8-bit, three-component baseline/progressive JPEG; `.jpg`, `.jpeg` | `images`, `image_type=message` → `image` |
| Image | `image/png` | 8-bit RGB/RGBA PNG; `.png` | `images`, `image_type=message` → `image` |
| File | `application/pdf` | Complete bounded PDF header/EOF; `.pdf` | `files`, `file_type=pdf` → `file` |
| File | `text/plain` | Complete UTF-8 or BOM-marked UTF-16 without disallowed control characters; `.txt` | `files`, `file_type=stream` → `file` |
| File | `application/msword` | Complete unencrypted compound Word document; `.doc` | `files`, `file_type=doc` → `file` |
| File | `application/vnd.ms-excel` | Complete unencrypted compound Excel workbook; `.xls` | `files`, `file_type=xls` → `file` |
| File | `application/vnd.ms-powerpoint` | Complete unencrypted compound PowerPoint document; `.ppt` | `files`, `file_type=ppt` → `file` |
| File | `application/vnd.openxmlformats-officedocument.wordprocessingml.document` | Complete bounded OOXML Word package; `.docx` | `files`, `file_type=stream` → `file` |
| File | `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` | Complete bounded OOXML Excel package; `.xlsx` | `files`, `file_type=stream` → `file` |
| File | `application/vnd.openxmlformats-officedocument.presentationml.presentation` | Complete bounded OOXML PowerPoint package; `.pptx` | `files`, `file_type=stream` → `file` |
| File | `application/zip` | Complete bounded ZIP archive; `.zip` | `files`, `file_type=stream` → `file` |
| File | `application/octet-stream` | Opaque bytes, unchanged, with any safe supplied filename or generated `.bin` | `files`, `file_type=stream` → `file` |
| Audio | `audio/ogg` | One complete mono Opus Ogg logical stream, bounded headers, valid page checksums, no chaining/multiplexing; `.opus`, `.ogg` | `files`, `file_type=opus` → `audio` |
| Video | `video/mp4` | Self-contained, nonfragmented, fast-start MP4 with one H.264 Baseline track and zero or one AAC-LC audio track; `.mp4` | `files`, `file_type=mp4` → `media` |

There are at most four attachments, 65,536 aggregate decoded inline bytes and 8,388,608 aggregate decoded/downloaded bytes. The common image profile also caps each side at 12,000 pixels and total pixels at 18,000,000. Video retains the shared structural limits: 1920×1080, 60 fps, ten minutes, 65,536 total samples, progressive pictures and one slice per frame. The shared profile checks track layout, sample ranges, timing tables, parameter sets and admitted I/P slice headers without decoding compressed payloads. Platform validation remains authoritative for decodability and client playback.

Typed document MIME types and filename extensions must match the complete bytes. Missing filenames receive a matching extension. Explicit `application/octet-stream` selects opaque file transport without asserting a document type; unspecified MIME also falls back to this for unrecognized bytes. Generic files keep a safe supplied filename; a missing suffix receives `.bin` to satisfy the native upload contract, within the same 255-byte filename limit. No file is re-encoded, converted or extracted to disk. Captions are refused for every kind; send explanation text in the message's existing `text` or text blocks. Discovery reports all fourteen MIME types, `max_total_bytes: 8388608`, `max_image_bytes: 8388608`, `max_caption_chars: 0`, and the shared `native_media.video` limits. It does not advertise stickers. Media-only messages do not produce placeholder text.

Broader formats remain goal gaps: other image formats, non-Opus audio, stereo/chained Opus, 3GPP, other video codecs/profiles, attached captions, video covers and duration metadata. Both regional APIs document duration and video cover as optional, so this increment omits them. Controls, cards/forms, templates, arbitrary reply targets and thread overrides cannot be mixed into this ordinary-media path. The separate default-off [image/control card path](CMAEG_Native_Lark_Image_Controls.md) supports one native PNG/JPEG image with buttons or choices. Legacy attachment blocks preserve their link-text meaning. This is a bounded native increment, not universal media-format parity with relay transport.

Typed Office validation reuses the existing full-byte document classifier. Legacy compound documents require matching unencrypted streams; OOXML requires matching content types, relationships and main XML identity. ZIP and OOXML validation caps the directory at 1,024 entries, each decompressed entry at 8 MiB and total decompressed bytes at 32 MiB; stored/deflated entries are CRC-checked without filesystem extraction. Typed ZIP excludes encrypted, ZIP64, multidisk, overlapping and unsafe-name archives. These are CMA validation bounds, not provider limitations; an explicitly opaque upload remains available for other binary content within the shared transport limit. Recognizable Office packages use typed MIME unless the caller explicitly chooses opaque transport. The upload preserves the original file bytes and encoding in every mode.

The provider's documented `doc`, `xls` and `ppt` values identify legacy Office formats. DOCX/XLSX/PPTX and ZIP use its generic `stream` value, which both regional API contracts explicitly support for other files. No native SDK schema migration is required: discovery's existing MIME list is additive, and the existing public `file` attachment kind already supports these payloads.

## Authority and delivery

The shared materializer prepares the entire batch before any token request or platform write. Public HTTPS uses its pinned public-address transport, aggregate cap and deadline; native credentials never accompany materialization. No caller can supply a platform upload key, local path, upload URL or alternative native destination.

Each write obtains a fresh regional custom-app tenant token and independently checks `/open-apis/bot/v3/info` against the retained bot identity. The routed gate then reloads exact current source/credential revision, original default or named consumer and provider authority before every upload and every recipient send. Removing a source/consumer, changing the provider or source admission/identity, rotating any of the three native credentials, or expiring the retained authority stops subsequent writes. An upload result is privately bound to the full retained destination, including region/app/tenant/bot, chat, original message, thread and revision.

Images are uploaded with multipart `image_type=message` and `image`; files use `file_type`, `file_name` and `file`. Only a bounded successful upload response supplies the subsequent `image_key` or `file_key`. Message content is serialized into the official JSON-string `content` field. Ordinary sends use the retained chat; retained thread destinations use the original message's `/reply` endpoint with `reply_in_thread=true`. A failed thread send never falls back to another conversation. The native origin comes exclusively from the enrolled region. Redirects and proxies are disabled; each API exchange and response body are bounded.

A successful recipient message must return a new message ID and the exact chat and optional thread. Its encrypted receipt retains the original event, source revision, consumer, provider, partition and conversation. Reactions/recalls on the confirmed output therefore retain their original sending target. Upload success alone is not message confirmation. Unknown upload/send acceptance, partial delivery and uncertain receipt storage stop the batch. Durable tool replay preserves confirmed or unknown outcomes and never repeats the upload/send after resumption. An uploaded resource may remain at the platform after authority revocation or a failed send; this increment does not attempt deletion.

The app requires bot capability and one of the documented upload permissions `im:resource` or `im:resource:upload`, plus the applicable native message-send permission, publication, installation and chat membership. The official image/file ceilings are 10 MB and 30 MB; the gateway's shared 8 MiB aggregate ceiling is deliberately lower. No live send or platform acceptance is implied by local fake-transport tests.

## Official evidence reviewed on 2026-10-28

Both regional upload, send, reply and message-content Markdown contracts were fetched directly. The following hashes identify the exact reviewed bodies; documentation snapshots are not vendored into the repository.

| Region | Official contract | SHA-256 |
| --- | --- | --- |
| feishu | [image](https://open.feishu.cn/document/server-docs/im-v1/image/create.md) | `02b65975f37b98fcabce103fab41826107a700abbb87f2d14b99782813d8546c` |
| feishu | [file](https://open.feishu.cn/document/server-docs/im-v1/file/create.md) | `63776fdbcf84472cd51ad0ec98a76b1fd15010b2e75a0dcf16aa3960b5071371` |
| feishu | [send](https://open.feishu.cn/document/server-docs/im-v1/message/create.md) | `23875d7fd3c9d25124f540a40f6b5976cc1314ac0512ea7332a71c0465fb4a08` |
| feishu | [reply](https://open.feishu.cn/document/server-docs/im-v1/message/reply.md) | `7c7e7210d6049616282ed891bbb0932cc090e98e0e43a78d34c8ed196d005944` |
| lark | [image](https://open.larksuite.com/document/server-docs/im-v1/image/create.md) | `78115c1946b053c016d48fec3ff7bc6f3cf1b9a08eb6234abf16720409167a24` |
| lark | [file](https://open.larksuite.com/document/server-docs/im-v1/file/create.md) | `7d633c4c09a25c8d13856c30f4c646f578d03ee3f0b61329595e29dd5c87ffe4` |
| lark | [send](https://open.larksuite.com/document/server-docs/im-v1/message/create.md) | `a0e47ace35f2f186ae6c0536135016bd7bd87df6a3bb890fb03a58a61eea051d` |
| lark | [reply](https://open.larksuite.com/document/server-docs/im-v1/message/reply.md) | `bf8db79f334a03951bd51b5bc69fb7a09fe51f22c95a083fcf1195039848778f` |
| lark | [content](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/create_json.md) | `db83ef8ca7820de987a293b60fcef9a914dfea6ea8b4e02e7b988e420d1218ff` |
| feishu | [content](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/create_json.md) | `41ed3446780b28b275151dbb8c6943eee2402973408890035cd71b67866c4853` |
