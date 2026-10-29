# Self-service native Discord channels

A channel owner can choose **Direct Discord bot** in the channel editor, enter the
bot's numeric user ID and private bot token, select server messages and/or direct
messages, and configure sender/chat/group admission. This is an explicit native
source choice. Existing NyxID Discord relay sources keep their current behavior.
The token is write-only: blank fields retain the saved token; replacement sends a
new secret; public channel definitions and activation status never expose it.

The bot must already exist, be installed in the intended server, have access to
its channels and have Message Content Intent enabled in the Discord Developer
Portal. Native intake supports ordinary human messages and the existing text and
attachment paths. With the separate [rich-content writer gate](CMAEG_Native_Discord_Rich_Content.md),
new messages can also include descriptive sticker, embed and poll facts, including
messages without a plain-text caption. Bot messages are unsupported. Native buttons and single-select
callbacks require separate [operator enablement](CMAEG_Native_Discord_Interactions.md).
They support ordinary channels and verified existing threads with retained thread
proof and authenticated channel checks. Only single-shard bots are supported.
Proven thread behavior has its own contract and gates; ordinary conversation
partitioning is the baseline here.

## Deployment gates

All flags below default to `false` in code. The deployment manifest records the
desired enabled state after compatible-reader deployment and predecessor drain;
it is not evidence that production has already activated those values.
Enabling authoring does not itself open a
Discord connection. A staged deployment can continue using explicit canary
records with automatic discovery disabled.

| Configuration | Purpose |
| --- | --- |
| `CMAEG_NATIVE_DISCORD_WRITES_ENABLED` | Accept new or changed native Discord source metadata and bot credentials. |
| `CMAEG_NATIVE_DISCORD_ENABLED` | Run native connections and durable inbox recovery. |
| `CMAEG_NATIVE_DISCORD_DISCOVERY_ENABLED` | Discover eligible explicit native Discord sources from the current persisted channel index. Requires native runtime and routed recovery. |
| `CMAEG_NATIVE_DISCORD_RICH_CONTENT_ENABLED` | Capture versioned descriptive sticker/embed/poll facts for new human messages. Default off; requires compatible inbox readers and predecessor drain before activation. |
| `CMAEG_NATIVE_DISCORD_FORWARDS_ENABLED` | Capture bounded immutable forwarded-message descriptions. Default off; independent of rich content. Deploy compatible readers first. |
| `CMAEG_ROUTED_RECOVERY_ENABLED` | Install the durable routing scheduler, provider replay and delivery recovery. |
| `CMAEG_ROUTED_ADMISSION_ENABLED` | Accept new durable routed admissions. Keeping this off pauses new messages while retained work recovers. |
| `CMAEG_MULTI_CONSUMER_WRITES_ENABLED` | Permit creation of the default routing policy required by self-service native channels. |

Every active consumer's provider must configure the Responses v2 recovery
`response` and `events` paths and support that protocol. A newly authored native
source with an unsupported provider is rejected before provider binding or record
reservation. The UI advertises direct creation only after authenticated capability
discovery confirms authoring, routing authoring and automatic activation.
Discovery also reports optional `response_recovery_providers`. The portal uses
that list to explain provider incompatibility before saving a new native source;
it does not probe upstream health or change existing relay sources.

`CMAEG_NATIVE_DISCORD_CHANNEL_RECORDS` remains an explicit canary list of at most
64 canonical UUID v4 records. Its entries take selection precedence and still need
to appear in `CMAEG_ROUTED_CHANNEL_RECORDS`. Discovery does not add arbitrary HTTP,
Custom or relay channels to that routed allowlist. Only a verified native inbox
launch can use a dynamically discovered selection.

### NyxBot creator authorization and recovery

NyxID's [creator authorization contract](https://github.com/ChronoAIProject/NyxID/blob/main/docs/CHANNEL_EVENT_GATEWAY.md#nyxbot-gateway-creator-authorization-and-recovery)
uses a dedicated, server-only creator bearer. CMA retains its normal introspection
and owner checks. Register the gateway's actual active confidential OAuth client
by its full ID in the `cmaeg` service's `introspection_client_ids`. NyxID grants
only channel read/create/update/delete to this exact mint; channel testing and
administration remain separate. Never grant permissions by trusting the actor
string directly or substitute the channel's provider key for this creator bearer.

