# Bridge connector capabilities

The [reserved text-control foundation](CHANNEL_CONTROLS.md) only persists safe
refusals while source controls are disabled. It does not change any catalog
capability, including `verified_actions` or `live_verified`, and cannot enable
chat approvals or automatic channel sends.

`GET /api/v1/bridge-connectors/capabilities` returns version 1 of the safe capability catalog. An optional `organization` query selects organization scope. The current owner or organization admin can read it, including when management or execution rollout switches are off. It uses no provider requests and exposes no bot/key inventory, custody identifiers, callback tokens, private URLs or credentials. Responses are private and must not be cached.

TypeScript callers use `client.bridges.capabilities({ organization })`. Rust callers use `client.bridges().capabilities(Some("organization-id")).await`. Existing validator implementations default to an empty version 1 catalog with `connection_validation_available: false`; SDKs reject an unknown catalog version. A missing platform or unknown capability provides no support guarantee.

The additive `placements` array describes `workspace_per_partition` and
`worktree_agent_per_partition` independently of connector stages. Each entry has
an exact `kind` and a typed `availability`:

| Availability            | Meaning                                                                                                                       |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `runtime_unavailable`   | The configured adapter is absent or lacks an existing shared-parent runtime requirement.                                      |
| `continuity_unverified` | The configured adapter meets the static requirements, but composed shared-parent upgrade and rollback continuity is unproved. |
| `available`             | Reserved for demonstrated placement availability; the current production composition never emits this state.                  |

The runtime requirements match existing reuse preflight: conversation support,
per-Agent homes, steer, interrupt, usage and resource observations. Meeting them
does not attest to retained live conversations, dirty work, concurrent siblings,
approvals or admitted turns surviving a control-plane upgrade and rollback.
There is no continuity override or new configuration switch. Enabling every
connector stage cannot change either unavailable placement state. Older servers
may omit `placements`; TypeScript preserves that absence and Rust defaults it to
an empty vector. Missing entries provide no shared-placement support. SDKs reject
unknown states or kinds, duplicate kinds, null and oversized placement arrays.

The dashboard can edit both placement shapes using existing public inventory,
effective-access and profile methods when the catalog permits the mode and its
connector. A new selection requires the exact Sandbox and, when applicable,
Workspace, current whole-parent management authority in the Bridge's owner
scope, published profile pins, explicit shared-filesystem/process/credential
consent, and both reserved-capacity values from 1 through 15. Inventory visibility
alone provides no management authority. New placement saves recheck current
catalog and target authority before sending the mutation; server admission
remains authoritative for profiles, limits and runtime compatibility.

Retained placement IDs, revisions, consent and reserves remain visible when
availability is missing. An unchanged placement can accompany unrelated settings
edits, and existing lifecycle controls remain usable. Once a mutation may have
been sent, its explicit retry preserves the original body and idempotency key to
recover that result even if the current catalog changes. Positive editor tests
use a scripted `available` catalog; they do not establish production availability
or close #864, #870 or #850.

`source_revision` identifies NyxID `85607255ee0beadca460a350e0aa6d177edd2fd2`. `evidence` distinguishes CMA protocol fixtures from upstream adapter source and the separate device ingest channel. `live_verified` is false for every current entry. Source review and protocol fixtures do not establish live delivery.

| Platform                | CMA text and identity fixtures                                   | Stable thread field                               | Current CMA text chunk limit | CMA reply format |
| ----------------------- | ---------------------------------------------------------------- | ------------------------------------------------- | ---------------------------- | ---------------- |
| Telegram / Telegram-new | Yes                                                              | Forum topic with verified topic/group metadata    | 4,000 UTF-16 units           | Escaped Markdown |
| Discord                 | Yes                                                              | Matching channel identity and thread channel type | 1,900 UTF-16 units           | Escaped Markdown |
| Lark                    | Yes                                                              | Message `thread_id`                               | 1,900 UTF-16 units           | Plain text       |
| Feishu                  | Yes                                                              | Message `thread_id`                               | 1,900 UTF-16 units           | Plain text       |
| Slack, WhatsApp, X      | Upstream source only; CMA bot setup unavailable                  | Unavailable                                       | Unavailable                  | Unavailable      |
| Device                  | Separate ingest channel; CMA bidirectional bot setup unavailable | Unavailable                                       | Unavailable                  | Unavailable      |

