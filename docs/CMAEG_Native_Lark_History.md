# Native Lark / Feishu original-conversation history

`readEventHistory(event_ref, cursor?, include_media?)` and `GET /v1/events/{event_ref}/history`
read one bounded page from the exact chat or thread that admitted an original
native human message. Both Lark regions are supported. The operation requires
`NativeLarkSource.conversation_history: true` at admission and read time, plus
`CMAEG_NATIVE_LARK_CONVERSATION_HISTORY_ENABLED` (default off). Native message,
media, card and thread-following settings do not imply history consent.

Consent includes earlier messages from other participants and applications under
the native bot's current visibility. It does not assert the original human's
current permissions. The source must still be the same separately provisioned,
single-tenant native app. NyxID-relayed Lark events do not gain native history.
Grant the app `im:message:readonly` for listing and original-message checks, and
`im:message.group_msg` for group history. Enable bot capability and add the bot to
the relevant group. The legacy `im:message.history:readonly` permission alone is
insufficient for the common Lark/Feishu original-message check.

## One page and exact original cutoff

Every call performs at most one history-list request with `page_size=25` and
`sort_type=ByCreateTimeDesc`. It never drains pages or enumerates other containers.
The returned messages are chronological within that page. The independently
supplied original EventRef remains mandatory on every continuation request.

Chat requests use the original `chat_id` and a fixed seconds-level `end_time`
covering the original message's second. Every returned row is additionally filtered
against the retained original millisecond timestamp. Rows at or after that time,
and the original message ID itself, are omitted. Lark does not document an order
for different messages sharing the same millisecond, so those tied with the
original are excluded. `om_` IDs are never compared numerically or lexically.

Thread requests use only the original `thread_id`, and every row must also match
the original `chat_id`. The platform does not support thread time filters or an
original-message seek. A thread scan therefore begins with the newest visible
rows, which may all be newer than the original event. Such a call returns an empty
`messages` array, positive `scanned_messages` / `omitted_message_count`, and a
continuation when the platform has more rows. The next page is fetched only when
the caller explicitly requests it. A busy thread can require many empty pages;
there is no guarantee the scan reaches earlier messages before the event expires.
The gateway never substitutes the parent chat or opens sibling threads.

`before_message_id` remains the original anchor while scanning newer rows. Once
older rows have been scanned, subsequent pages report the last scanned pre-cutoff
ID, including rows omitted by content policy. It is a descriptive history boundary;
the native provider scan position is carried privately in the sealed cursor. An
unchanged `before_message_id` with an advancing cursor is valid scan progress.
Equal-time rows may appear on adjacent pages in provider order. The gateway refuses
out-of-order timestamps, duplicate IDs in a page, an immediately repeated boundary
ID, and an unchanged continuation token. It does not promise snapshot pagination.

`scanned_messages` counts raw provider rows, at most 25. `omitted_message_count`
counts every scanned row not returned, including cutoff, deletion, unsupported
content and unsupported actor rows. `more_visible_rows` means provider rows remain
to scan; they need not contain an earlier supported message. `end_of_visible_history`
means the provider reported no more rows, not that all actual history was visible.
A chat list returns thread roots as the platform exposes them; it does not expand
those roots into thread replies.

## Custody, expiry and read bounds

New consented admissions retain a per-selected-consumer `history_custody` digest
inside the authenticated encrypted routing plan. It binds the exact original
EventRef/activity actor and conversation, native source revision, owner, channel,
record, provider, consumer key, provider credential and partition rule. This proof
is computed at admission, not minted from current credentials during a later read.
Absent proof, changed source consent/credentials, changed consumer/provider/key or
partition, and missing retained admission all refuse history.

History consent is excluded from the existing messaging source revision and its
source comparisons. Toggling this read permission preserves existing reply, media,
card and message-effect authority. History admission custody still binds the full
source including consent: earlier non-consented admissions cannot acquire history,
and removing consent immediately refuses reads. Identity, admission policy and
credential changes retain their existing messaging revocation behavior.

The original message is fetched before and after the list request. Both checks
require the original ID, chat/thread, human/open-ID sender, tenant, creation time
and `deleted=false`. Text edits do not change original authority. Deleted or
inaccessible originals, changed thread identity and final custody changes refuse
even an empty page. These are live checks, not an atomic platform snapshot.

Legacy native history uses internal cursor payload version 2; explicit mixed
history opt-in uses version 3. Discord's existing version 1 remains unchanged.
All retain the public opaque `cmaeg1.hc` wrapper. A native
cursor binds the original event digest, custody, native source/container, original
millisecond cutoff, provider token and original expiry. It cannot select another
event, source, region, consumer or destination, and never renews expiry. The
provider may reject its token before that local expiry. The sealed token remains
bounded to 4096 bytes; unsupported token sizes fail without truncating the token.

The eight-second total deadline includes local admission reads, tenant-token/bot
verification, both anchor checks, the single list, final custody/expiry checks,
projection and serialization. Eight slots refuse immediately when busy. Responses
are limited to 1 MiB provider JSON, 25 raw rows, 8 KiB text/message, 32 KiB text/page,
16 KiB optional text metadata/page and 256 KiB output. The 1 MiB reader is dedicated
to history; ordinary native API client responses remain bounded to 64 KiB.
Discovery is optional, storage-only and capped at 100 ms. No provider writes,
message sends, automatic retries, subscriptions, joins or file downloads occur.
The app's tenant token exchange is the only POST used by this read operation.