The NyxBot provider recovery paths are
`/api/v1/nyxbot/responses/{response_id}` and
`/api/v1/nyxbot/responses/{response_id}/events`. Verify the deployed provider
contract before registering them. Configuration alone is not end-to-end proof:
acceptance also needs a genuine creator flow, the dedicated channel key, durable
replay without a second turn submission, and a confirmed native return delivery.
An anonymous 401 or a legacy Custom-channel test does not establish those facts.

Existing NyxBot relay sources retain their selected transport. Move them only
through NyxID's explicit release/drain/reconnect flow; do not rewrite a live
source in place. NyxID's current native Discord response implementation rejects
attachments, so CMA native media availability does not imply NyxBot media support.

## Creation and recovery

Native creation requires a stable canonical UUID v4 `record_id`; the editor
already supplies one and retries the exact same request after uncertain results.
The durable creation flow seals a reservation before provider binding and sets
`creation.ready` only after binding succeeds. Native runtime requires that retained
original channel identity and a ready reservation.

When automatic discovery is enabled, a **new** explicitly authored native channel
receives a default-only routing policy before its reservation is persisted. The
implicit default consumer keeps its original profile, provider binding and
credential. Existing source records, routing policies and relay channels are not
rewritten. The routing and consumer editors can subsequently add named consumers
and addressing rules. Canary-only native creation continues to require explicit
routing configuration.

Creation retries preserve the fingerprint, record, binding and channel identity.
An exact existing reservation can finish or replay after authoring gates close;
changing the request under the same record identity remains a conflict.

## Bounded discovery and capacity

Each process scans at most 32 current channel-index heads per step, with a
five-second scan cadence and a cyclic cursor. It freshly resolves each candidate's
sealed definition, owner, ready original identity, native source/token authority,
routing and provider replay eligibility before selecting it. The console's private
owner index is not an execution authority.

Each process selects at most **64 native records**, including pinned canaries, and
supervises at most 64 source workers. Overflow remains unselected and is reported
explicitly. Removing a source, routing policy, ready identity or executable provider
eligibility releases discovered capacity after an exact current-record check.
Partial, empty or failed index pages do not establish deletion, and transient
storage failures do not revoke healthy unseen selections. The existing per-effect
source checks continue to fail closed. A restart rescans the current index; no
unbounded candidate cache or process-only channel identity is retained.

Replicas keep their own bounded selection. Their selected cohorts can differ after
restart or churn; the existing durable per-bot ownership lease still prevents two
workers from simultaneously owning the same bot/source. Selection capacity is not
a deployment-wide semaphore.

## Owner-visible activation status

Authenticated channel reads, creation responses and lists include
`native_discord_activation` only for native sources. It includes `state`, safe
`detail`, the process `capacity`, and `selected_records`. The source's **Check
configuration** action also reports current native activation. No status call
contacts Discord or sends an external message.

| State | Meaning |
| --- | --- |
| `selected` | This process selected the source for supervised intake. It does not attest bot authentication, socket health or delivery. |
| `pending_discovery` | Saved, eligible configuration awaits a discovery pass. Refresh the channel or check its configuration. |
| `capacity_exceeded` | Saved but unselected because the native selection is full; incoming native messages are not being received by this process for that channel. |
| `disabled` / `not_selected` | Native runtime is disabled, or automatic discovery is disabled and this record is outside the canary selection. |
| `creation_pending` / `routing_required` / `provider_unavailable` | A ready retained identity, routing policy or provider replay prerequisite is missing. |
| `admissions_paused` | Durable recovery is enabled, but new routed admissions are paused. |

Public capabilities add optional `native_discord_writes_enabled`,
`native_discord_auto_activation_enabled`, and `native_discord_capacity` under
`relay_authoring`. Older gateways omit these fields; the editor keeps saved source
settings and fails closed for new self-service choices.

Native lease fencing, encrypted credential custody, Identify quotas, reconnect
cooldowns, immutable inbox snapshots and original receipt timestamps are unchanged.
Activity receives the durable receipt before inbox processing or provider launch.
The local integration fixture exercises native inbox persistence, that Activity
receipt, the real routed provider adapter and executor, and a single Discord REST
reply. Live platform delivery and production activation require separate evidence.