Telegram-new has its own upstream platform identity and is never normalized to Telegram for ownership or selected-binding validation. Telegram-new delegates the pinned Telegram boundary contract, has separate signed platform and partition identity, and uses its own default-off `CHRONO_BRIDGE_TELEGRAM_NEW_ENABLED` switch. Complete provider inventory can contain upstream-only bots and keys without blocking supported choices. Their rows still participate in ownership and shared-key conflict checks before selection is filtered.

Each `stages` value reports the current deployment switch combined with that platform's switch. `connection_validation_available` reports whether CMA has its connection verifier configured. These facts do not establish a particular Bridge's enabled state, active bot/key/routes, current owner/resource access, renewable authority, Agent readiness or required shared-context consent. Actual admission and effects continue to check those independently.

`output_text` and `output_progress` describe explicit channel reply tools in an active conversation created by the channel, under its current reply capability. They do not enable automatic final-answer forwarding, replies from ordinary pre-existing sessions, or automatic approvals. Text preparation neutralizes mentions, controls and selected markup before bounded chunks enter the ordinary outbox. `output_limit_utf16` counts each prepared chunk, after escaping. All current CMA `output_edit` values are false: the composed relay permits initial replies only, although pinned upstream Lark/Feishu has edit support. Outbound attachments are unavailable.

`reply_to_bot` requires the signed raw provider parent message and an exact permanent CMA Sent receipt for the selected bot. Generic mention text, a reply to any bot and generic thread context do not establish selected-bot addressing. `bot_mentions`, inbound edit/delete execution and verified interactive actions remain unavailable. Known edits/actions retain their typed refusals and cannot start an Agent turn.

Messages containing normalized attachment references or recognized raw media receive `attachment_download_required`; other nontext content in a verified original message receives `content_unsupported`. These are `PartitionRefusal` values, not successful downloads or model input. Bot, edit, action and loop checks retain their precedence. The entire message is refused rather than dropping its media and executing its text. Refused callbacks retain no reply credential or protected interaction context. Attachment references remain inert and subject to the existing count, metadata, URL syntax and payload bounds; no URL fetch, private-platform credential use, OCR, audio interpretation or attachment custody is implemented by this change.

The signed unsupported-event fixtures cover Telegram, Telegram-new, Discord, Lark and Feishu with both ordinary text and reserved-control-looking normalized text. They exercise edits, deletion-shaped and unknown events, unsupported content, bot/echo events, missing raw evidence, inconsistent event discriminators and raw/normalized text, sender and chat disagreement. Lark/Feishu app senders are refused as bots; no additional loop signal is inferred. Synthetic unsupported event names prove refusal only and do not establish that NyxID emits those callbacks or supports platform deletions.

Telegram updates containing both a message and a callback query are malformed. Contact, location, venue, poll, dice, game, invoice and successful-payment payloads cannot become text input merely by retaining a text field. The same rules apply independently to Telegram-new. Existing original text, provider identity and alias handling retain their ordinary behavior.

HTTP and disposable-Mongo fixtures verify exact denied-receipt replay and no accepted or claimable inbox work, control admission, Agent command or outbound effect. A `partition_denied` receipt can still be returned as `inbox_persisted`; this records a refusal and does not admit an Agent message. Malformed signed input fails before persistence. These fixtures do not prove live delivery, attachment retrieval or unsupported platform enablement.

Existing inbox records retain their original admission, source hash and normalization facts; replay does not reclassify them. The new refusal/event enum values are additive for current readers, but pre-change readers may reject records containing those values. Mixed-writer and rollback compatibility must be verified under #850 before rollout; this document does not claim that verification has passed.

Issue #867 remains incomplete until missing platform adapters and their personal/group/thread/identity/edit/delete fixtures, actual disposable Telegram and Lark/Feishu live receipts, authorized bounded attachment retrieval with network/redirect protections, owner-scoped artifact custody/expiry/read grants and capability-supported Agent input are implemented and verified. The device channel remains outside bidirectional bot parity.