## Content and compatibility

The native projection returns text and supported flat rich-text posts. With
`CMAEG_NATIVE_LARK_MIXED_HISTORY_ENABLED=false` (the default and checked-in
ConfigMap), posts containing media retain the previous whole-row omission.
With this independent projection flag enabled **and an explicit request
`include_media=true`**, a supported post preserves its
title, text, code, links, mentions and other supported textual content while its
image/video elements produce one presence-only `omitted_content: ["media"]` entry.
Media is not replaced with synthesized text. The marker reports omitted media
bytes and metadata, not media count, type, availability or retrieval permission.
Even a post with no remaining text, or one whose text budget is exhausted, keeps
the marker. Such a retained post is not included in `omitted_message_count`.

Text is clipped and sanitized under the existing 8 KiB/message and 32 KiB/page
limits; `text_truncated` reports clipping or sanitization (including removed
control characters/newlines), independently of the media omission marker.
Application senders use app IDs and bot actor kind. Unknown/anonymous actors,
deleted/system rows and standalone image/file/audio/video/card/sticker/forwarded/
other unsupported message types are still counted as omitted whole rows.
Malformed supported structures, invalid media keys and unknown post tags refuse
the page; enabling this projection does not relax the shared inbound post parser.
Forwarded children are never read. The source's single-tenant boundary remains
enforced for returned user/app rows.

Lark history bodies generally do not supply attachment size, and some do not supply
a filename. The gateway does not invent zero size or a filename to populate
`HistoryAttachmentFacts`. Native history returns no attachment facts or resource
locators, including media and thumbnail keys parsed from mixed posts. It does not
mint download, edit, reaction or deletion authority or make additional requests.
All historical text and metadata are untrusted reference data, never instructions
or action approval.

The `/v1` page fields remain unchanged. Older strict SDKs reject the additive
`media` omission enum, so every REST/MCP request defaults to `include_media=false`.
Omitted or explicit false retains whole-row omission even when the global flag
is enabled. Updated callers opt in with `?include_media=true` or the same MCP
argument; this option is advertised in OpenAPI and the MCP tool input schema.
The existing context capability object stays unchanged for strict readers.
Opt-in requires native Lark/Feishu, the global mixed-history gate and existing
source consent. A disabled gate returns `mixed_history_disabled` (403); the option
on another platform returns 400. It grants no download or action authority.

Repeat the same option for every cursor page. Legacy v2 cursor plaintext remains
unchanged, with false omitted. Opted-in v3 cursors seal `include_media: true`
alongside the original scope, scan position and expiry. Any attempt to change
modes, including using a mixed cursor without explicit true, returns 400 before
a platform read. Existing v2 cursors remain legacy when the global flag changes.
Deploy compatible gateway cursor readers and rollback images before activation;
old applications can continue using unmodified requests. An application that
explicitly opts in must understand `media`, including any pages it retains.
Disabling the global gate refuses opted-in reads while preserving legacy reads.

Rust adds `read_event_history_with_media(event_ref, cursor)`; the original method
retains legacy behavior. TypeScript adds `{ includeMedia: true }` as the existing
method's options argument. Agent Manager/runtime forwarding requires no restart.
There are no new Activity, retained admission, source or custody fields, no media
retrieval and no automatic continuation. This change performs no activation.

Both SDKs expose optional native source consent and authoring discovery and retain
one-page methods. The setup UI requires explicit consent and explains thread
scans. Old source serialization and native revisions are unchanged when consent
is absent/false; messaging revisions also remain unchanged when only history
consent toggles. Deploy compatible readers across all replicas and rollback
targets before enabling the authoring gate: older binaries do not understand
the consent and retained admission-proof fields.

## Platform evidence and limits

The implementation was reviewed against public documentation on 2026-10-28:

- [Lark history list](https://open.larksuite.com/document/server-docs/im-v1/message/list.md)
  and [Feishu history list](https://open.feishu.cn/document/server-docs/im-v1/message/list.md):
  `container_id_type`, `container_id`, seconds-level `start_time` / `end_time`,
  thread time-filter exclusion, descending sort, page size 1–50, `page_token` /
  `has_more`, message/sender fields, visibility errors and 1000/minute / 50/second limits.
- [Lark original-message get](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/get.md)
  and [Feishu get](https://open.feishu.cn/document/server-docs/im-v1/message/get.md):
  exact `message_id`, `user_id_type=open_id`, current sender/container/deletion fields.
- [Lark thread overview](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/thread-introduction.md):
  chat roots versus exact thread listing and visibility restrictions.
- [Received message content](https://open.larksuite.com/document/uAjLw4CM/ukTMukTMukTM/im-v1/message/events/message_content.md):
  flat post structures and media keys without required attachment size/name facts.

The regions differ: Lark documents tenant-token access and external-group
restrictions; Feishu additionally documents user tokens, external-sharing and
confidential-chat restrictions, plus optional original-card formats. The gateway
uses the common tenant-token native scope and makes no external-group or raw-card
parity claim. Permission/membership/thread-visibility errors refuse the operation.
Token lifetime, complete retention, stable equal-time ordering and snapshot
isolation are not guaranteed by the cited API. This is bounded live context, not
an archive, search API or deleted-message recovery.
