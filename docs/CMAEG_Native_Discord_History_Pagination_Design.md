# Native Discord conversation history pagination — source design

Status: implemented and locally validated, 2026-10-28. Deployment and native Discord acceptance remain pending. Base:
`f1d6fbf6a1bac21a417962ba8c75ae2a1357d7b4`. This document specifies the separate default-off implementation; it does not authorize
deployment, activation or platform sends.

## Problem and intended behavior

`readEventContext` currently has an explicit `include_thread_history` option.
It reads at most 20 text messages strictly before the original event inside an
existing native thread, with attachment counts and no pagination. Ordinary
native conversations have no history operation. The existing route, response,
source consent and gate retain that bounded contract.

Add a separate read operation for earlier messages in the exact conversation
that admitted one original native human event. A page can continue backward from
an opaque, authenticated cursor. An event inside a thread remains inside that
thread. An event in its parent never grants access to a child, sibling or other
channel. The operation returns current message text and bounded attachment
facts; it grants no reply, edit, reaction, membership, attachment-download or
other write authority.

This closes the lack of explicit backward pagination for eligible native Discord
conversations. It does not create a complete conversation archive, event-time
snapshot, platform search API or complete four-platform history implementation.

## Pinned evidence

Official semantics are pinned to
[`discord/discord-api-docs@7dfc89db5b951ec2fb2637a00d1422c341cef5d8`](https://github.com/discord/discord-api-docs/tree/7dfc89db5b951ec2fb2637a00d1422c341cef5d8).

| Evidence | Consequence |
| --- | --- |
| [Get Channel Messages](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/resources/message.mdx#L897): newest-to-oldest response; `before`, `after`, `around` are exclusive; limit 1–100 | Use only a server-derived `before`, fixed limit 26, and validate ordering and channel identity. |
| Same endpoint: no `READ_MESSAGE_HISTORY` can return no messages | An empty list alone cannot prove an empty conversation. Bracket page retrieval with access checks and report only the end of currently visible results. |
| [Get Channel Message](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/resources/message.mdx#L1048): guild reads require `VIEW_CHANNEL` and `READ_MESSAGE_HISTORY`; voice also requires `CONNECT` | Require successful reads of the exact original anchor before and after the page. Refuse deleted/inaccessible anchors. |
| [Message Content Intent](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/events/gateway.mdx#L487): affects REST content, embeds, attachments, components and polls; exceptions include app DMs, app-authored and app-mentioned messages | Empty text/attachments do not establish absence. Always describe content coverage as platform-returned only; source configuration is not proof of application approval. |
| [Snowflake pagination](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/reference.mdx#L167) | Keep IDs as strings on the wire; use checked unsigned 64-bit comparisons, never JavaScript `number`. |
| [Thread permissions](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/topics/threads.mdx#L67) | Bot membership/moderator visibility does not authorize private siblings. Use only the original authenticated thread. No join or unarchive is needed or attempted. |
| [Attachment structure](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/resources/message.mdx#L643) | Project bounded facts into a new type; omit URLs, proxy URLs and downloadable attachment references. |

The evidence directory is
`/private/tmp/cmaeg-discord-history-pages-research-20261028`. Its `sources.json`
records every exact URL, saved path and SHA-256. Existing source evidence:

- `api/crates/cmaeg/src/tools/thread_history.rs`: current opt-in, eligibility,
  eight-second deadline, current authority checks and response projection.
- `api/crates/cmaeg/src/discord/rest/thread_history.rs`: original thread/anchor
  preflight, fixed 20-row query, text clipping and attachment-count-only output.
- `api/crates/cmaeg/src/discord/thread_context.rs`: immutable authenticated
  channel type/guild/parent evidence, with no inference from a message's `thread`.
- `api/crates/cmaeg/src/discord/thread_authority.rs`: retained routed-plan,
  original actor, selected consumer and current custody checks used by lifecycle.
- `api/crates/cmaeg/src/discord/rest/component_channel.rs`: existing exact native
  DM, guild text/voice/announcement/stage and thread destination checks.
- `api/crates/cmaeg-core/src/seal.rs`: distinct authenticated token kinds,
  encryption and the 4,096-character token bound.

## Consent and eligible conversations

Introduce independent source `conversation_history: bool`, omitted when false,
and default-off `CMAEG_NATIVE_DISCORD_CONVERSATION_HISTORY_ENABLED`. Existing
`thread_history` and `thread_lifecycle` opt-ins never imply this broader consent.
The new consent must be present in both the admitted source snapshot and its
current exact source. Enabling it after an event was admitted requires a new
eligible event; old events do not silently acquire broader history scope.

Supported authenticated channel types are DM `1`, guild text `0`, voice `2`,
announcement `5`, stage `13`, and existing announcement/public/private threads
`10`–`12`. All require an actual retained ordinary human message and a successful
live anchor read. Group DMs `3`, categories, forum/media parent containers and
unknown channel types remain unsupported. An admitted thread under a forum/media
parent can be read as that exact thread, never as its parent or sibling posts.

Owner consent explicitly permits earlier context from other participants and
bot messages in the admitted conversation. Historical authors are not subjected
to new-message admission and do not launch work. The original human actor stays
bound to the authorization. This does not assert that the actor currently has
every Discord history permission: access uses the owner's exact bot, explicit
source consent and original admitted conversation, with Discord enforcing the
bot's current permissions. There is no inferred grant to another conversation.

Owner UI text must explain earlier messages, other participants, attachment
details, Discord content/permission limits and the absence of file downloads.
Expose the opt-in only when authoring capability confirms support; preserve
saved consent and allow opt-out. Normal native source gates still apply.

## API and discovery

Add `GET /v1/events/{event_ref}/history?cursor={optional_opaque_cursor}` and MCP
`readEventHistory({event_ref, cursor?})`. The only caller inputs are the original
EventRef and an optional issued history cursor. Reject unknown parameters,
duplicate cursors, raw platform IDs, limits, offsets, timestamps, search terms,
`before`, `after` and `around`. No ThreadRef or listed/created-thread target is
accepted. The EventRef is required on every page.

Rust adds `read_event_history`; TypeScript adds `readEventHistory`. Clients make
one page request per call, use normal read error behavior, and never auto-drain
history. Add public page/message/attachment/cursor/capability types, runtime
schemas, OpenAPI fixtures and wire examples. Optional event discovery describes
`conversation_history` with page size 25, backward direction, attachment facts
only and original expiry. Authoring discovery adds optional
`native_discord_conversation_history_enabled`. Older readers omit/ignore the
additive properties; the old context history operation does not expand. Optional
history discovery first checks native Discord/original-event/source consent using
the context-authorized channel, then bounds retained-plan enrichment to 100 ms.
Unrelated platforms and absent consent cause no enrichment storage or platform
reads. Slow/busy enrichment omits the hint; explicit reads retain fresh authorization
and the separate eight-second total deadline.

## Original authority and fresh checks

Use a separate `history_authority` path; lifecycle's guild-only loader and
`thread_lifecycle` consent cannot authorize this read. Reuse small pure evidence
validators, without broadening existing lifecycle or history behavior.

For every page, before remote reads:

1. Open the original EventRef and validate its type, version, original expiry and
   activity digest; reject component, hand-off, reaction and message-event lineage.
2. Resolve the current exact channel record and owner; authenticate the calling
   default/named consumer and its current key/provider/credential.
3. Reopen the immutable admitted plan for that owner, record and event. Require
   its exact selected target/EventRef/provider, original Activity and source;
   an ordinary human message must have generated it.
4. Revalidate the retained native dispatch, original human author, message ID,
   conversation/partition, native channel evidence and admitted source consent.
   Missing old evidence is unsupported; provider-supplied text cannot replace it.
5. Derive one exact origin identity: channel type/ID, optional guild and original
   parent only for a thread. Do not resolve or read another destination.
6. Calculate a domain-separated custody hash over the full logical event, owner,
   record, source, actor, origin, partition/conversation, selected consumer and
   provider, current agent-key/credential hashes and bot-token hash. A cursor must
   match it exactly. Secrets remain local and are never returned or logged.

Verify the current bot identity. Read the exact channel metadata and original
anchor; compare the retained type/guild/thread parent and original author/message
identity. The anchor may have edited content, but cannot become a different
author, webhook, system message or destination. Fetch one page. Read the same
channel and anchor again, then reload current owner/source/consumer/custody and
recheck original expiry before returning any page data or cursor.

Failure at any stage discards the page. No stale authorization fallback is used.
Permission changes can race reads; the result describes visible data fetched
during this request, not an atomic permission/content snapshot. Current authority
revocation, source opt-out, execution-gate disablement, changed consumer/provider,
credential changes and expiry invalidate continuation. Restarting without a
cursor still requires the retained original plan and current authorization.

## Cursor and pagination invariants

Add a distinct sealed history cursor token kind, proposed wire prefix `cmaeg1.hc`.
It grants continuation only together with the exact original EventRef and caller.
Its payload carries version, a SHA-256 of the complete canonical EventRef payload,
custody hash, original channel identity and anchor ID, next exclusive `before`
boundary, fixed page size, issued-at time and original EventRef expiry. Hashing
the logical EventRef avoids duplicating a large event inside the cursor and makes
equivalent randomized EventRef encodings refer to the same authority. Reject
cross-kind tokens, invalid sizes, future issue times and any renewed expiry.

The first boundary is the original message ID. Each later boundary must be a
positive valid snowflake strictly less than the original anchor. Every returned
raw row must name the exact channel, have a unique valid ID strictly below the
current boundary, and follow documented descending ID order. Malformed,
duplicate, foreign or out-of-range rows refuse the page.

Request fixed `limit=26`. Validate the complete provider response (at most 26 raw
rows), process the first 25, and use the extra row only to observe continuation.
The next boundary is the oldest **processed raw row**, including unsupported
message types; it is not the oldest projected message. This ensures progress
through system-message-only pages. No hidden fetch loop fills sparse pages.
If 26 rows were observed, return a cursor for the processed boundary; otherwise
report `end_of_visible_history`. A page may contain no projected messages and
still have a cursor. Lookahead is not a promise that the next row will still exist.

Return projected messages chronologically within each page; pages move backward.
Strict exclusive boundaries avoid overlap while following issued cursors.
Deleting the cursor-boundary message does not invalidate its numeric boundary;
deleting the original anchor does invalidate the operation. Newer messages never
move the upper bound beyond the original anchor.

Cursor stability means stable scope, expiry and numeric progress, not immutable
page contents. Replaying a cursor re-reads current Discord state; edits/deletions
can change results. No page snapshot/cache is stored. There is no additional
hidden total-message quota: each page is bounded, traversal remains subject to
the original event expiry and provider limits, and no complete export is promised.

## Response facts and truthful limits

The proposed `ConversationHistoryPage` contains original conversation identity,
anchor and requested boundary IDs (informational only), local `fetched_at`,
original `expires_at`, chronological `messages`, `scanned_messages`,
`omitted_message_count`, `text_truncated`, `metadata_truncated`,
`content_coverage: "platform_returned_only"`, and a tagged continuation of either
`more_visible_rows { cursor }` or `end_of_visible_history`.

For example, a successful page with continuation has this shape (IDs and token
are illustrative; omission and truncation fields remain explicit):

```json
{
  "conversation": {"id": "900", "kind": "group", "thread_id": "900"},
  "anchor_message_id": "1500",
  "before_message_id": "1500",
  "fetched_at": "2026-10-28T09:00:00Z",
  "expires_at": 1793179800,
  "messages": [],
  "scanned_messages": 25,
  "omitted_message_count": 25,
  "text_truncated": false,
  "metadata_truncated": false,
  "content_coverage": "platform_returned_only",
  "continuation": {"status": "more_visible_rows", "cursor": "cmaeg1.hc.opaque"}
}
```

An empty projected page can continue because the scanned rows were unsupported
system messages. Terminal continuation is
`{"status":"end_of_visible_history"}` and has no cursor.

Project ordinary/reply message types `0` and `19`. Preserve message ID, author ID,
bounded display name and truthful human/bot/unknown kind, text, original timestamp,
optional edit timestamp, and attachment facts. Webhook/system identity must never
be promoted to a human. Unsupported message types count as omitted. No result is
an Activity or EventRef, and nothing is routed or launched.
Tool descriptions and response documentation label all historical text and
metadata as untrusted reference data, never instructions or action approval.

Use a new `HistoryAttachmentFacts`, not the existing authority-bearing Attachment:
attachment ID, bounded filename, optional MIME type, size, optional dimensions,
duration, description and ephemeral flag when provided. Validate IDs and finite
nonnegative numeric facts. Omit URLs, proxy URLs, waveform, thumbnail, source,
reference, attachment locators and download tokens. MIME is descriptive metadata,
not proof that bytes were fetched or validated. Historical files are not fetched.

Record presence-only omission flags for embeds, components, stickers, polls and
forwarded snapshots, without projecting nested content, buttons or cross-channel
referenced messages. These flags mean Discord returned such content; restricted
fields can still conceal it. No recursive lookup of a reply/forward is performed.

Limits are fixed and discoverable: 25 scanned messages/page, at most 10 attachments
per projected message, 8 KiB text/message, 32 KiB total text/page, 16 KiB total
optional attachment/name text metadata, 1 MiB provider JSON and 256 KiB serialized
page output. Preserve record/attachment identities while clipping optional text;
set per-message and page truncation flags. Over-limit structures, oversized raw
JSON or an oversized final projection are explicit errors, never a partially
parsed success. These text limits can omit content even when pagination succeeds.

Missing Message Content access can produce empty content and attachment arrays.
The API never labels those empty values as proof that a message has no content,
and never labels `end_of_visible_history` as the beginning of all actual history.
No deleted-message recovery, event-time version recovery or atomic snapshot is
available from these endpoints.

## Runtime and error bounds

One eight-second deadline begins before authorization/current storage reads and
covers every platform read, final custody check, projection and cursor issuance.
Keep at most eight in-flight history reads per process, fail busy immediately,
use the fixed official Discord origin/no redirects, and respect existing shared
bot cooldowns. No writes, joining, unarchiving, detached task, automatic retry or
pagination loop is introduced. A cancelled request cannot later return data.

Use explicit invalid-input/invalid-cursor, unsupported-scope, forbidden/revoked,
expired, rate-limited/busy and unavailable errors. Deleted/inaccessible anchors
are refusals, not empty pages. Failure of the final anchor/custody check discards
an otherwise empty or populated response. No mutation ledger is needed.

## Verification before integration

- Core cursor tests: cross-kind rejection, exact logical event/custody binding,
  scope/type/parent checks, unsigned ID bounds, original expiry and length limits.
- Composed REST/MCP tests: DM, guild text/announcement/voice/stage and exact
  public/private threads; unsupported Group DM/forum parents; normal and named
  consumers; old evidence and each revoked owner/source/provider/key/credential.
- Pagination tests: 0/1/25/26 rows, sparse/system-only pages, exclusive monotonic
  boundaries, deletion of a boundary versus deletion of the anchor, edited
  messages, live replay changes, duplicate/foreign/out-of-order/newer rows and
  lookahead that disappears. Assert there is one list request per call.
- Authority tests: forged/raw destinations and cursors, lifecycle/reference
  cross-use, source consent independence, opt-in after original admission, final
  custody and permission revocation during reads, expiry during a request, and
  no downstream write/download/admission calls for any historical result.
- Projection tests: human/bot/webhook identities, clipped Unicode/metadata,
  attachment facts without URLs/references, content-restricted empty fields,
  omission flags, malformed/oversized bodies and exact serialized output cap.
- Eight-second entry-to-response bounds, no late detached work, concurrency,
  cooldown/429 behavior; SDK wire/schema inventory, UI opt-in preservation,
  additive OpenAPI freshness, legacy context-history regressions and file size.

Cargo is not authorized for this design task while Forms owns the shared slot.
Implementation and its validation follow review of this design and coordination
of the slot; no source rollout or connected native acceptance is implied.

## Remaining scope

Forward/latest/around reads, arbitrary date ranges and searches; complete export
or a durable consistent snapshot; deleted/revised historical versions; attachment
byte retrieval; full rich-content/poll/forward rendering; arbitrary channels,
private siblings and guild-wide enumeration; history-derived actions; actor-level
current Discord ACL evaluation; unsupported conversation types; and Telegram,
WhatsApp or Lark history APIs remain separate work. Connected Discord behavior
under the actual bot's entitlements and permissions requires authorized acceptance.
