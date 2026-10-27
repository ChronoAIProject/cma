# CMA Event Gateway operations

## Durable recent activity

Public channel activity uses independent encrypted `channel_recent_activity_v1`
records in the existing `eg_interaction_records` collection. No collection,
credential, environment variable or external service is added. Each owner/channel
has deterministic daily records, bounded to 50 rows and 44,000 plaintext bytes;
existing interaction TTL removes each daily record at its fixed seven-day
boundary. Existing routed recovery ignores this separate record kind. The
original admission, execution, reply authority and runtime records are unchanged.

Incoming authenticated work commits its receipt before starting a provider turn.
Failure to persist that receipt refuses launch with
HTTP 503 with `provider_unavailable` and detail `channel_activity_unavailable`;
retry retains the original event identity.
Progress observations occur only at meaningful lifecycle boundaries and actual
transport outcomes, never on each text delta. A failed progress observation is
logged and must not repeat an accepted provider turn or confirmed platform send.
Each progress observation has a three-second deadline, so storage failure cannot
leave the execution or delivery worker indefinitely blocked on activity tracking.
Absence of a timestamp is not evidence that the corresponding action occurred.

The current single gateway replica publishes notifications after committed
activity updates. A new or reconnected SSE reader restores authoritative daily
snapshots, including after a process restart; no process-local ring is the source
of activity truth. GET/SSE snapshots are bounded recent activity, not an archival
ledger. Future multiple-replica service must add shared commit notification before
claiming the same immediate push guarantee across replicas.

## Public documentation

The gateway itself serves a single `/docs` document combining the integration
guide, architecture and complete protocol, with section navigation and a
`/docs/download` Markdown download. The former `/docs/architecture` and
`/docs/protocol` addresses redirect to their sections. `/openapi.json` remains
the machine-readable contract. Individual source articles are also available
verbatim at `/docs/integration.md`, `/docs/architecture.md` and
`/docs/protocol.md`. They require no login, JavaScript or external assets.
The build embeds exactly the reviewed integration, architecture and protocol
Markdown files; operations runbooks are not part of the public bundle.
`infra/cmaeg/Dockerfile` copies those inputs for the binary build. Documentation
updates therefore roll out with the gateway image, independently of CMA web.

Service flow diagrams are reviewed static SVG assets under
`api/crates/cmaeg/assets/docs`. Regenerate them with
`.github/scripts/render-cmaeg-diagrams.py` using Mermaid CLI 11.17.0 after editing
Mermaid source blocks; `--check` verifies source freshness in CI. The browser
loads local SVG images under the gateway's restrictive content security policy.
The complete Markdown download retains the Mermaid sources and internal links.

Production catalog display name: **Agent Event Gateway**. NyxID slug: `cmaeg`.
The catalog REST base is `https://bot.chrono-ai.fun/event-gateway/v1`, with OpenAPI at
`https://bot.chrono-ai.fun/event-gateway/openapi.json`. The `/v1` base is necessary
because the NyxID OpenAPI parser discovers paths but does not apply `servers`.
The public/internal service is auto-connected and forwards caller bearers;
channel ownership and credential-specific gateway checks remain authoritative.

## Independent Gateway storage and retained production features

The canonical `cma-event-gateway` deployment retains the Console and inbound
media behavior from live source `976e877379038cec7c76ab185f5b99d2f099dba1`.
Console OAuth continues to use `client_secret_post`, its own session cookie,
owner checks and CSRF forms. The shared web portal uses the existing Agent
Manager browser login through its credential-custody bridge; a Console cookie
does not authorize the portal API. Channel Activity's exact read routes also
use that bridge, with immediate SSE forwarding and fresh authentication on
reconnect. Media remains enabled with the 2 GiB
memory request/limit and 32 MiB media volume. The empty speech-to-text URL and
FFmpeg directory keep transcription and transformation disabled, as in the
live configuration.

When the independent storage settings are supplied, every channel, connection
and interaction record goes to the Gateway's `eg_*` collections, including
Console listing/reads and both operator audit commands. Agent Manager no
longer serves the old record endpoints. Before replacing the old deployment,
import all three legacy record domains with the independent migration command
and compare IDs, owners, versions, expiry and sealed blob bytes. Keep the
original master keys and `CMAEG_CMA_BASE_URL=http://cma-api.chrono-bot.svc`:
existing sealed pointers bind that exact origin even though persistence now
uses Mongo. Provider dispatch uses the canonical Agent Manager Service.

Quiesce all legacy Gateway writers, including inbound interaction handling,
until import and verification finish. With a temporary read-only legacy Mongo
credential and the correct database selected in `mongosh`, set
`CMA_GATEWAY_EXPORT_FILE` to a new local path and load
`.github/scripts/export-gateway-records.js`. The exporter requires all three
legacy collections to exist, even when empty. It validates schema version 1,
preserves exact record revisions and sealed bytes, and translates interaction
BSON Dates to the importer's whole-second expiry values without rounding.
It publishes a mode-0600 NDJSON file only after the full export succeeds and
refuses to overwrite an existing path. A failed export removes its private
staging directory; after a killed process, remove any abandoned
`.cma-gateway-export-*` directory only after confirming that export has stopped.
Run `cmaeg --import-legacy-records PATH` with the destination migration
configuration, then verify every live record. Expired interactions remain in
the export for comparison but the importer deliberately skips them. Keep the
export protected as sealed production data and remove it after verification.

The local authenticated Mongo contract starts the real Gateway executable
with Console and media enabled, stops the old record server, then verifies
creation/list/delete, Console OAuth/read/delete visibility, media tool discovery,
API authentication and Console CSRF refusal. It complements the media/Console
suites; Linux seccomp and image self-tests still run in native Linux CI. The
shipped image must pass the current source's CI and image gates before rollout.

## Startup

The wire authority is [CMAEG protocol](CMAEG_Protocol.md). Configuration is loaded
once before binding; invalid values fail startup. Restart through the drain
procedure to apply changes. Configuration is operator-owned.

Before loading configuration, the gateway builds its Tokio runtime and starts
16 blocking threads that it keeps for its whole lifetime. When the OS refuses
new threads, for example at the pod PID limit, DNS lookups and file IO queue on
those threads until one is free instead of stalling until thread creation
recovers. If the 16 threads do not all start within ten seconds, the gateway
logs `CMAEG blocking pool prewarm failed` and exits 1 before any listener
binds. `--print-openapi` builds no runtime. This behaviour is unconditional; it
applies with all media flags off. `--audit-custom-attachments [--clear]` and
`--audit-routing [--clear [--allow-default-widening]]` are operator commands,
not serving modes: each builds the same runtime, binds no listener, starts no
worker and exits when its report is printed (see
[Rolling back below inbound media](#rolling-back-below-inbound-media) and
[Rolling back below multi-consumer routing](#rolling-back-below-multi-consumer-routing)).

The gateway makes itself non-dumpable (`PR_SET_DUMPABLE=0`) in every mode, as
its first action after reading its arguments and before any logging or runtime.
The files under its `/proc/<pid>` are then owned by root (the directory itself
keeps uid 10001), so `/proc/1/environ` (the master keys and service tokens)
and `/proc/1/mem` are unreadable from
`kubectl exec` and from debuggers, it cannot be ptraced by a same-uid process,
and core dumps are disabled. If the kernel refuses, the process prints
`cmaeg: cannot make the process non-dumpable: <error>` and exits 1. **This
applies with all media flags off.**

## Configuration

| Environment key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `CMAEG_LISTEN` | socket address | `0.0.0.0:8090` | CMAEG listen address. |
| `CMAEG_METRICS_LISTEN` | socket address | `0.0.0.0:9091` | Separate Prometheus scrape listener; `/metrics` is absent from the public router. |
| `CMAEG_PROVIDERS` | JSON array | `[]` | Strict JSON provider allowlist described above. |
| `CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED` | `true` or `false` | `false` | Allows new scoped `bot_messages: native_mentions` policies and advertises `native_bot_mentions_v1` through authenticated channel capability discovery. Only exact `true`/`false` are accepted. Upgrade all gateway readers and consuming applications before enabling it. Turning it off prevents new opt-ins but retains existing policy execution; see [Native bot mention rollout](#native-bot-mention-rollout). |
| `CMAEG_MULTI_CONSUMER_WRITES_ENABLED` | `true` or `false` | `false` | Allows policy-only routing edits after all replicas and rollback targets understand stored routing. Does not provision consumers or enable fanout execution. |
| `CMAEG_LARK_RESOURCE_WRITES_ENABLED` | `true` or `false` | `false` | Allows new/changed exact Lark/Feishu embedded resource bindings after all gateway and consuming readers support them. Retained bindings/removal remain supported with writes off. Also needs inbound media and an independently confirmed receiving bot plus an already authorized exact bot-API UserService. CMA follows no redirects to NyxID; NyxID remains the downstream credential/transport boundary. No automatic setup or live acceptance is implied. See [resource bindings](CMAEG_Protocol.md#opt-in-lark-and-feishu-embedded-resource-binding). |
| `CMAEG_RELAY_RAW_VERIFICATION_WRITES_ENABLED` | `true` or `false` | `false` | Allows new or changed Discord/WhatsApp raw verification independently from dynamic source topology. Enable only after all readers and rollback targets support it. Exact retained settings remain usable when disabled; no automatic trust downgrade. |
| `CMAEG_NATIVE_DISCORD_WRITES_ENABLED` | `true` or `false` | `false` | Allows native Discord source/credential writes after all readers and rollback images support them. Authoring alone does not activate sockets; the native runtime, routed execution and selection prerequisites remain required. |
| `CMAEG_NATIVE_DISCORD_EDITS_ENABLED` | `true` or `false` | `false` | Issue native Discord single-part text/link edit references after all readers and rollback images understand the optional exact destination. Disabling issuance preserves retained references. |
| `CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED` | `true` or `false` | `false` | Issue 24-hour delete/reaction authority on confirmed native Discord single-part text/link replies; requires native edit issuance too. Deploy compatible readers before enabling. Retained references remain readable with issuance off. Reactions change only the bot’s own reaction; custom emoji IDs resolve in the destination guild. |
| `CMAEG_NATIVE_DISCORD_THREAD_HISTORY_ENABLED` | `true` or `false` | `false` | Allows new native source `thread_history` opt-ins and explicit bounded `readEventContext` history reads after compatible readers are deployed. Both this gate and source consent are required for execution. Off revokes history reads, retained settings can be removed; ordinary context reads are unchanged. See [native thread history](CMAEG_Protocol.md#bounded-native-discord-thread-history). |
| `CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED` | `true` or `false` | `false` | Persists verified native channel metadata in immutable inbox bodies and issues exact-thread reply evidence. Deploy compatible inbox and sealed-reference readers first. This uses one bounded authenticated channel read per new admitted message, with no per-channel cache; duplicate inbox messages reuse their original evidence. Disabling stops new metadata writes but retained evidence continues to replay. |
| `CMAEG_NATIVE_DISCORD_ENABLED` | `true` or `false` | `false` | Enables supervised native source connections and durable inbox recovery for explicit canaries or bounded automatic discovery. Off starts no native worker, scans or Discord requests. Authoring remains separately gated. |
| `CMAEG_NATIVE_DISCORD_DISCOVERY_ENABLED` | `true` or `false` | `false` | Discovers eligible ready native sources from the persisted channel index in pages of 32, with at most 64 selections/workers per process. Requires native runtime and routed recovery. Explicit canaries take precedence; dynamic selection authorizes verified native inbox admission only. It does not activate arbitrary HTTP, Custom or relay routing. |
| `CMAEG_NATIVE_DISCORD_CHANNEL_RECORDS` | comma-separated canonical UUID v4 IDs | empty | Initial bounded rollout selection, at most 64 records. When enabled, every selected record must also be in `CMAEG_ROUTED_CHANNEL_RECORDS` and routed recovery must be on. Sources need their original ready channel identity and an explicit routing snapshot. This staged allowlist is not automatic source discovery. |
| `CMAEG_DYNAMIC_SOURCES_WRITES_ENABLED` | `true` or `false` | `false` | Allows extended source lists and partition shapes after all readers support them. Existing settings may carry through unchanged. Does not enable Discord/WhatsApp raw verification. |
| `CMAEG_THREAD_CONTRACT_WRITES_ENABLED` | `true` or `false` | `false` | Advertises and permits new or changed native-thread contracts after all gateway readers and the NyxID native thread-reply endpoint support them. Keeping it false preserves ordinary channel writes and execution of retained contracts; removing a contract remains allowed. Only exact `true`/`false` are accepted. See [Native thread rollout](#native-thread-rollout). |
| `CMAEG_MEDIA_TOOL_PROVIDERS` | comma-separated provider slugs | `cma_codex` when that provider is configured, otherwise empty | 0..=8 distinct slugs, each naming a `CMAEG_PROVIDERS` entry; startup fails otherwise. Profile bindings of these providers install the gateway MCP tools, so only their agents are told how to open an attachment through a gateway tool. Set it to the empty string to render no tool hints. A separate key because provider entries refuse unknown fields. |
| `CMAEG_NON_HUMAN_PROVIDERS` | comma-separated provider slugs | `cma_codex` when that provider is configured, otherwise empty | 0..=8 distinct slugs, each naming a `CMAEG_PROVIDERS` entry; repeated slugs collapse, and startup fails otherwise, naming the key. Whose agents answer custom events and hand-offs by default: a consumer of a channel with routing that has no `non_human` field answers them only when its provider slug is listed here. A consumer's `non_human` field overrides this per consumer. A CMA consumer bound with `source_user` session affinity needs `non_human:false`, because CMA selects its agent from the verified human sender, which a custom event or hand-off does not have. Aevatar (D-c): add its slug only once its endpoint answers non-human activities. Set it to the empty string to name no provider. The routing audit reads it to decide whether `default` answers custom events (see [Rolling back below multi-consumer routing](#rolling-back-below-multi-consumer-routing)). A separate key because provider entries refuse unknown fields. |
| `CMAEG_ROUTED_RECOVERY_ENABLED` | boolean | `false` | Installs the durable admission adapter, replay executor, tool receipts and supervised recovery scheduler. Required before selecting records or enabling admission. Keep enabled while retained routed work can execute or retry. A stopped recovery worker removes readiness; shutdown uses the existing turn drain deadline. See [durable routed execution](CMAEG_Routed_Execution.md). |
| `CMAEG_ROUTED_ADMISSION_ENABLED` | boolean | `false` | Allows new routed plans only for explicitly selected records. Set false to pause new admissions with 503 while exact committed retries and recovery continue. This does not disable the channel's legacy path when no routing policy exists. |
| `CMAEG_ROUTED_CHANNEL_RECORDS` | comma-separated explicit record IDs | empty | At most 64 IDs; no wildcard. Unselected ingress keeps its existing launcher without admission-store reads. Retain selected IDs while their durable identities can be retried; removing selection is not a safe rollback. Provider replay capability must be explicitly configured and proved before a new routed plan is accepted. |
| `CMAEG_MEDIA_ENABLED` | `true` or `false` | `false` | Master switch for inbound media. Exactly `true` or `false`; any other value, including `True` or `1`, fails startup. Off, every media message is refused with 202 `attachment_unsupported`, and every Lark or Feishu rich-text post with `content_unsupported`, exactly as before media existed. On, authenticated Telegram media from the NyxID relay and the native webhook, and Lark and Feishu image, file, audio and media/video messages from the relay, are admitted as activity attachments, and Lark and Feishu posts as text the observer flattens from the raw event (protocol §6); Lark stickers stay refused. It also governs custom sources: off, management refuses a custom `attachments` setting with `custom_attachments_disabled` and ingress ignores a stored one; on, the setting can be stored and the items it names in custom events are admitted as attachments. Enabling it requires the `cmaeg` container's memory request to equal its limit at 1536Mi or more; `tests/infra_manifests.rs` enforces this. |
| `CMAEG_MEDIA_MAX_BYTES` | unsigned decimal, bytes | `20971520` | 1..=26214400. Most bytes fetched for one attachment; Telegram and NyxID serve at most 20 MiB themselves. Validated at startup even while `CMAEG_MEDIA_ENABLED` is `false`, like every media budget below; an out-of-bounds or non-decimal value fails startup naming the key. |
| `CMAEG_MEDIA_INFLIGHT_BYTES` | unsigned decimal, bytes | `67108864` | `CMAEG_MEDIA_MAX_BYTES`..=1073741824. Pod-wide bytes held at once by fetched originals and transform outputs; a reservation larger than the budget takes all of it, so one maximum-size original can always proceed. |
| `CMAEG_MEDIA_RESPONSE_BYTES` | unsigned decimal, bytes | `25165824` | 4194304..=134217728. Pod-wide weighted bytes of media response bodies in flight. Each body releases its share at a hard deadline even when its reader never polls, so a stalled client cannot hold it until the load balancer's 660-second timeout. |
| `CMAEG_MEDIA_MAX_JOBS` | unsigned decimal | `6` | 1..=64. Pod-wide concurrent media jobs. Each channel runs at most two at once, so one channel cannot hold every job permit. |
| `CMAEG_MEDIA_CACHE_BYTES` | unsigned decimal, bytes | `33554432` | 0..=268435456. Capacity of the per-pod derived-media cache, weighted by bytes, with a 30-minute lifetime; `0` disables caching. |
| `CMAEG_MEDIA_RATE_PER_MINUTE` | unsigned decimal | `60` | 1..=6000. Media fetch starts per channel per minute, with a burst of the same size. |
| `CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR` | unsigned decimal, bytes | `268435456` | 20971520..=4294967296. Download bytes per channel per hour, counted in KiB and charged by `GET /v1/media/content` before an original's first byte; a spent quota answers `429 rate_limited` with `Retry-After: 60`. |
| `CMAEG_MEDIA_LINK_TTL_SECONDS` | unsigned decimal, seconds | `120` | 30..=600. Lifetime of a media download link minted by `readEventAttachment` with view `download`, never past its event's own expiry. A link is sent to the model provider in the tool result and stored by CMA as the shell tool call's title, so it must expire quickly; an out-of-bounds or non-decimal value fails startup naming the key. |
| `CMAEG_MEDIA_PREPARE_BUDGET_MS` | unsigned decimal, ms | `15000` | 1000..=60000, and below `CMAEG_TURN_TIMEOUT_MS`; startup fails naming both keys otherwise. How long a media turn waits for its eager items (inline transcripts and text excerpts) before its provider turn starts without them. Items not ready by then render `transcript is still being prepared` and keep running in the background. See [Media turn preparation](#media-turn-preparation). |
| `CMAEG_MEDIA_FFMPEG_DIR` | absolute directory path | `""` | Empty disables the media transformer: videos keep platform thumbnails and transcripts the direct upload path. Otherwise an absolute, canonical directory owned by root and not group- or other-writable, holding regular, root-owned, executable `ffmpeg` and `ffprobe` files (no symlinks); startup checks all of this. A non-empty value fails startup on any platform but linux/amd64. The transformer runs only while `CMAEG_MEDIA_ENABLED` is `true`; set while media is off, it is ignored with a warning. Configured, it requires `CMAEG_MEDIA_INFLIGHT_BYTES` ≥ `CMAEG_MEDIA_MAX_BYTES` + `CMAEG_STT_MAX_BYTES` (startup fails naming the three keys), a `sleep` preStop hook, and a memory request equal to a limit of at least 2Gi; `tests/infra_manifests.rs` enforces the last two. See [Media transformer](#media-transformer). |
| `CMAEG_MEDIA_VIDEO_FRAMES` | unsigned decimal | `6` | 1..=6. Keyframes extracted per video while the transformer is configured. |
| `CMAEG_STT_MAX_SECONDS` | unsigned decimal, seconds | `600` | 1..=1500. Most metered seconds one transcription call may cost. With `CMAEG_STT_MAX_BYTES` it sets the direct-upload cap, `min(CMAEG_STT_MAX_BYTES, CMAEG_STT_MAX_SECONDS × 4000)` bytes. Like every speech-to-text key, validated at startup even while speech-to-text is disabled; an out-of-bounds or non-decimal value fails startup naming the key. See [Speech-to-text spend](#speech-to-text-spend). |
| `CMAEG_STT_MAX_BYTES` | unsigned decimal, bytes | `20971520` | 1048576..=24000000, below the speech-to-text API's 25 MB upload limit. Most bytes one upload may carry. |
| `CMAEG_STT_RATE_PER_MINUTE` | unsigned decimal | `20` | 1..=600. Transcription calls per channel per minute, with a burst of the same size. |
| `CMAEG_STT_CHANNEL_SECONDS_PER_HOUR` | unsigned decimal, seconds | `3600` | 600..=86400. Metered seconds per channel per hour, checked before the global budget. Startup fails naming all three keys unless `CMAEG_STT_MAX_SECONDS` ≤ `CMAEG_STT_CHANNEL_SECONDS_PER_HOUR` ≤ `CMAEG_STT_SECONDS_PER_HOUR`. |
| `CMAEG_STT_SECONDS_PER_HOUR` | unsigned decimal, seconds | `36000` | 600..=360000. Metered seconds per hour across every channel: the cost backstop, at most about $2.70 per hour at $0.0045 per minute. |
| `CMAEG_STT_EAGER_MAX_SECONDS` | unsigned decimal, seconds | `300` | 0..=600. Audio whose *declared* duration exceeds this is not transcribed before its turn starts; undeclared audio is. `0` disables eager audio transcripts. Declared durations decide eligibility only and are never charged: the spend limits above still meter every upload. |
| `CMAEG_STT_URL` | URL | `""` | Speech-to-text endpoint, speaking the OpenAI transcription protocol. Empty disables speech-to-text: transcripts are refused as `transcriber_disabled` without fetching, and `CMAEG_STT_API_KEY` is ignored. Otherwise an absolute `https` URL with a host and no userinfo, query or fragment; `http` only with a loopback host, for local tests. Startup errors never print the URL. See [Speech-to-text endpoint](#speech-to-text-endpoint). |
| `CMAEG_STT_MODEL` | string | `gpt-transcribe` | 1..=128 printable ASCII characters without whitespace; sent as the multipart `model` field. |
| `CMAEG_STT_FORMATS` | comma-separated formats | `mp3,mp4,mpeg,mpga,m4a,wav,webm` | 1..=9 distinct values from `mp3`, `mp4`, `mpeg`, `mpga`, `m4a`, `wav`, `webm`, `ogg` and `flac`, without spaces or empty entries. `mpeg` and `mpga` also admit the MP3 container. `ogg` covers Telegram `.oga` and Lark `.opus` voice; add it only after probe P3 passes. A container not listed is refused as `unsupported_format` without a request. |
| `CMAEG_STT_TIMEOUT_MS` | unsigned decimal, ms | `30000` | 1000..=120000. Deadline of one request attempt, within the derivation's own deadline. |
| `CMAEG_STT_API_KEY` | secret string | Required when `CMAEG_STT_URL` is set | Platform speech-to-text key from the optional `cmaeg-stt` Secret, sent only to `CMAEG_STT_URL` as a bearer token and never logged. 1..=4096 printable ASCII characters without whitespace. Never a tenant's key. |
| `CMAEG_PROVIDER_CONNECT_TIMEOUT_MS` | positive integer, ms | `2000` | TCP/TLS connection timeout for each provider pool. |
| `CMAEG_PROVIDER_REQUEST_TIMEOUT_MS` | positive integer, ms | `30000` | Deadline for provider setup and probe requests, and receipt of streaming HTTP response headers. Active streams are owned and bounded by their consumer. |
| `CMAEG_PROVIDER_MAX_INFLIGHT` | positive integer | `256` | Deployment-wide upper bound applied to each provider's semaphore. Calls wait at most 50 ms for a provider permit before returning a typed busy result. |
| `CMAEG_PUBLIC_URL` | origin URL | Required | Retained console OAuth/CSRF origin and legacy callback authority, without a trailing slash. Production `https://bot-gateway.chrono-ai.fun`. HTTPS, or loopback HTTP for local tests. |
| `CMAEG_PUBLIC_API_URL` | public base URL | `CMAEG_PUBLIC_URL` | Canonical API, setup, webhook and media URL base. Production `https://bot.chrono-ai.fun/event-gateway`. HTTPS with clean optional path segments; no credentials, query, fragment or trailing slash. Loopback HTTP is allowed for local tests. |
| `CMAEG_ADVERTISE_PUBLIC_API_URL` | `true` or `false` | `true` | First-rollout switch: `false` keeps generated URLs and discovery on the legacy public origin while canonical routes, exact Origin checks and MCP compatibility remain available. Set `true` only after every serving replica supports the canonical namespace and the public routes have been verified. |
| `CMAEG_MASTER_KEYS` | secret string | Required unless file source is set | Kubernetes Secret environment value containing `kid:base64key[,kid:base64key]`; each key decodes to 32 bytes and the first key is current. |
| `CMAEG_MASTER_KEYS_FILE` | file path | Optional alternative | Compatibility file source for the same key ring; mutually exclusive with `CMAEG_MASTER_KEYS`. |
| `CMAEG_CMA_BASE_URL` | origin URL | Required | CMA record-store origin. HTTPS, or loopback/`.svc` HTTP; no credentials, path, query, fragment, or trailing slash. |
| `CMAEG_CMA_SERVICE_TOKEN` | secret string | Required unless file source is set | Kubernetes Secret environment value authenticating inbound gateway rekey. Also used for outbound CMA authentication when the service-account client pair is absent. |
| `CMAEG_CMA_CLIENT_ID` | string | Optional pair | Dedicated NyxID service-account client ID, injected from Kubernetes Secret. Must be set together with its client secret. |
| `CMAEG_CMA_CLIENT_SECRET` | secret string | Optional pair | Exchanges renewable outbound CMA tokens at the configured NyxID origin using `client_credentials` and `openid roles`. The account role grants `cma:gateway:records` and `cma:provider:bindings`. |
| `CMAEG_CMA_SERVICE_TOKEN_FILE` | file path | Optional alternative | Compatibility file source for the same bearer; mutually exclusive with `CMAEG_CMA_SERVICE_TOKEN`. |
| `CMAEG_NYXID_BASE_URL` | origin URL | Required | NyxID origin for creator authentication, with the same URL restrictions as the CMA origin. |
| `CMAEG_MONGODB_URI` | connection URI | Required by the management integration tests and the independent gateway store | MongoDB connection for Event Gateway channel and credential records; keep it in a Secret. |
| `CMAEG_MONGODB_DATABASE` | database name | Required by the management integration tests and the independent gateway store | Database name for Event Gateway records. |
| `CMAEG_MONGODB_AUTHORITY_MODE` | `isolated` or `shared-schema` | Defaults to `isolated`; production uses `shared-schema` in `chrono-bot` | Shared mode admits a database-scoped credential while migrations and stores remain confined to `eg_*` collections. |
| `CMAEG_NYXID_CLIENT_ID` | string | Required with independent Gateway storage | Reuses Agent Manager's NyxID client ID through its Deployment ConfigMap reference; the Gateway independently verifies the forwarded bearer. |
| `CMAEG_NYXID_CLIENT_SECRET` | secret string | Required with the client ID | Reuses Agent Manager's client secret through a Secret reference; never put it in a ConfigMap. |
| `CMAEG_WEB_ORIGIN` | legacy name | Unused by the shared portal | Retained compatibility constant; exact-origin browser checks now run in Agent Manager's portal bridge. |
| `CMAEG_COOKIE_KEY_FILE` | legacy name | Unused by the shared portal | Retained compatibility constant; the portal uses the existing CMA browser session and no separate Gateway cookie key. |
| `CMAEG_TOOLS_NYXID_SLUG` | string | `cmaeg` | NyxID service slug in the provider tools advertisement, matching `[a-z0-9_]{1,32}`. |
| `CMAEG_CHANNEL_CACHE_TTL_MS` | positive integer, ms | `60000` | Fresh lifetime of a resolved channel on this replica. |
| `CMAEG_CHANNEL_NEGATIVE_TTL_MS` | positive integer, ms | `10000` | Lifetime of a cached CMA record-not-found result; expired negative entries are never served. |
| `CMAEG_CHANNEL_CACHE_MAX` | positive integer | `50000` | Maximum positive and negative channel entries on this replica. |
| `CMAEG_RESTORE_RATE_PER_MINUTE` | positive integer | `10` | Per-channel restore refill rate, with an initial burst of three. |
| `CMAEG_RESTORE_TIMEOUT_MS` | positive integer, ms | `800` | Total cold-restore deadline, inside the record client's 1.5-second HTTP deadline. |
| `CMAEG_JWKS_TTL_MS` | positive integer, ms | `600000` | Fresh lifetime of relay signing keys; at most one refresh every five seconds per retained URL. |
| `CMAEG_INGRESS_MAX_BODY` | positive integer | `1048576` | Callback body limit, configurable from 1 byte through 16 MiB. |
| `CMAEG_INGRESS_INFLIGHT` | positive integer | `256` | Concurrent callbacks per replica; overflow returns immediate 429 with `Retry-After: 1`. |
| `CMAEG_ACCEPT_BUDGET_MS` | positive integer, ms | `20000` | Time from the last inbound body byte to acknowledgment; a pending provider request continues after the gateway returns 202. |
| `CMAEG_TURN_TIMEOUT_MS` | positive integer, ms | `600000` | Total turn lifetime including provider setup, retry and stream consumption. |
| `CMAEG_NYXID_REPLY_TIMEOUT_MS` | positive integer, ms | `5000` | NyxID reply connect/send/read timeout; uncertain sends are never blindly retried. |
| `CMAEG_OUTBOUND_MESSAGES_WRITES_ENABLED` | boolean | `false` | Enables owner-reviewed preparation and explicit commit to an existing authorized relay destination. Does not grant native media, templates or interaction authority. Retained receipts and pre-commit cancellation remain available when disabled. |
| `CMAEG_NYXID_COMPLETE_REPLIES` | boolean | `false` | `true` selects one complete ordinary relay request and attests that the deployed NyxID ordinary/thread splitters and full-send receipts have been verified. `false` preserves legacy chunking. Only exact `true`/`false` are accepted; see the coordinated rollout below. |
| `CMAEG_NYXID_NATIVE_MEDIA_ENABLED` | boolean | `false` | Advertises and accepts explicit `native_attachments` on verified NyxID Telegram, Lark/Feishu, Discord and WhatsApp relays. This relay-only gate does not control native Telegram Bot API or native Discord media uploads. Enable only after every serving NyxID replica honors the attachment contract and controlled file/image/audio/video sends confirm actual platform attachments. An older replica may ignore an unknown attachment field. Native media uses a single bounded 60 s exchange and a 65 s non-routed tool deadline; legacy text keeps its configured timeout and 8 s tool deadline. Complete ordinary replies use 15 s/18 s. Compatible runtime tool proxies wait 70 s; retained older Sandbox images may still cut off at 10 s. Routed sends retain their independent 20 s window. Only exact `true`/`false` are accepted. |
| `CMAEG_CONSUMER_INTERACTIONS_ENABLED` | boolean | `false` | Enables new named-consumer native controls after every callback reader supports the optional encrypted owner snapshot. Requires each platform's own gate. Retained callbacks remain executable with this flag off. Removed, pending or rebound consumers are refused; credential rotation is allowed. Default records and dispatch remain unchanged. |
| `CMAEG_DISCORD_INTERACTIONS_ENABLED` | boolean | `false` | Issues native Discord buttons and single-select menus in ordinary channels and verified existing threads after all serving and rollback readers support native card identity, optional thread proof, callback inbox snapshots and durable interaction admission. Named consumers also require `CMAEG_CONSUMER_INTERACTIONS_ENABLED`. Disabling stops new issuance; retained callbacks and recovery remain readable. Gateway type-3 callbacks persist Activity before a single type-6 acknowledgement attempt within a 2.5-second budget. Thread controls require retained exact-thread event/reply evidence plus authenticated channel metadata at issuance and for fresh clicks; replies stay in that thread. Multi-select additionally requires `CMAEG_DISCORD_MULTISELECT_ENABLED`; mixed native media and interactive edits remain unsupported. See [Native Discord controls](CMAEG_Native_Discord_Interactions.md). |
| `CMAEG_DISCORD_MULTISELECT_ENABLED` | boolean | `false` | Issues native Discord `choice` controls with `multi: true` after serving and rollback readers support the optional consumed-click selection snapshot. Also requires `CMAEG_DISCORD_INTERACTIONS_ENABLED` and the named-consumer gate when applicable. The submitted whole set is canonicalized in card order, including an explicit empty set. Disabling issuance preserves retained callbacks and recovery. No new API/SDK fields or thread lifecycle authority. See [Native Discord controls](CMAEG_Native_Discord_Interactions.md). |
| `CMAEG_WHATSAPP_INTERACTIONS_ENABLED` | boolean | `false` | Advertises and issues bounded WhatsApp reply buttons and single-choice lists for raw-verified private events; named consumers also require `CMAEG_CONSUMER_INTERACTIONS_ENABLED`. Enable only after every gateway reader supports the retained interaction record and NyxID preserves `reply.metadata.interactive` plus raw button/list selections. Disabling stops new issuance; retained callbacks stay readable. No editing, templates or initiated-send authority is implied. |
| `CMAEG_WHATSAPP_TEMPLATES_ENABLED` | boolean | `false` | Advertises and accepts bounded approved WhatsApp template replies. Enable only after every serving NyxID replica honors `reply.metadata.template` and a controlled approved-template send has confirmed a platform message ID. Disabled refuses templates before credential claim; ordinary replies are unchanged. Only exact `true`/`false` are accepted. This enables neither interactive callbacks nor initiated sends. |
| `CMAEG_RELAY_TEXT_EDITS_ENABLED` | boolean | `false` | Issues ordinary single-part relay text edit references and advertises Lark/Feishu/Discord editing only after every gateway reader understands the enriched reference. Telegram retains its existing interactive editing capability. Disabling stops new issuance; retained references remain readable/editable. Only exact `true`/`false` are accepted. See [Relay text edit rollout](#relay-text-edit-rollout). |
| `CMAEG_LARK_CARDS_ENABLED` | boolean | `false` | Enables new Lark/Feishu JSON 2.0 card issuance and native card capabilities after all interaction readers are upgraded. Only exact `true`/`false` are accepted. Retained card callbacks and edits remain active when issuance is disabled. Named consumers additionally require `CMAEG_CONSUMER_INTERACTIONS_ENABLED`. No threaded cards, mixed native media, or native forms. See [Lark and Feishu card rollout](#lark-and-feishu-card-rollout). |
| `CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED` | boolean | `false` | Issues native Telegram admission evidence and effect authority on newly confirmed bot-message references; advertises delete/reaction tools after all readers are upgraded. Exactly `true`/`false`. Disabling stops new issuance and discovery; previously issued references remain usable until their 24-hour expiry or authority revocation. Relay transports are unsupported. |
| `CMAEG_ACK_TEXT` | string | `Working on it…` | Progress acknowledgement for long turns; nonempty text up to 1024 bytes without controls. |
| `CMAEG_HEARTBEAT_MS` | positive integer, ms | `60000` | Inactivity after acknowledgement before a heartbeat, capped at five per turn. |
| `CMAEG_FAILURE_TEXT` | string | `Sorry, I couldn’t finish that request. Please try again.` | Short failure notice; same bounds as acknowledgement text. |
| `CMAEG_DRAIN_SECONDS` | positive integer, seconds | `600` | Maximum shutdown drain for tracked turns and HTTP requests. |
| `CMAEG_BUSY_RETRY_MAX_MS` | positive integer, ms | `180000` | Maximum interval for retrying upstream busy refusals, including first-conversation preparation. Jitter is added after the provider's `Retry-After` minimum. |
| `CMAEG_CONSOLE_ENABLED` | `true` or `false` | `false` | Switch for the sign-in console at `/console`. Exactly `true` or `false`; any other value fails startup. Off, no `/console` route exists (404, as before) and nothing console-related is allocated. On, the console never blocks startup or readiness: while its OAuth client is incomplete or NyxID discovery has not succeeded, `/console*` answers a generic 503 page and ingress is unaffected. Enabling it requires its in-memory state to fit in an eighth of the `cmaeg` memory limit: `CMAEG_CONSOLE_ACTIVITY_MAX_BYTES` + `CMAEG_CONSOLE_INDEX_MAX_RECORDS` × 300 B + `CMAEG_CONSOLE_CHANNEL_IDS_MAX` × 600 B + `CMAEG_CONSOLE_SUMMARY_CACHE_MAX` × 2 KiB + `CMAEG_CONSOLE_SESSIONS_MAX` × 4 KiB, about 80 MiB at the defaults against 128 MiB at a 1Gi limit; `tests/infra_manifests.rs` enforces this. See [Console OAuth client](#console-oauth-client). |
| `CMAEG_CONSOLE_CLIENT_ID` | string | `""` | The NyxID OAuth client id the console signs people in with: at most 256 printable characters without spaces. Empty leaves the console at its 503 page, with an `error` log naming this key. |
| `CMAEG_CONSOLE_CLIENT_SECRET` | secret string | unset | The console client's secret, injected from Kubernetes Secret `cmaeg-console-oauth` through an optional `secretKeyRef`; never a ConfigMap value, never logged. Unset or empty leaves the console at its 503 page, with an `error` log naming this key. |
| `CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE` | unsigned decimal | `60` | 1..=6000. One gateway-wide limit shared by `/console/login` and `/console/callback`; over it, either renders the sign-in page with 429. It also bounds the token requests the console sends NyxID. |
| `CMAEG_CONSOLE_SESSION_TTL_MS` | unsigned decimal, ms | `28800000` | 600000..=86400000. Absolute lifetime of a console session; it ends then whatever its activity. |
| `CMAEG_CONSOLE_SESSION_IDLE_MS` | unsigned decimal, ms | `1800000` | 60000..=`CMAEG_CONSOLE_SESSION_TTL_MS`. A session without a request for this long ends. |
| `CMAEG_CONSOLE_SESSIONS_MAX` | unsigned decimal | `1000` | 10..=100000. Sessions held at once; beyond it the least recently seen session ends. Each counts 4 KiB toward the console memory rule. |
| `CMAEG_CONSOLE_SESSIONS_PER_SUBJECT` | unsigned decimal | `5` | 1..=50. Sessions one NyxID account holds at once; its oldest ends first, so one account's sign-in loop never ends other people's sessions. |
| `CMAEG_CONSOLE_INDEX_MAX_RECORDS` | unsigned decimal | `100000` | 1000..=1000000. Channel record heads (record id, owner, version) the console's owner index holds. Beyond it the index is marked partial, `cmaeg_console_index_partial` is `1` and the console shows a banner. Each counts 300 B toward the console memory rule. |
| `CMAEG_CONSOLE_INDEX_RESYNC_MS` | unsigned decimal, ms | `600000` | 60000..=86400000. Interval between full re-listings of CMA's record heads, paged sequentially at 100 ± 50 ms, 100 heads per page, under the console's four CMA permits; never a readiness gate. |
| `CMAEG_CONSOLE_SUMMARY_CACHE_MAX` | unsigned decimal | `2000` | 100..=100000. Channel summaries cached by record id and version, least recently used first out. A summary holds names, source and consumer chips, status and the settings the channel detail tabs show (counts, key ids, masked hashes, whether each secret is set, and the routing policy with rule chat and sender lists by count only, each rule's sources and kinds named once, and each of its pattern lists as whole patterns up to 128 bytes with the rest counted), never profiles or secret values. The bound is in 2 KiB units. A summary is charged the larger of its serialized size and a conservative resident-size estimate, rounded up. The estimate includes struct headers, every nested vector and string capacity, allocation padding, a normalized record key charged at its 512-byte bound, and cache/Arc overhead. Larger routed channels consume more units. The manifest rule budgets this many times 2 KiB for those charges; Moka eviction is best effort, so this is not an exact process RSS bound and excludes summaries callers retain after eviction. |
| `CMAEG_CONSOLE_CHANNEL_IDS_MAX` | unsigned decimal | `20000` | 100..=1000000. Channel ids the console remembers for display, learned from `creation.channel_id`, from restores and from an owner's paste-link; an evicted id is relearned from traffic. Each counts 600 B toward the console memory rule. |
| `CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL` | unsigned decimal | `100` | 10..=1000. Activity entries kept per channel; the oldest goes first. |
| `CMAEG_CONSOLE_ACTIVITY_MAX_BYTES` | unsigned decimal, bytes | `33554432` | 1048576..=268435456. Bytes every channel's activity entries and producer entries may hold together; beyond it the oldest entries of any channel are evicted first. Counted in full toward the console memory rule. |
| `CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL` | unsigned decimal | `32` | 1..=256. Distinct custom `ce.source` producers remembered per channel, least recently seen first out; their bytes count against `CMAEG_CONSOLE_ACTIVITY_MAX_BYTES`. |

Provider entry shapes and outbound policy are described in
[CMA operations](CMA_Operations.md#cmaeg). Public URL must have no trailing slash.
Master keys and the CMA service bearer are Kubernetes Secret environment values,
never ConfigMap values.
Master key IDs are unsigned bytes; each base64 key decodes to 32 bytes, and the
first key is current. RUST_LOG controls the tracing filter through the standard
tracing subscriber, except that it cannot raise `reqwest`, `hyper`,
`hyper_util` or `h2` (or any of their submodules) above `warn`: their debug
and trace events can carry token-bearing URLs. A separate fixed filter
combined with RUST_LOG enforces this, so even
`RUST_LOG=trace,reqwest::connect=trace` prints none of them, and it applies
with every media flag off. The latency harness has a separate test-only delay setting
documented below; it is not a deployment key.

## Provider response recovery transport

Recovery is an explicit per-provider opt-in. Existing provider definitions omit
`paths.recovery` and retain their POST and streaming behavior. After deploying
compatible registry readers everywhere, the CMA provider may add this member to
its existing `paths` object:

```json
{"recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}
```

Both paths are required and use the CMA Responses v2 dialect. The same strict
allowlisted origin, normalized templates and encoded opaque path segments apply.
Redirects are never followed. Do not infer recovery from a provider's POST path
or name. Leave Nyxbot recovery absent until its deployed API supplies a verified
snapshot/replay contract. The deployment manifest includes the same opt-in
example and deliberately leaves both provider entries unchanged.

The transport only reads an already accepted response using the fresh consumer
agent key supplied by the dispatcher. CMA's response endpoints recheck current
command/binding and stream authority. There is no management-token fallback,
conversation creation or turn POST. A snapshot must match the sealed journal's
accepted response ID and the frozen command's exact metadata. Its
`conversation.id` is CMA's native Agent ID, not the gateway's conversation alias;
retain that Agent ID and check it on subsequent snapshots and lifecycle frames.

Replay requires `x-cma-response-id` to match the requested accepted response.
Each durable SSE ID must be the canonical decimal `sequence_number`, contiguous
from zero or `starting_after + 1`. Gaps, duplicates, changed IDs, metadata, native
Agent IDs and lifecycle types are refused. An SSE `error` without an ID reports
observation failure, even if it contains `sequence_number`; it never emits a
durable failed-turn event or advances the checkpoint. End-of-stream without a
durable terminal event is likewise an observation failure.

Snapshot bodies are bounded at 4 MiB, HTTP error bodies at 64 KiB, replay wire
bytes at 16 MiB and individual SSE frames at 1 MiB. One read/replay attempt lasts
at most the configured provider request timeout or 60 seconds, whichever is
shorter; a later attempt resumes from the last committed cursor. Dropping or
cancelling a read releases its provider concurrency permit, including an idle
stream or pending response headers. Non-200 HTTP responses preserve their status
and bounded code in `ProviderError::Rejected`; 429 retains `Retry-After` as
`Busy`. In this read-only recovery API, a 5xx refusal is an observation error
eligible for policy-controlled retry, never proof the durable command failed.

This transport does not activate routed ingress or provide its journal,
dispatcher, delivery fencing or checkpoint policy. The dispatcher must advance
the durable cursor only after confirming the corresponding downstream durable
effect, and must never resubmit an accepted turn when observation fails.

## Native thread rollout

Deploy with `CMAEG_THREAD_CONTRACT_WRITES_ENABLED=false`. Keep existing platform
and channel settings intact while the new gateway readers roll out. The flag
blocks new or changed thread contracts and their capability advertisement;
unchanged contracts and their execution remain supported, as does removal.

Wait for every old gateway Pod, including terminating/draining Pods, to exit.
Verify that NyxID supports the dedicated `/api/v1/channel-relay/thread-reply`
endpoint on every serving replica before enabling the flag. Verify an explicit
native-thread reply in a controlled test channel before enabling production
channel opt-ins. Ordinary channel replies retain their existing behavior.

For rollback, disable new thread-contract writes first and retain a gateway
version that can read already stored contracts and sealed thread reply targets.
Disabling the flag does not migrate those records for an older binary.

## Lark resources and native Discord history rollout

The binary defaults for `CMAEG_LARK_RESOURCE_WRITES_ENABLED` and
`CMAEG_NATIVE_DISCORD_THREAD_HISTORY_ENABLED` remain false. The deployment
manifest enables these two opt-in capabilities only after the reader phase:
deploy compatible gateway and consuming application images, wait for every old
Pod including terminating/draining Pods to exit, and retain a compatible
rollback image. Apply the two flags and roll the gateway through its normal
drain procedure; environment values are captured at process startup.

Authenticated `GET /v1/channel-capabilities` must then advertise both
`relay_authoring.lark_resource_writes_enabled=true` and
`relay_authoring.native_discord_thread_history_enabled=true`. Existing sources
remain unchanged. Lark/Feishu needs an owner-authored exact `lark_resources`
binding, independently pinned receiving bot and authorized connection. Native
Discord needs source `thread_history:true` and an explicit
`include_thread_history=true` request on its original event. Normal context
reads make no history request. Enabling the flags performs no platform send.

Keep `CMAEG_NYXID_COMPLETE_REPLIES=false` during this activation. Its retained
runtime proxy compatibility and complete-delivery verification are separate.
The two source capabilities do not require replacing users' active Sandbox
runtimes. Local authenticated contract tests cover these features; connected
Lark/Discord acceptance remains a stated validation limit.

To stop new opt-ins, disable both flags and roll the gateway again. Disabling
native history also stops history reads. Disabling Lark writes preserves
retained bindings; remove an exact binding or rotate its channel key to revoke
old resource locators. Keep compatible readers for retained records and
locators; flag rollback does not make them readable by older binaries.

## Native bot mention rollout

Keep `CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED=false` while deploying the new
gateway reader to every replica and upgrading consuming applications to enforce
guest permissions and durable automation limits. After both are ready, enable
the flag and restart through the drain procedure. Authenticated
`GET /v1/channel-capabilities` then advertises `native_bot_mentions_v1`, allowing
consumers such as NyxID to reconcile existing connections automatically.
Participants do not need an OAuth connection to send an admitted native mention.

The flag controls new policy writes. Existing opted-in sources continue to run
when it is disabled; unchanged definitions and opt-in removal remain writable.
Before returning to an older reader, stop opt-in writers, remove retained
`bot_messages` opt-ins using the current reader, verify the routing audit is
clear, and wait for channel caches and their stale window to expire. The
existing routing audit clear operation removes these opt-ins too. See the
[native mention protocol](CMAEG_Protocol.md#native-automated-group-mentions)
for exact identity and source restrictions.

## Observability

CMAEG serves Prometheus text at `/metrics` on `CMAEG_METRICS_LISTEN`
(`0.0.0.0:9091` by default). The public listener does not expose that path.
Keep the scrape listener private to the cluster. Registration is process-wide
and idempotent. All duration histograms use seconds with buckets
`0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10`, except those named
`cmaeg_media_*`, which use `0.01, 0.05, 0.1, 0.25, 0.5, 1, 2, 4, 8, 15, 30, 60, 90`:
media work spans an 8-second tool budget, a 15-second preparation budget and a
90-second background budget, all beyond the default top bucket.

| Metric | Labels | Measurement |
| --- | --- | --- |
| `cmaeg_ingress_to_provider_seconds` | `source`, `cold`, `media` | Last inbound body byte to first provider request socket write. The transport observation is required before recording a sample. `media` is `none` for a turn that neither prepared attachments nor waited for an earlier turn of its conversation, `prepared` when it prepared eager attachment items first, and `fenced` when it waited for its conversation predecessor to start; it is always `none` while `CMAEG_MEDIA_ENABLED=false`. |
| `cmaeg_restore_duration_seconds` | `outcome` | A shared restore flight, including the bounded CMA fetch, validation, and decryption. |
| `cmaeg_provider_accept_seconds` | `provider` | First socket write of the accepted attempt through the provider's first valid acceptance frame. |
| `cmaeg_turn_duration_seconds` | `provider`, `outcome` | Local turn consumer lifetime, including cancellation and timeout. |
| `cmaeg_delivery_seconds` | `platform`, `outcome` | A logical outbound message or custom terminal delivery, including route pacing and permitted retries. |
| `cmaeg_tool_call_seconds` | `tool`, `outcome` | Shared REST/MCP tool operation, including authorization and waiting for its receipt. |
| `cmaeg_events_total` | `source`, `outcome` | Ingress responses, distinguishing deliberate refusals from accepted and rejected requests. |
| `cmaeg_refusals_total` | `reason` | Bounded admission refusals and protocol error codes. |
| `cmaeg_provider_errors_total` | `provider`, `kind` | Classified turn failures; provider diagnostic strings are never labels. |
| `cmaeg_callback_invalid_total` | `source`, `detail` | Relay and native Telegram callbacks answered 401 `callback_invalid`. `source` is `nyxid_relay` or `telegram`; `detail` is the static verification code returned as the problem detail, such as `payload_invalid` or `telegram_webhook_secret`. |
| `cmaeg_relay_envelope_repairs_total` | `field` | Authenticated NyxID relay envelope facts repaired or dropped instead of rejecting the callback. `field` is one of `attachment_item`, `attachment_overflow`, `attachment_container`, `content_type`, `url`, `platform_message_id`, `file_key`, `image_key`, `filename`, `mime_type`, `size_bytes`, `download_url` or `sender_display_name`. |
| `cmaeg_delivery_worker_restarts_total` | none | Delivery workers replaced after they stopped, for example by panicking. Dispatch continues for every channel; commands already queued for the stopped worker are lost with it. |
| `cmaeg_media_activity_degraded_total` | `source` | Relay (`nyxid_relay`) and native Telegram (`telegram`) activities that still failed validation after attachment fact normalization. Their attachments were reduced to reference, kind and locator, or the message was refused with 202 `attachment_unsupported` if it was still invalid, instead of answering 400 or 401 and losing the message. Only the failing field's JSON pointer is logged. |
| `cmaeg_media_facts_repaired_total` | `source`, `field` | Sender-declared Telegram, Lark and Feishu media facts normalized into the attachment domain instead of refusing the message. `source` is `nyxid_relay` or `telegram`; `field` is one of `filename`, `mime_type`, `dimensions`, `duration_seconds` or `emoji`. |
| `cmaeg_media_items_total` | `source`, `kind`, `outcome` | Telegram, Lark and Feishu media items turned into attachments. `kind` is `image`, `file`, `audio`, `video` or `other`; `outcome` is `extracted` for every attachment, and additionally `invalid_locator` for a native Telegram item whose file identifiers are invalid or a relayed Lark item whose resource key is invalid, which is delivered without a locator. A transcript refused by its speech-to-text spend limits counts once more, with `source` `telegram`, `nyxid_relay` or `custom` and `outcome` `stt_rate_limited`, `stt_channel_budget_exhausted` or `stt_budget_exhausted` (see [Speech-to-text spend](#speech-to-text-spend)). |
| `cmaeg_media_relay_correlation_total` | `outcome` | Relayed media items matched (`matched`) or not (`unmatched`) to a NyxID envelope entry: a Telegram item by its `file_id` as the entry's `url`, a Lark item with a valid resource key by the entry's `image_key` or `file_key`. An unmatched item is delivered without a locator and cannot be fetched. |
| `cmaeg_media_fetch_seconds` | `source`, `outcome` | One attachment retrieval through the source registry: a whole-body fetch from its start to its last body byte or its error, or a streamed open until the response is open. `source` is `nyxid`, `telegram`, `url` or `inline`; `outcome` is `ok` or one of `not_retrievable`, `too_large`, `unauthorized`, `not_found`, `timed_out`, `upstream`, `rejected`, `busy` or `rate_limited`. Never labelled with a URL, token or identifier. |
| `cmaeg_media_fetch_bytes_total` | `source` | Attachment bytes fetched, by the same `source` values; a streamed body's bytes are counted as they pass. The `cmaeg.media.fetch` span carries the same `source` and `outcome`, the byte count and the channel's hash, never a URL, token, file identifier, file name or message ID. |
| `cmaeg_media_inflight_bytes` | none | Bytes reserved against `CMAEG_MEDIA_INFLIGHT_BYTES`, in whole KiB. |
| `cmaeg_media_response_bytes` | none | Weighted bytes reserved against `CMAEG_MEDIA_RESPONSE_BYTES` by response bodies in flight, in whole KiB. |
| `cmaeg_media_jobs_inflight` | none | Media jobs holding one of the `CMAEG_MEDIA_MAX_JOBS` permits. |
| `cmaeg_media_cache_bytes` | none | Weighted bytes held by the derived-media cache. |
| `cmaeg_media_derive_seconds` | `op`, `outcome` | One derivation job of an attachment artifact, from the moment it holds its channel slot and job permit to its result, including the fetch. `op` is `image`, `text`, `excerpt`, `transcript`, `visuals` or `metadata`; `outcome` is `ok` or one of `not_retrievable`, `not_found`, `too_large`, `too_large_pixels`, `unsupported_format`, `decode_failed`, `timed_out`, `transcriber_disabled`, `transcriber_failed`, `too_long`, `rate_limited`, `source_unavailable` or `transform_failed`. A job stopped at its 90-second cap records `timed_out`. Every image normalization also records `op="image"` on its own (waiting for the single decode permit, decoding, downscaling, orienting and re-encoding), including the one inside an `image` derivation and a thumbnail's inside `visuals`, so an `image` derivation that reaches its decode contributes two samples. Refusals answered without a job (an unknown attachment, a disabled transcriber, a spent fetch quota) record nothing here. Uses the media buckets. |
| `cmaeg_media_tool_reads_total` | `view`, `status` | Attachment reads through `readEventAttachment`. `view` is the requested `auto`, `image`, `text`, `frames` or `download`; `status` is the view's `ready`, `pending` or `unavailable`, or `refused` when the read was answered with a problem (media disabled, unverified context, a missing attachment, or a key or reference failure). The `cmaeg.media.read` span carries the same `view` and `status`, the view's `reason` and the channel's hash, never an event_ref, file name or content. Its duration is also recorded as `cmaeg_tool_call_seconds{tool="readEventAttachment"}`. |
| `cmaeg_media_downloads_total` | `outcome` | Requests to `GET /v1/media/content`. A served original counts when its body ends: `served` when every byte was handed on, `aborted` when the 300-second deadline, the `CMAEG_MEDIA_MAX_BYTES` cap, an upstream error or a reader that went away cut it short. A refusal counts at once as `invalid`, `expired`, `used`, `revoked`, `not_found`, `busy`, `rate_limited`, `too_large` or `unavailable`. The `cmaeg.media.download` span carries the outcome, the bytes forwarded and the channel's hash, never a link, jti, URL, file name or `Authorization` value. |
| `cmaeg_media_links_total` | `outcome` | Download links requested through `readEventAttachment` with view `download`. `outcome` is `minted`, `not_permitted` (the binding is not autonomous or installs no gateway tools), `rate_limited` (the channel's 30 per minute or the replica's 600 per minute) or `unavailable` (the link could not be sealed, such as an event too large for the 4096-character token limit, which also logs a warning with the payload's size only). Attachments without a source are answered `not_retrievable` before any link is requested and are not counted. |
| `cmaeg_media_prepare_seconds` | `outcome` | Time a media turn spent on its eager items (transcripts and excerpts), recorded only when at least one ran. `outcome` is `ready` when every eager item was ready, otherwise `pending` when any was still being prepared at the budget, otherwise `unavailable`. The `cmaeg.media.prepare` span carries `items`, `eager`, `outcome` and the channel's hash, never a file name, transcript or excerpt. Uses the media buckets. |
| `cmaeg_media_fence_wait_seconds` | none | Time a turn waited for an earlier turn of its conversation to return from its provider start, recorded only for a turn that had such a predecessor. A turn waits at most the busy-retry bound plus the preparation budget (`CMAEG_MEDIA_PREPARE_BUDGET_MS`) plus five seconds and then proceeds out of order with a `conversation order fence timed out` warning. Uses the media buckets. |
| `cmaeg_media_transform_total` | `op`, `outcome` | Sandboxed media jobs (see [Media job sandbox](#media-job-sandbox)). `op` is `probe`, `frame`, `audio` or `sandbox_check`; `outcome` is `ok`, `timeout` (the gateway's deadline), `cpu_limit` (the 20 s CPU limit), `resource_limit` (the memory watchdog), `killed` (any other SIGKILL), `exit_nonzero`, `output_invalid` (over its stdout cap or failing validation) or `sandbox_unavailable` (exit 125, or the job could not be started or observed). The `cmaeg.media.transform` span carries `op`, `outcome`, `ms`, `cpu_ms` and `stdout_bytes`, never FFmpeg's output, a file name or media bytes. Zero while `CMAEG_MEDIA_FFMPEG_DIR` is empty. |
| `cmaeg_media_transform_seconds` | `op` | One sandboxed media job, from spawn to reap. Uses the media buckets. |
| `cmaeg_media_sandbox_ready` | none | `1` once the media transformer's startup sandbox self-check passed; `0` while the transformer is not configured. |
| `cmaeg_stt_audio_seconds_total` | none | Speech-to-text audio seconds counted for finished calls: the metered seconds, or the provider's reported usage when it is larger. |
| `cmaeg_stt_channels_at_budget` | none | Distinct channels refused by their own `CMAEG_STT_CHANNEL_SECONDS_PER_HOUR` budget within the last hour. It is refreshed on each such refusal and at least every 256 charges. |
| `cmaeg_turns_inflight` | none | Admitted local consumers; decremented on completion or cancellation. |
| `cmaeg_channels_cached` | none | Locally retained positive and negative cache entries, including entries awaiting eviction. |
| `cmaeg_console_enabled` | none | Exported only while `CMAEG_CONSOLE_ENABLED=true`. `1` once the console's OAuth client is configured and NyxID's discovery document passed validation; `0` while `/console*` answers its 503 page. Discovery retries after 30 seconds, doubling to 5 minutes, and a success is fetched again hourly; an unreachable NyxID keeps the last good endpoints, while a document that fails validation sets `0`. |
| `cmaeg_console_sessions` | none | Console sessions held in memory. Sessions live only in the process: a restart or rollout signs everyone out, and the next page view shows the sign-in page. |
| `cmaeg_console_session_evictions_total` | `reason` | Sessions ended by `subject_cap` (the account's oldest beyond `CMAEG_CONSOLE_SESSIONS_PER_SUBJECT`), `global_cap` (the least recently seen beyond `CMAEG_CONSOLE_SESSIONS_MAX`), `idle` or `absolute` expiry. A sweep every minute ends expired sessions nobody returns to. |
| `cmaeg_console_revocations_total` | `outcome` | Best-effort revocations of an ended session's refresh token: `ok`, `failed`, or `dropped` when four are already in flight or no endpoint is known; a dropped token simply expires at NyxID. |
| `cmaeg_console_rejections_total` | `reason` | Console form posts refused before any handler runs: `origin` when the request does not carry exactly one `Origin` equal to `CMAEG_PUBLIC_URL`, `csrf` when its per-session, per-form token is missing, duplicated or wrong. Every refusal answers a generic 403 page. |
| `cmaeg_console_logins_total` | `outcome` | Console sign-in steps: `started` (the sign-in link sent the browser to NyxID), `ok` (a session was created), `expired` (a callback without a valid login cookie or with another `state`; no NyxID call is made), `denied` (NyxID returned no code), `failed` (NyxID refused the code or the identity), `unavailable` (NyxID or the four exchange permits were unavailable) and `rate_limited` (`CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE` reached). Logs carry only the outcome and, for `ok`, an 8-character subject hash prefix. |
| `cmaeg_console_authorizations_total` | `outcome` | Console requests whose owner authority was derived: `ok`, `refreshed` (the access token was rotated first), `refresh_transient` (NyxID could not refresh a token expiring within 30 seconds; the request got the 503 page), `downgraded` (the selected organization's admin role ended and the session dropped to Personal), `ended` (NyxID refused the refresh token or the identity) and `unavailable` (NyxID could not answer the check; the request got the 503 page). See [Console authority](#console-authority). |
| `cmaeg_console_observer_dropped_total` | none | Exported only while `CMAEG_CONSOLE_ENABLED=true`. Console observations dropped because the 4,096-entry observation queue was full. Ingress, launch, delivery, management and the resolver only `try_send` to the queue and never wait for it, so a burst the console's one drainer cannot keep up with loses console detail, never traffic. It should stay `0`; a rising count means the console shows gaps. |
| `cmaeg_console_unauthenticated_total` | `source` | Exported only while `CMAEG_CONSOLE_ENABLED=true`. Ingress requests that failed authentication, by `source` (`nyxid_relay`, `telegram` or `custom`): a bad relay token, Telegram webhook secret, custom HMAC signature or NyxID key. They are only counted here and never enter the console's memory, so knowing a public channel URL is not enough to fill a channel's activity. |
| `cmaeg_console_index_records` | none | Exported only while `CMAEG_CONSOLE_ENABLED=true`. Channel record heads in the console's owner index. The index is listed from CMA 30 seconds after `/readyz` first answers ready, then every `CMAEG_CONSOLE_INDEX_RESYNC_MS`, 100 heads per page, one page at a time with 100 ± 50 ms between pages, under the console's own four CMA permits so it never competes with channel restores; it never gates readiness. Management writes and deletes update it at once. |
| `cmaeg_console_index_partial` | none | Exported only while `CMAEG_CONSOLE_ENABLED=true`. `1` once the owner index reached `CMAEG_CONSOLE_INDEX_MAX_RECORDS` and stopped indexing new channels; the console then shows a banner that some channels may be missing. Raise the bound within the console memory rule. |
| `cmaeg_console_activity_bytes` | none | Exported only while `CMAEG_CONSOLE_ENABLED=true`. Bytes counted by every channel's activity entries and producer entries together, held at or below `CMAEG_CONSOLE_ACTIVITY_MAX_BYTES` by evicting the oldest entries of any channel first. |
| `cmaeg_console_requests_total` | `page`, `outcome` | Exported only while `CMAEG_CONSOLE_ENABLED=true`. Every `/console*` request, by `page` (the route template, such as `/console/channels/{record_id}`, never a request path, so record ids, event keys and source ids never become label values) and `outcome`: `ok`, `redirect`, `invalid` (400), `refused` (403), `not_found` (404, including every page of another owner's channel), `conflict` (409, such as a re-posted form with a stale version), `rate_limited` (429), `not_available` (501, source changes this gateway release cannot make), `unavailable` (503), `client_error` or `error`. |

Sources, outcomes, tools, platforms, and error kinds come from finite gateway
classifications. Provider names come from the boot-time allowlist. Channel IDs,
event IDs, request IDs, credentials, and message contents are never metric labels.
`cold=true` belongs to the request that initiated the restore flight; concurrent
waiters and cache hits do not claim to have initiated it. A stale cache lookup
that starts a background refresh also triggers a restore.

The process holds the 16 prewarmed blocking threads in addition to one Tokio
worker per CPU, so a thread count at that floor is expected, not a leak. While
the pod is at its PID limit, slow DNS resolution or file IO means that work is
waiting for one of those threads rather than for a new one.

Every public router generates a UUID v4 `x-request-id` when the caller did not
provide one, propagates it in the response, and creates one
`cmaeg.request{request_id,route}` span. The route is a template, so event references
in tool URLs do not enter the log. Child restore, verification, turn, and delivery
spans use a 16-character SHA-256 channel prefix and an event ID when available.
Turn and delivery tasks retain the originating trace across task and queue
boundaries. Each media derivation job runs in a `cmaeg.media.derive` span with
`op`, `outcome`, the channel hash and the attachment `index`; it never carries
a file name, URL, token or content. Info logs exclude bearer keys, reply tokens, event references, and
message content. Provider-controlled parsing diagnostics are also excluded.

A request handler that panics is answered with a 500 `internal` problem whose
detail is `internal_error`, with the usual protocol headers and `x-request-id`;
the gateway keeps serving every other request. The error log line is
`request handler panicked`, emitted inside that request's `cmaeg.request` span,
so the request ID and route template identify it. A panic in middleware outside
that span is answered the same way. The process panic hook replaces Rust's
default one and logs `panic` with only the source `location`. Panic payloads
are never logged, because they can carry request data. This applies with all
media flags off.

Ingress responses include `Server-Timing` fields `restore`, `verify`, `launch`,
and `total`, in milliseconds. `total` includes body reading and waiting for the
acceptance response; it is at least every measured part. `launch` is an elapsed
time from body completion, so it overlaps the restore and verify parts and the
fields must not be summed. Without an observed provider socket write, it is
`launch;dur=0;desc="not_written"`; this includes early refusals and integration
compositions without an observed transport. Such values are not successful
zero-latency launches. REST and MCP tools responses include `deliver;dur`,
covering the complete request through the receipt or refusal.

The `request finished` info record includes the exact `server_timing` header
for ingress and tool routes. A separate `ingress socket write` info record emits
`ingress_to_provider_ms`, `source`, and `cold` once per observed first provider
write, under the originating `cmaeg.request` span. The value matches the histogram
sample, before bucket aggregation. It can arrive after the HTTP response if the
acceptance budget elapsed before the write; retain that record instead of treating
the earlier `not_written` header as a zero-latency sample. Repeated transport
notifications and retries do not create extra first-write samples.

For a live latency campaign, retain raw metric scrapes and these timing records
with the pod UID/image, request IDs and measurement window. Calculate nearest-rank
percentiles from the per-write values, separating `cold=true` initiators from
warm requests. Histogram buckets provide threshold bounds rather than exact
per-message percentiles. Metrics have no channel label: an isolated test window
or request correlation is required before attributing aggregate counts to one Bot.

The Responses transport observes successful TCP writes. HTTP/1 connections are
pooled with up to 64 idle connections per provider and a 90-second idle lifetime.
HTTP/2 requests share a connection; cleartext `.svc` providers use prior knowledge,
and HTTPS providers negotiate HTTP/2 or HTTP/1.1 through ALPN. An HTTP/2 frame's
stream ID associates its first HEADERS byte with the right request; SETTINGS,
PING, and other control frames cannot produce a launch sample. The observer reads
only the fixed nine-byte frame header and never decodes credentials or content.

TLS timing is taken below encryption, at the first TCP write of the TLS record
containing the request's first plaintext segment. Earlier handshake and control
records are flushed before that segment is armed. Accepting bytes into a TLS
buffer does not record a write. Provider headers, acceptance, and response content
arrive afterward and do not affect the ingress sample. Retries retain the first
attempt's ingress sample; acceptance duration refers to the accepted attempt.
If acceptance times out before the write, the already-returned header cannot be
updated; the metric records the later write if the admitted turn reaches it.

Each turn's semaphore remains held for the stream lifetime. Dropping an HTTP/2
turn resets its stream while other turns keep their connection. HTTP/1 terminal
frames permit a bounded 100-ms, 16-KiB tail drain for connection reuse; cancelled
consumers close immediately. Request header deadlines remain independent of
stream lifetime. Normal certificate verification remains enabled. The transport
uses the workspace's existing locked Hyper, h2, Tokio, and Rustls dependency
versions; it does not add an alternative TLS stack.

Run the executable latency gate from `api/`:

```sh
cargo nextest run -p cmaeg --run-ignored ignored-only -E 'test(latency_budget)' --locked --success-output final
```

The ignored integration test composes the real management, ingress, turn runner,
tools, and delivery services with local fake CMA, JWKS, provider, and NyxID
endpoints. It creates one channel through the management API, then relocates the
JWKS URL inside the test backend's encrypted record to the local fixture. The
production signature, issuer, restoration, authorization, and provider IO paths
remain active. RSA signing keys are generated at runtime.

The warm run sends 500 callbacks with 16 concurrent callers. Its p50 must stay
below 50 ms and p99 at or below 1000 ms. It asserts there are no CMA reads or
additional JWKS fetches. The cold run sends 200 callbacks in 25 rounds of eight;
each round builds and signs all eight HTTP requests before invalidating the
cache, then polls their sends concurrently. Synchronous RSA signing during the
batch's first poll could otherwise let the first restore finish before later
callers arrive. The eight callers share one restore flight. It asserts 25
additional CMA reads, no additional JWKS fetches, every restore wait above
0.5 ms, launch p99 at most 2000 ms, and restore wait p99 at most 800 ms. The
initiating caller owns the `cold=true` metric sample; all eight wait on the cold cache.
Provider processing time is outside both launch budgets.

The fake CMA read delay defaults to 50 ms and is configurable through the
test-only `CMAEG_LATENCY_RESTORE_DELAY_MS` environment variable. Forced repeated
evictions use an elevated configured restore rate so the harness measures the
restore path; production defaults remain unchanged. The channel uses explicit
tool replies and disables automatic replies and acknowledgements. The provider
still streams a message and completes every turn.

The tool run sends 200 authenticated `replyToEvent` requests through the real
delivery engine to the fake NyxID receiver. Sixteen configured routes run in
rounds; the next round begins three seconds after the previous receipts. The
production route pacing is active, every receipt must confirm delivery, and the
receiver independently checks each route's send interval. Tool `deliver` p99
must stay at or below 500 ms. This measures the tool request on an available
route; requests queued behind the same route's previous send include their
pacing wait and can take longer.

The test prints nearest-rank p50/p90/p99 and writes `api/target/cmaeg-latency.txt`.
CI's `cmaeg-latency` job runs after `backend`, has a fifteen-minute limit, retries
the complete harness once on failure, and uploads that report even on failure
when it exists. The second failure fails the job. No thresholds are relaxed in
CI. Hosted results are recorded on the corresponding pull request's Actions run.

Local evidence on macOS ARM64, 2026-09-22, with the default 50-ms fake CMA delay:

| Measurement | Samples | p50 ms | p90 ms | p99 ms | p99 limit ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Warm launch | 500 | 1.241 | 1.479 | 3.029 | 1000 |
| Cold launch | 200 | 48.937 | 57.036 | 59.157 | 2000 |
| Cold restore wait | 200 | 47.174 | 55.824 | 57.984 | 800 |
| Tool delivery | 200 | 1.887 | 3.807 | 8.678 | 500 |

That run also scraped the private metrics listener with `curl` after the measured
requests and verified 200 confirmed Telegram deliveries, one cached channel,
and zero remaining local turns. The scrape is written to
`api/target/cmaeg-metrics.txt`. These are local fixture measurements; they do not
claim deployed provider or platform latency.

## Deployment and runbooks

Run the gateway as a separate workload with its public listener and private
metrics listener on separate service ports. The gateway stores no durable
state of its own: the encrypted channel records it keeps in CMA are its only
durable state. On a restart it loses its in-memory state, which is
acceptable: console sessions, activity rings, the owner index, publish
limiters, root budgets, routed-turn leases, fan-out finalizers, and the
delivery queues and event groups. A routed tool send answered `unknown` that
is lost with its pod stays unknown; see "Routed tool deadlines" (#995). Readiness is `/readyz`;
liveness is `/healthz`. Readiness gates loaded master keys, parsed provider policy and draining state.
Its CMA reachability field is diagnostic only; remote outages do not remove a
warm replica from service. A public route must never forward the private scrape port.
`infra/cmaeg/` declares a separate `cmaeg` Deployment with **one replica** in
`chrono-bot`, matching the maintainer's 2026-09-22 deployment decision. CMA API
also keeps one replica. The gateway listens on 8090; its private metrics listener
uses 9091 and is not exposed through the public Service. The HTTPRoute attaches
to `gateway-infra/chronoai-gateway`. Its canonical route on `bot.chrono-ai.fun`
rewrites `/event-gateway/v1`, `/event-gateway/mcp`, `/event-gateway/openapi.json`
and `/event-gateway/docs` to the service's existing endpoints. It forwards
`/event-gateway/connect` and `/event-gateway/legacy-console` unchanged so the
backend can preserve each browser flow's paths and exact Origin checks. The SPA
at exactly `/event-gateway` remains on web; `/api/event-gateway` remains CMA's
cookie-custody bridge. The old `bot-gateway.chrono-ai.fun` route stays installed.
DNS, certificates, Gateway listeners and NyxID registrations are unchanged.

Deploy CMA's dual-base validation first. For the first Gateway upgrade, keep the
canonical `CMAEG_PUBLIC_API_URL` but temporarily override
`CMAEG_ADVERTISE_PUBLIC_API_URL=false`. Roll out the final verified image with
that value and wait for all old Pods (including terminating/draining Pods) and
Service endpoints to disappear. Install and verify the canonical public routes,
including `/event-gateway/connect` and the fixed legacy-console entry. Then set
the advertisement flag to `true` and roll the same immutable image again.
During the second rolling update, both generations accept the canonical paths;
only newly issued links differ. The checked-in configmap describes the final
`true` state. A ConfigMap change alone does not update existing process
environments: each phase needs an explicit Pod-template change.

CMA accepts native MCP advertisements at exactly either configured public base.
Its binding/runtime proxy destination remains the retained legacy `/mcp`
authority, so changing the advertisement never rewrites a binding's immutable
tool endpoint or breaks replay. The configured canonical URL is an exact
advertisement alias; request data cannot choose another proxy destination.

Do not advertise canonical setup links during the first image overlap: a new
Pod could issue a link that a still-serving old Pod cannot route. Keep the
existing Service selectors, replica policy, ingress paths and NyxID registration
unchanged. Both services' configmaps set the same canonical public API base.
CMA accepts only exact canonical or legacy callbacks for the observed channel,
including mixed-version deployment. A returned callback/custom-events pair must
use one coherent base. Existing signed
callbacks, channel IDs, keys and cookies are not rewritten. Existing Telegram
webhooks on the exact legacy channel URL stay installed until authorized cleanup;
new webhooks use the canonical base. An unrelated URL is always refused.

`CMAEG_PUBLIC_URL` remains the registered legacy console origin. The canonical
legacy-console entry uses a fixed redirect to that origin; it cannot accept a
caller-supplied destination. `/connect` capability tabs continue to work on the
old host, and new setup links use the prefixed canonical routes. The HTTP OpenAPI
response advertises `{CMAEG_PUBLIC_API_URL}/v1` after phase two (the retained
`{CMAEG_PUBLIC_URL}/v1` while the advertisement flag is false); the reviewed
exported protocol snapshot retains its relative `/v1` server and unchanged
operations/security.
Rollback must retain the additive routes, legacy host and CMA's dual-base
validator once canonical callbacks have been issued. Reverting URL generation
is safe within that compatibility floor; reverting to an older CMA binary that
recognizes only the legacy base would reject retained canonical callbacks.

Use native Kubernetes Secrets, injected as environment variables through explicit
`secretKeyRef` entries. `cmaeg-master-keys` supplies `CMAEG_MASTER_KEYS`, and
`cmaeg-cma-service-token` supplies `CMAEG_CMA_SERVICE_TOKEN`,
`CMAEG_CMA_CLIENT_ID`, and `CMAEG_CMA_CLIENT_SECRET`. Populate both Secrets
out of band. The optional `cmaeg-console-oauth` Secret supplies
`CMAEG_CONSOLE_CLIENT_SECRET` (see [Console OAuth client](#console-oauth-client)). `secret.yaml` contains names and key contracts only, never values.
There are no credential file mounts, CSI resources, or Google Secret Manager
dependencies for the gateway. Its service account has no RBAC grants and does not
mount a Kubernetes API token. Existing `_FILE` inputs remain optional for other
installations; setting both sources for a credential fails startup.

Secrets are read at process startup. Preserve the key ring across pod replacements.
Production supplies the service-account client pair because NyxID access tokens
expire after one hour. CMAEG obtains a token before opening its listener and
shares its cache across records and provider management. Requests renew the token
within the final minute of its lifetime; concurrent callers share one exchange.
A failed refresh can use the existing token only until expiry, and retries are
bounded. Exchange responses and credentials are never logged, and redirects are
refused. Renewal requires no Secret update, file, pod restart, or extra workload.

`CMAEG_CMA_SERVICE_TOKEN` remains a separate stable inbound rekey credential when
renewable authentication is enabled. It is not sent to CMA in that mode. Static
outbound bearer configuration remains available for other installations; those
operators must renew it and restart before expiry. Client-secret rotation also
requires a Secret update and a gateway rollout. Retain the same service-account
subject so stored CMA records and provider bindings remain accessible. Grant the
account only the two gateway permissions. CMA uses the advertised native `/mcp`
endpoint with a runtime-local credential proxy; no NyxID tools catalogue entry
or owner connection is needed. `CMAEG_TOOLS_NYXID_SLUG` remains in the provider
advertisement for providers that use catalogue integration. Configure provider
paths explicitly and retain provider/binding ownership.
A channel's bot agent key supplies provider and tool authority; a relay user token
is never used as a provider credential.

The deployment permits a rolling replacement with one surge pod and zero
unavailable pods. Its 615-second termination grace covers the five-second
load-balancer propagation delay, the 600-second application drain, and teardown.
Apply ConfigMap and populated Secrets before starting the Deployment. Confirm
`/healthz`, `/readyz`, accepted HTTPRoute/health policy status, authenticated CMA
record access, and the public HTTPS route after rollout. A ready gateway alone
does not prove a deployed Telegram round trip.

For shutdown, remove the instance from new ingress through readiness, send SIGTERM
or SIGINT, and allow at least `CMAEG_DRAIN_SECONDS` plus load-balancer propagation
and process-exit margin before force termination. The gateway first drains
admitted turns and then queued delivery under a single deadline. Ephemeral output
can be lost if the deadline expires. A rollout does not stop provider Sandboxes;
the provider remains responsible for their durable lifecycle and admission.

For a restore failure, inspect `cmaeg_restore_duration_seconds` outcomes and
`cmaeg_refusals_total`, then check CMA reachability, the injected service token,
record ownership, and the retained key IDs. Repeated misses can consume the
per-channel restore burst; wait for its refill instead of raising the global
limits first. Invalid records fail closed. Positive stale entries can be served
while a bounded refresh runs; negative entries expire strictly. Deletion or key
revocation can therefore take a cache lifetime to reach another replica.

For latency above budget, compare warm/cold ingress histograms, restore durations,
provider acceptance and delivery durations. High acceptance with low launch is
provider work. High cold launch and restore is record retrieval/decryption. High
same-route delivery can be the required three-second Telegram pacing. Verify the
private metrics scrape directly; a `not_written` launch header is not a fast
successful request. Run the local harness before changing timeouts and retain
its latency and metrics artifacts with the investigation.

For authentication refusals, identify the bounded problem/refusal code and request
ID. Check issuer, route membership, signing-key rotation, callback timestamp and
exact body bytes; do not paste tokens, sealed references or content into logs.
A JWT `kid` never changes the pinned JWKS URL. Creator auth caches positive results
for 60 seconds and failures for ten seconds. Tool authentication is local against
the retained key hash; rotate it through channel update and allow cache expiry on
other replicas. `channel_key` and `agent_key` have different roles.

For master-key rotation, add the new key first and retain old keys on all replicas
before making it current. Invoke the service-authenticated rekey operation for
known channel IDs, checking optimistic conflicts. Existing channel IDs still
contain pointers sealed by their original key, so rekeying only the record does
not make that old pointer key disposable. Keep it while those channel IDs remain
valid. Never remove a key based only on a successful rekey count. Back up the ring
through the secret-management system; the encrypted CMA record alone is not a
recoverable credential backup.

For provider busy failures, inspect inflight turns and upstream `Retry-After`.
Retries keep the event idempotency key. Do not synthesize a new event ID to force
admission; that can create another turn. For uncertain platform delivery, inspect
the receipt's `unknown` outcome and the sink, then reconcile manually if needed.
The gateway does not blindly resend unknown Telegram sends. Webhook receivers
must deduplicate stable webhook IDs across the four bounded attempts.

### Console

`/console` is a NyxID-signed-in owner's view of every channel their account,
or an organization they administer, owns. It is rendered on the server from
gateway memory, carries no JavaScript, and answers only with same-origin
pages and forms; editing consumers and routing stays in the REST API.

| Page | Route | Shows |
| --- | --- | --- |
| Channel list | `GET /console?page=&q=` | 50 channels a page, filtered by a case-insensitive name substring of at most 80 characters: name, short record id, status, source and consumer chips, routing (`legacy`, or how many rules a routed channel has), last event, the last hour's events and failures, version |
| Channel detail | `GET /console/channels/{record_id}?tab=` | Overview (ids, endpoints, settings including the partition key and the custom event key, record size, which secrets are set), Sources, Consumers (each consumer's binding and masked key hash, whether it is mentionable, whether it answers non-human actors and whether that is its own setting or its provider's default from `CMAEG_NON_HUMAN_PROVIDERS`, its reply and publish settings, and a Pending badge), Routing (the default targets, the rules in order with an inactive badge on a rule that names only sources the channel does not hold, addressing, the most targets per event, reply labels, the masked default reply destination, the agent-event limits and whether the default consumer receives every event), Activity (the newest 50 events with how each was routed; `refresh=10`, `30` or `60` adds a meta refresh, any other value is ignored) and Producers |
| Event | `GET /console/channels/{record_id}/events/{event_key}` | How one event was routed (its resolution, rule and targets, or routing's refusal reason, and the resolver's trace: rules tried, consumers skipped and why, the addressing form and the kill switch's shadow decision), each consumer's timeline (launch, acceptance time, delivery and destination) and the parent of a published event while the ring holds it |
| Paste-link | `POST /console/channels/{record_id}/link` | Links a channel id the console has not seen, once it opens to that record |
| Attach | `POST /console/channels/{record_id}/sources` | One form per source type |
| Detach | `GET` and `POST /console/channels/{record_id}/sources/{source_id}/detach` | A confirmation naming a default reply destination that blocks the detach (refused with 409) and the rules it leaves inactive, then the detach |

Every route derives the owner's authority again for each request (see
[Console authority](#console-authority)); another owner's record and an
unknown one both answer the same generic 404 page, on every page, tab and
event. Every response carries the hash-based CSP, `Cache-Control: no-store`,
`X-Frame-Options: DENY` and a `noindex` robots tag.

**Never shown.** Agent keys, `channel_key`, the Telegram bot token and webhook
secret, HMAC secrets (except the one-time reveal below), NyxID tokens, reply
tokens, `event_ref`, `message_ref`, `reply_target_ref`, message text and
CloudEvent data. Secrets appear only as "set" or "not set". A refused form is
re-rendered with what the owner typed except every secret field, and error
pages are generic: no upstream body is ever echoed.

**Masking.** Key hashes are shown as `sha256:…` and their last 8 hex
characters; actor and chat ids as `…` and their last 4 characters; display
names cut to 32 characters; admission allowlists by count only; `ce.source`
without userinfo, query or fragment. A routing rule shows its chat and sender
lists by count, and each `ce.type` or `ce.source` pattern list as whole
patterns up to 128 bytes followed by how many more it holds, so a large
policy stays small in memory. Every owner-controlled string (names,
labels, key ids, bot usernames, `ce.source`, chat ids) is HTML-escaped.

**Memory.** Everything shown is in gateway memory (see
[Console memory](#console-memory)): a restart loses it and the owner index is
listed again from CMA 30 seconds after `/readyz` first answers ready. Until
then the list shows "Loading channels… n"; an index at its bound shows
"Partial index". Activity and producers are relearned from new traffic.

**Sign-in.** Only the explicit "Sign in with NyxID" link leaves the site;
nothing redirects to NyxID by itself, and logout or an ended session lands on
the same-origin signed-out page (see [Console OAuth client](#console-oauth-client)).

**Source attach and detach.** Both run through the same record-keyed
management functions as REST, with the request's verified creator and the
version the owner's page showed, so a second submission of the same form
(a browser refresh) answers 409 and changes nothing. A successful attach or
detach answers 303 to the Sources tab; a detach is made by record and needs no
channel id, and the detached source stops admitting events before the
response returns. When the owner asks the gateway to generate a custom
source's HMAC secret, the attach answers 200 with the one-time reveal: the
secret sits in a collapsed section whose summary shows its last four
characters, and it is never put in a URL, a `Location`, a flash, the session,
a cache or a log. Source changes need #999's record-keyed source mutations
and its `CMAEG_DYNAMIC_SOURCES_ENABLED` switch. This release ships neither:
the Sources tab says source changes are not available yet, and a direct post
answers a 501 page without any write. Each attempt is logged as
`console_action{action, record_hash, outcome, subject_hash_prefix,
owner_kind, source_id}`, and every request is counted in
`cmaeg_console_requests_total`.

**Alerts.**

| Condition | Expression | Meaning |
| --- | --- | --- |
| Sign-ins failing | `sum(increase(cmaeg_console_logins_total{outcome=~"failed\|unavailable"}[15m])) > 5` | NyxID refuses codes or identities, or is unreachable; check the client secret and NyxID |
| Sign-ins rate limited | `sum(increase(cmaeg_console_logins_total{outcome="rate_limited"}[10m])) > 0` | `CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE` is reached; a burst or abuse of `/console/login` |
| Forged or stale forms | `sum(increase(cmaeg_console_rejections_total[15m])) > 20` | Posts from a foreign `Origin` or with a wrong CSRF token; a cross-site attempt or a broken proxy |
| Session caps evicting | `sum(increase(cmaeg_console_session_evictions_total{reason=~"subject_cap\|global_cap"}[1h])) > 0` | More sessions than `CMAEG_CONSOLE_SESSIONS_PER_SUBJECT` or `CMAEG_CONSOLE_SESSIONS_MAX` allow |
| Observations dropped | `increase(cmaeg_console_observer_dropped_total[10m]) > 0` | The console's queue was full; pages show gaps, traffic is unaffected |

### Console OAuth client

The console signs people in with NyxID's authorization code flow (PKCE S256)
as a confidential client. Registering that client is NyxID configuration only.
An admin of the ChronoAI organization registers it once:

```
POST {nyxid}/api/v1/developer/oauth-clients
{"name":"CMA Event Gateway console","client_type":"confidential",
 "redirect_uris":["https://bot-gateway.chrono-ai.fun/console/callback"],
 "allowed_scopes":["openid","profile"],"target_org_id":"<chronoai org>"}
```

- Put the returned `client_id` into the ConfigMap as `CMAEG_CONSOLE_CLIENT_ID`.
- Put the returned secret into Kubernetes Secret `cmaeg-console-oauth`, key
  `CMAEG_CONSOLE_CLIENT_SECRET`, out of band. It is never committed;
  `secret.yaml` carries only the empty Secret and its reference comment.
- Every admin of the organization named by `target_org_id` can manage this
  client, including rotating its secret. Keep the ChronoAI organization's admin
  set minimal.

The redirect URI must be exactly `{CMAEG_PUBLIC_URL}/console/callback`. The
console requests `openid profile` and no service access: it sends no `resource`
and no `requested_service_ids`, and approving the consent page without granting
services is expected.

**How sign-in works.** A visitor without a session sees a same-origin sign-in
page; only its explicit "Sign in with NyxID" link leaves the site, so nothing
ever redirects to NyxID by itself and a logout on a shared machine sticks.
`/console/login` seals the OAuth `state`, a PKCE S256 verifier and the
same-origin return path into the 10-minute `__Host-cmaeg-console-login`
cookie, so the callback works on either pod during a rollout.
`/console/callback` checks that cookie and `state` before calling NyxID, then
exchanges the code with `client_secret_post` (at most four exchanges at once)
and identifies the person with REST's own `authenticate_token` (NyxID
`users/me`, under its 3-second timeout). That call fills REST's per-token
cache exactly as a REST call with the same token would, and its subject is the
one every later Personal check compares against. The `id_token` is never
read.

**Rotating the client secret.** NyxID replaces the secret's hash immediately,
with no grace period:

1. `POST {nyxid}/api/v1/developer/oauth-clients/{client_id}/rotate-secret`;
2. update Secret `cmaeg-console-oauth` with the new secret immediately;
3. run `kubectl rollout restart deployment/cmaeg -n chrono-bot`.

Between steps 1 and 3, console sign-in and session refresh fail, and sessions
that need a refresh end. Ingress, tools and delivery are unaffected.

### Console authority

A session's selected owner is a selection, never a grant. Every `/console*`
request except the sign-in, login, callback and signed-out pages and
`POST /console/logout` derives owner authority again before it reads or
writes anything (Amendment R6 §R6.4). Reads and writes, the owner switch
included, take the same check; there is no separate mutation path. Under the
session's own lock, each request:

1. ends a session idle for `CMAEG_CONSOLE_SESSION_IDLE_MS` (30 minutes) or
   older than `CMAEG_CONSOLE_SESSION_TTL_MS` (8 hours), revoking its refresh
   token best effort;
2. refreshes the NyxID access token only when it expires within 30 seconds.
   NyxID refusing the refresh token (`invalid_grant` or 401) ends the
   session; a timeout, connection error, 429, 5xx or malformed answer keeps
   it, answers the generic 503 page and is not retried for 10 seconds;
3. checks the selection with exactly REST's `authenticate_token`, its 3-second
   timeout and its per-token cache: 60 seconds for a success and 10 seconds
   for a failure. The session keeps no verdict of its own; REST's cache is the
   only cache. Personal requires NyxID `users/me` to name the subject the
   session signed in as; an organization requires `your_role` to be `admin`,
   exactly as `X-CMAEG-Owner` does on REST.

| Selection | NyxID answer | Effect |
| --- | --- | --- |
| Personal | The session's subject | Served |
| Personal | Refused, or another subject | The session ends; a GET shows the sign-in page and a POST goes to `/console/signed-out?reason=expired` |
| Organization | Admin | Served |
| Organization | Refused | The selection becomes Personal with "Your admin access to … has ended", and the Personal check runs: a GET is served as Personal (an organization record answers 404), and a POST changes nothing and goes to `/console` |
| Either | Unavailable, including the 3-second timeout | The generic 503 page with no channel data; the session and its selection are kept |

Logout derives no authority, so it ends the session even during an outage.

**Owner switcher.** The signed-in layout shows the masked subject, a
switcher offering "Personal" and the organizations NyxID's
`GET /api/v1/orgs` lists for the session's own token (fetched at most once a
minute per session; display only), and sign-out. Organizations where the
account is not an admin are shown disabled with "Admin role required".
`POST /console/owner` first passes the same per-request check for the current
selection, then verifies the requested owner with `authenticate_token` and
REST's cache before storing it; a refused choice leaves the selection
unchanged with a notice. Console mutations pass the verified creator to the
same channel functions as REST. Logs carry only `console_action{action,
record_hash, outcome, subject_hash_prefix, owner_kind}` and login
`{subject_hash_prefix, outcome}`.

**Bound.**

| NyxID change | Reads and writes stop within |
| --- | --- |
| Organization admin demoted or removed; organization deleted | 60 s (the cache's success TTL), as on REST |
| User deactivated, or grant revoked, on a Personal or organization view | 60 s, as on REST |
| Logout in this console | At once |
| Console disabled and the rollout restarted | At once (sessions live only in memory) |

**Load.** Per session there is at most one cache fill per 60 seconds, or per
10 seconds after a failure; concurrent requests of one session wait on its
lock and share that fill. Each refresh, about every 870 seconds with NyxID's
900-second tokens, changes the token and so starts a new fill. A Personal
fill is one NyxID call (`users/me`); an organization fill makes at most 3
(`users/me`, then `oauth/userinfo` or `orgs/{id}/authorization` when needed,
then the organization check), and a fallback to Personal adds one more fill.
With 1,000 sessions on organization views, checks cost at most about 50 NyxID
calls per second, plus about 1.2 refreshes per second.

**Alert.** Warn when any console request has been answered with the 503 page
because NyxID could not answer, sustained for 10 minutes:
`sum(increase(cmaeg_console_authorizations_total{outcome=~"unavailable|refresh_transient"}[10m])) > 0`.
Console users are seeing the generic 503 page because NyxID's `users/me`,
organization authorization or token endpoint is slow or unreachable; their
sessions are kept and recover by themselves. Ingress is unaffected.

### Console memory

The console keeps no database. Everything it shows comes from gateway memory,
built from CMA's channel records and from what this process observed, and a
restart loses all of it; the encrypted channel records stay CMA's only
durable state.

- **Owner index.** CMA's record heads (record id, owner, version), listed 30
  seconds after `/readyz` first answers ready and every
  `CMAEG_CONSOLE_INDEX_RESYNC_MS` after, paced and under the console's own
  four CMA permits, and kept current by every management write and delete.
  When an owner opens the console their records are re-listed from CMA at
  most once every 30 seconds. Ownership never changes, so the index is the
  console's authority for which owner may see a record; every read takes the
  request's freshly verified owner and another owner's record answers 404.
- **Summaries.** Name, owner kind, sources, consumer chips (with masked key
  hashes), status and record size, read through management's record-keyed
  load with REST's owner check and cached by record id and version, at most
  `CMAEG_CONSOLE_SUMMARY_CACHE_MAX` 2 KiB units of them; a summary takes one
  unit per 2 KiB of its conservative charge, including nested allocation
  capacities and the normalized record key. Moka eviction is best effort;
  the charge is not an exact RSS measurement and excludes caller-held copies
  after eviction. A page reads at most 50 missing
  summaries; a refresh with unchanged versions reads nothing from CMA. A
  summary never holds a profile, credential or secret. A record that cannot
  be opened is shown as `unreadable`, one CMA could not return just now as
  `unavailable`.
- **Channel ids.** A channel id is sealed with a random nonce and cannot be
  derived from its record, so the console shows only ids it has seen: a
  recoverable creation's retained id, ids restored by ingress, tools or
  management in this process, and ids an owner pastes, which must open to
  that record in this gateway's CMA. It never seals a new pointer, since that
  would be a different id string from the one integrations use. The map is
  display-only, holds at most `CMAEG_CONSOLE_CHANNEL_IDS_MAX` ids, and an
  evicted id is relearned from traffic.
- **Activity.** Per channel, the latest `CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL`
  events: when and from which source slot they arrived, their kind, the actor
  and chat ids masked to their last four characters, attachment counts, each
  consumer's launch, acceptance time and delivery steps, and any refusal. A
  routed event also keeps its routing decision (resolution, rule id and
  targets, and a hand-off's parent and hop) and, when the ingress that routed
  it reported it, the resolver's trace of at most 16 steps: rules tried,
  consumers skipped and why, the addressing form, and the kill switch's
  shadow decision. A trace holds only identifiers and fixed words. The
  ingress's routing report and the launcher's report of one admission make
  one entry. Events are named by the first 16 hex characters of SHA-256 of
  their id.
  Only events that passed ingress authentication are recorded; a failed
  authentication only counts in `cmaeg_console_unauthenticated_total`. An
  entry never holds message text, CloudEvent data, attachments, references or
  tokens.
- **Producers.** Per custom channel, the latest
  `CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL` `ce.source` values, stripped of
  userinfo, query and fragment, with the credential that authenticated them
  (`hmac:<key_id>` or `nyxid_key:#<index>`), first and last seen, count, last
  type and outcome. A producer's identity is advisory: it proves only that
  the sender holds one of the channel's ingress credentials, and in binary
  mode under HMAC the `ce-source` header is not signed.

Activity and producers share `CMAEG_CONSOLE_ACTIVITY_MAX_BYTES`; the oldest
entries of any channel go first, so one busy channel cannot crowd the others
out beyond the budget. Observations reach memory through one queue drained by
one task; the hot paths never wait for it (see
`cmaeg_console_observer_dropped_total`). With `CMAEG_CONSOLE_ENABLED=true` and
a warm index, the ignored `latency_budget_with_console` test measures the
ingress budgets with every observer composed and requires no dropped
observation; run it with `cargo nextest run -p cmaeg --test latency_budget
--run-ignored ignored-only`.

### Relay callback rejection alerts

NyxID records a callback that CMAEG answers with 401 as failed and never
retries it, so a rejected authenticated relay callback is a lost message.
`cmaeg_callback_invalid_total{source,detail}` makes every rejection visible.
Configure these alerts on the private metrics scrape:

| Severity | Expression | Meaning |
| --- | --- | --- |
| Page | `sum(increase(cmaeg_callback_invalid_total{source="nyxid_relay",detail=~"payload_invalid\|payload_bounds\|payload_binding_mismatch"}[15m])) > 0` | Authenticated NyxID callbacks rejected after the signature was verified: messages are being lost. |
| Warn | `sum(increase(cmaeg_callback_invalid_total{source="nyxid_relay",detail="route_denied"}[15m])) > 0` | NyxID delivers a route the channel does not list: tenant route drift. |
| Warn (ratio) | `sum(increase(cmaeg_callback_invalid_total{source="nyxid_relay",detail=~"header_.*\|callback_bounds\|token_.*\|jwks_.*\|signing_key_unknown"}[15m])) / sum(increase(cmaeg_events_total{source="nyxid_relay"}[15m])) > 0.05` | More than 5% of relay callbacks fail before their signature is verified. Such requests can be unauthenticated noise, so they never page. |

Runbook for each alert:

- **Envelope rejection (page).** Find the `relay callback envelope rejected`
  warning in the alert window. Its `field=` names the unknown body field; `-`
  means the failure was not an unknown field, and `category=` then says whether
  it was a syntax, data or truncated-body error. The log never contains the
  offending value. Compare NyxID's callback serializer at its deployed version
  (`backend/src/services/channel_relay_service.rs`) with §6 of the protocol and
  the `CallbackPayload` schema. A new NyxID field needs an additive gateway
  release, as `thread_context` and `activity` received. `payload_bounds` means a
  non-attachment field exceeded its bound; `payload_binding_mismatch` means the
  body disagrees with its signed callback claims and points at a NyxID defect.
  Record the request IDs of the lost messages for their channel owners.
- **Route drift (warn).** Compare the channel's relay `route_ids` with the
  NyxID route that serves the bot, then update the channel with the exact route
  ID. An empty list is a pending binding and denies every callback.
- **Pre-signature failures (ratio warn).** Check the NyxID issuer, JWKS
  reachability and signing-key rotation first, then the callback headers and
  clock skew. A rise without a matching NyxID change is likely unauthenticated
  traffic and needs no data repair.

`cmaeg_relay_envelope_repairs_total` is informational and does not alert.
Non-zero values mean NyxID sent sender facts outside CMAEG's bounds, such as an
oversized file name or display name, and the gateway repaired or dropped them
instead of losing the message. A `field` label that keeps rising is a reason to
compare NyxID's serializer with the protocol, not an outage.

### Inbound media alerts

`CMAEG_MEDIA_ENABLED` turns on Telegram, Lark and Feishu media ingress. Every
sender-declared attachment fact is normalized into the validated domain before
an activity is built, so a degraded activity means the normalizer missed a
validator rule.
Configure this alert on the private metrics scrape:

| Severity | Expression | Meaning |
| --- | --- | --- |
| Warn | `increase(cmaeg_media_activity_degraded_total[15m]) > 0` | A media activity failed validation after normalization and was degraded to minimal attachments or refused as `attachment_unsupported`. The message was not lost, but its media facts were. |
| Warn | `rate(cmaeg_media_downloads_total{outcome="aborted"}[30m]) / rate(cmaeg_media_downloads_total[30m]) > 0.2` | More than a fifth of media download requests ended with a cut-short body. |

Runbook: find the `activity failed validation after attachment normalization`
error in the alert window. Its `pointer=` names the failing field (never its
value). Fix the normalizer (`Attachment::normalize_facts` or the Telegram or
Lark extractor) so that field is mapped into the validated domain, and add the
case to the normalization tests. No data repair is possible or needed.

`cmaeg_media_facts_repaired_total` is informational: it rises whenever senders
declare facts outside the domain, such as a 0×0 video. A rising
`cmaeg_media_relay_correlation_total{outcome="unmatched"}` means NyxID's
envelope stopped listing the raw Telegram `file_id` as an entry's `url`, or the
raw Lark `image_key` or `file_key` as the entry's key; compare NyxID's Telegram
or Lark adapter with protocol §6.

Runbook for aborted downloads: the `cmaeg.media.download` spans with
`outcome=aborted` carry the bytes forwarded. Bodies cut at about
`CMAEG_MEDIA_MAX_BYTES` are originals larger than the cap whose source did not
declare a size; bodies cut after 300 seconds are readers too slow for the
stream deadline; bodies cut early with few bytes point at the source (compare
`cmaeg_media_fetch_seconds` by `source` and `outcome`) or at agents that stop
reading. No state needs repair: every slot and budget share was released when
the body ended.

Image normalization refuses lossless WebP, and lossy WebP whose transparency is
compressed losslessly (libwebp's default, so most transparent WebP, Telegram
stickers included), as `unsupported_format` before any decoder reads it. The
decoder for that compression has no memory bound, so one such image could
exceed the 256 MiB decode budget and the 1536Mi of container memory that media
requires. These refusals appear as
`cmaeg_media_derive_seconds_count{op="image",outcome="unsupported_format"}` and
as `refused a webp that only the lossless decoder reads` debug logs naming the
chunk. They are expected, not a fault, and need no repair.

#### Media download links

A media link is redeemed in a per-replica set before the download is checked
any further. The set keeps a redeemed link's jti for ten minutes, the longest
a link can live, and holds up to 16,384 entries (about 2 MiB). A replica
mints at most 600 links per minute, so at most 6,000 of its links are live at
once and the set never evicts one that is still live: a second use on the same
replica is always refused with `410 media_link_used`. The set is not shared and does not survive a
restart, so during a rollout surge, or after a restart, a link can be redeemed
once more on another replica before it expires. That is the accepted bound:
links expire within minutes and every request re-validates the channel, the
binding, the agent key and the source, so rotating the agent key, switching
the binding to guarded, or removing the source revokes every outstanding link
at once on every replica.

### Media turn preparation

With `CMAEG_MEDIA_ENABLED=true`, a turn whose activity carries attachments
prepares them after admission and before its provider turn starts, so every
binding class, including existing-agent bindings and providers without the
gateway's tools such as `nyxbot`, receives audio transcripts and small text
excerpts in its single input. Audio is eligible when its declared duration is at
most `CMAEG_STT_EAGER_MAX_SECONDS` or undeclared, video when it is not an
animation or sticker and declares at most 60 seconds, and video notes always;
text-like files of at most 256 KiB, or of unknown size, give an excerpt of at
most 4 KiB. The eager items run concurrently within
`CMAEG_MEDIA_PREPARE_BUDGET_MS`; images, and video visuals when a thumbnail or
keyframes exist, are only warmed in the background, never awaited, and only for
tool-bearing bindings. A media turn releases its ingress permit right after it
is spawned, so a burst of voice notes cannot exhaust `CMAEG_INGRESS_INFLIGHT`
for text, and every transport answers exactly as before. Within one
conversation on one replica, turns reach the provider in the order they reached
launch: a turn that follows a media turn waits, at most for the busy-retry
bound plus the preparation budget plus five seconds, until the media turn's
provider start has returned.

Service-level queries on the private metrics scrape:

```promql
# Text and attachment-free turns keep the warm p99 <= 1000 ms (protocol §11).
histogram_quantile(0.99, sum by (le) (rate(cmaeg_ingress_to_provider_seconds_bucket{media="none",cold="false"}[5m])))
# Eager preparation, target p95 <= 8 s for voice notes of up to 60 s.
histogram_quantile(0.95, sum by (le) (rate(cmaeg_media_prepare_seconds_bucket[30m])))
# Time turns waited behind an earlier media turn of their conversation.
histogram_quantile(0.95, sum by (le) (rate(cmaeg_media_fence_wait_seconds_bucket[30m])))
```

A rising share of `cmaeg_media_prepare_seconds_count{outcome="pending"}` means
transcripts routinely miss the budget: check the speech-to-text latency and the
`cmaeg_media_jobs_inflight` saturation before raising
`CMAEG_MEDIA_PREPARE_BUDGET_MS`, which delays every media turn and every turn
queued behind one. `conversation order fence timed out` warnings name only the
channel hash; a turn that logs one reached the provider out of order.

### Speech-to-text spend

Speech-to-text costs money per second of audio, so every call is metered
before it is sent and charged against three limits.

**Metering.** A direct upload is charged `ceil(bytes / 4000)` seconds, at
least one: 4000 bytes per second is a 32 kbps floor, below Telegram and Lark
voice bitrates, so the charge bounds the real duration from above. Only
originals up to the direct cap, `min(CMAEG_STT_MAX_BYTES,
CMAEG_STT_MAX_SECONDS × 4000)` bytes (2.4 MB at the defaults, about ten
minutes at 32 kbps), are sent; a larger one is `too_long`. Declared durations
are never charged: senders define them, and documents carry none. When the
provider's response reports `usage` of type `duration` above the metered
seconds, the excess is charged to both hourly budgets on a best-effort basis
(a refusal is ignored, since the call already happened) and the larger value is
counted in `cmaeg_stt_audio_seconds_total`. A call that fails after it was
charged is not refunded.

**Order and scope.** Each call first takes one of its channel's
`CMAEG_STT_RATE_PER_MINUTE` calls, then charges its metered seconds to its
channel's `CMAEG_STT_CHANNEL_SECONDS_PER_HOUR` budget and only then to the
pod's `CMAEG_STT_SECONDS_PER_HOUR` backstop. The channel goes first so that
one channel's senders exhaust that channel and never the budget every channel
shares; a global refusal after a successful channel charge leaves the channel
charged, which only errs towards spending less. Every refusal reaches the
agent as `transcript unavailable: rate_limited` and counts in
`cmaeg_media_items_total` with outcome `stt_rate_limited`,
`stt_channel_budget_exhausted` or `stt_budget_exhausted`. The budgets are per
pod, as is every media budget.

**Known limit.** The floor assumes at least 32 kbps. Opus voice at 6-24 kbps
can run up to 5.3 times longer than it is metered unless the endpoint reports
usage. Probe P3 records real bitrates and whether usage is returned: if it is,
reconciliation corrects the counter and the budgets; if it is not, enable the
audio transformer, which meters decoded audio exactly, before raising any
budget.

**Trade-off.** High-bitrate audio over 2.4 MB is `too_long` until the audio
transformer truncates it and meters its decoded audio exactly. Autonomous
agents can still download the original.

**Cost.** At $0.0045 per minute, the global default of 36000 seconds per hour
spends at most about $2.70 per hour, and the per-channel default of 3600
seconds per hour at most about $0.27 per hour for one channel.

**Alerts.** Configure these on the private metrics scrape, substituting the
configured `CMAEG_STT_SECONDS_PER_HOUR`:

| Severity | Expression | Meaning |
| --- | --- | --- |
| Warn, for 15 m | `increase(cmaeg_stt_audio_seconds_total[1h]) > 0.8 * 36000` | Spend is near the global backstop. |
| Page, for 15 m | `sum(rate(cmaeg_media_items_total{outcome="stt_budget_exhausted"}[5m])) > 0` | The global backstop is refusing transcripts for every channel. |
| Warn | `increase(cmaeg_media_items_total{outcome="stt_channel_budget_exhausted"}[15m]) > 0` | A channel spent its hourly budget. The `speech-to-text channel budget exhausted` log line carries its `channel` hash for triage. |
| Warn, for 15 m | `cmaeg_stt_channels_at_budget >= 3` | Several channels are at their budget: a sender may be spreading load across bots before the global backstop trips. |

**Log query.** Metered seconds per channel hash per day, from the
`speech-to-text call finished` line inside each `cmaeg.media.stt` span (one
per call; its fields are `channel`, `metered_seconds`, `bytes`, `container`,
`outcome` and `ms`, never a URL, key or transcript):

```sh
kubectl logs -n chrono-bot deploy/cmaeg --since=24h \
  | grep 'speech-to-text call finished' \
  | sed -nE 's/.*cmaeg\.media\.stt\{channel=([0-9a-f]+) metered_seconds=([0-9]+).*/\1 \2/p' \
  | awk '{ seconds[$1] += $2 } END { for (channel in seconds) print channel, seconds[channel] }'
```

### Speech-to-text endpoint

With `CMAEG_STT_URL` set, the gateway transcribes audio through an
OpenAI-compatible endpoint: one multipart `POST` per attempt carries `file`
(named `audio.<format>`, streamed from the fetched bytes without a copy),
`model` and `response_format=json`, with the platform key as a bearer token.
At most four calls run at once per pod; further calls wait for a permit
within their deadline. A 429 or 5xx is retried once after 500 ms when at
least two seconds of the deadline remain; a second 429 is `rate_limited` and
a second 5xx `transcriber_failed`. Any other non-200 status, an unparsable
body, a body without `text` or one over 256 KiB is `transcriber_failed`
without a retry, and an attempt past `CMAEG_STT_TIMEOUT_MS` is `timed_out`.
A `usage` object of type `duration` is read when present and reconciled as
described above; any other `usage` is ignored.

**Privacy.** Audio reaches the processor only after an operator sets both
`CMAEG_STT_URL` and the `CMAEG_STT_API_KEY` Secret. From then on, the audio of
every voice note, audio file and short video sent to any channel is sent to
it; there is no per-channel opt-out yet. Announce this before rollout. The
URL, the key, request and response bodies and transcripts are never logged:
the `cmaeg.media.stt` span carries only the channel hash, metered seconds,
size, container, outcome and duration.

**Credential policy.** Only a platform key, from the `cmaeg-stt` Secret, is
ever sent; the gateway never spends a tenant's key. `chrono-llm-public` may be
used only with a platform key, and only after probe P3 passes.

**Probe P3.** Before adding `ogg` to `CMAEG_STT_FORMATS`, prove the endpoint
accepts Telegram and Lark voice. Keep the key in an environment variable and
never paste it into a ticket. With a real Telegram `.oga` and a real Lark
`.opus`, each renamed `voice.ogg`, run:

```sh
curl -sS -X POST "$CMAEG_STT_URL" -H "Authorization: Bearer $STT_KEY" \
  -F model=gpt-transcribe -F response_format=json -F "file=@voice.ogg;type=audio/ogg"
```

Record on the epic whether each succeeds, whether the response carries
`usage`, and the measured bitrate (bytes divided by duration). Add `ogg` only
when both succeed. Until then such items render
`transcript unavailable: unsupported_format`.

**Failure alert.** Warn when transcriber failures and timeouts exceed 10% of
transcript derivations over 30 minutes:

```promql
(
  sum(increase(cmaeg_media_derive_seconds_count{op="transcript",outcome=~"transcriber_failed|timed_out"}[30m]))
  /
  sum(increase(cmaeg_media_derive_seconds_count{op="transcript"}[30m]))
) > 0.1
```

Runbook: the `speech-to-text` log lines in the alert window give the status
class the endpoint answered; check the provider's status and the key's quota
before raising `CMAEG_STT_TIMEOUT_MS`.

### Media job sandbox

Keyframes and audio transcoding run FFmpeg, a large C decoder, on hostile
input. It never runs in the gateway process: each job is a fresh exec of the
gateway binary, `cmaeg --media-job <op> <bin-dir> <job-dir> [arg]`, that
sandboxes itself and then executes `ffmpeg` or `ffprobe` exactly once with a
fixed argv. The sandbox exists on **linux/amd64 only**; on any other platform
both job modes exit 125 without executing anything.

Before its first step the wrapper parses its arguments (`probe`, `frame` with
a time `^[0-9]{1,5}\.[0-9]{3}$` of at most `86400.000`, or `audio` with
1..=1500 seconds; absolute UTF-8 directories), builds the program path, argv
and an empty environment, takes its own pid and compiles both filters. It then:

1. closes every descriptor above stderr (`close_range`, a raw syscall because
   musl has no wrapper);
2. sets every limit below with soft = hard;
3. switches to `SCHED_IDLE` (a raw syscall: musl's `sched_setscheduler` is an
   `ENOSYS` stub), which FFmpeg's threads inherit;
4. sets `no_new_privs`;
5. installs Filter E, then Filter A;
6. executes the program.

Nothing allocates after step 5. Any failure writes the static line
`cmaeg media-job: step <n> failed` (step 0 is argument parsing) and exits 125,
which the gateway classifies as `sandbox_unavailable`. The job modes are
dispatched before the gateway makes itself non-dumpable, read no environment,
log nothing and build no runtime.

**Filters.** Filter E answers `clone3` with `ENOSYS` and allows everything
else, so libc falls back to `clone`. Filter A is a default-deny allowlist:
anything else fails with `EPERM`. It allows `openat` and `open` only with
`O_ACCMODE`, `O_CREAT`, `O_TRUNC` and `__O_TMPFILE` clear (every open is
read-only; empty files on the memory `emptyDir` would pin inodes outside its
`sizeLimit`), `clone` only with `CLONE_THREAD`, signals (`kill`, `tkill`,
`tgkill`, `rt_sigqueueinfo`, `rt_tgsigqueueinfo`) only to the job's own pid,
`prlimit64` and `sched_getaffinity` only on itself, and fixed `fcntl`, `ioctl`,
`prctl` and `arch_prctl` commands. The order is fixed: Filter A denies the
`prctl` and `seccomp` calls that installing a filter needs, so it comes last,
and the kernel keeps the lowest action, so E's `ENOSYS` wins over A's allow
entry for `clone3`. Syscall numbers outside the x86_64 table (x32 included)
never match A, and any other architecture is killed. The allow table may
never contain the socket family, `io_uring`, process creation or namespaces,
any call aimed at another process (`ptrace`, `process_vm_*`, `pidfd_*`,
`kcmp`), `perf_event_open`, `bpf`, `userfaultfd`, keyrings, mounts, scheduling
or I/O priority of others, `memfd_create`, `inotify`, `fanotify`,
`timer_create`, System V and POSIX IPC, `personality`, `seccomp`,
`execveat`, any file creation or mutation, `openat2`, handle opens, or
permission and attribute changes; a test fails if it ever does. The table
covers every call the pinned FFmpeg makes, as probe P8's strace recorded it
for each op on every fixture; that record added only `getrusage`, with which
FFmpeg's command line reads its own CPU time.

**Limits** (soft = hard, so `prlimit64` on itself cannot raise them):

| Limit | Value | Why |
| --- | --- | --- |
| `RLIMIT_AS` | 402653184 (384 MiB) | One job's term in the memory gate. |
| `RLIMIT_CPU` | 20 s | The kernel sends SIGKILL, never SIGXCPU (FFmpeg's handler would exit 255). |
| `RLIMIT_FSIZE` | 0 | The job writes no file. |
| `RLIMIT_NOFILE` | 64 | |
| `RLIMIT_CORE` | 0 | |
| `RLIMIT_MEMLOCK` | 0 | |
| `RLIMIT_SIGPENDING` | 64 | Bounds the job's own memcg-charged queue of real-time signals. |

`RLIMIT_NPROC` is deliberately absent: it counts per uid across the node, and
uid 10001 is shared; seccomp refuses process creation instead.

**Self-check.** `cmaeg --media-sandbox-check` runs steps 1 to 5 in its own
process and then requires that a thread can still start, `clone3` fails with
`ENOSYS`, `socket`, `openat(".", O_CREAT | O_WRONLY)` and `kill(<parent>, 0)`
fail with `EPERM`, and `/proc/self/status` opens read-only. It exits 0, or
writes `cmaeg media-sandbox-check: <check> failed` and exits 125. The image
build runs it on the musl binary (`linux/amd64` must pass; other architectures
must exit 125), because CI tests use glibc. Building `linux/amd64` under QEMU
user emulation is unsupported for this step; CI builds natively.

### Media transformer

With `CMAEG_MEDIA_FFMPEG_DIR` set and media on, the gateway runs sandboxed
FFmpeg jobs inside its own pod: no other workload, Secret or manifest file is
involved, only the memory `emptyDir` mounted at `/var/cmaeg/media`.

**How it runs.** `TransformerKeyframes` replaces the platform-thumbnail
extractor and `TransformerTranscoder` the direct upload path; each keeps the
one it replaces as its fallback. Both run inside the existing single-flight
derivation job, so the tool's pending answer and the preparation budget are
unchanged. A keyframe set probes the video and extracts up to
`CMAEG_MEDIA_VIDEO_FRAMES` JPEG keyframes at evenly spread positions (one at
the start when the duration is unknown), skipping any frame that fails, and
normalizes each to a 768 px preview with its position. A failed set falls back
to a native Telegram thumbnail when the attachment has one.
`readEventAttachment` view `frames` reports keyframes with `origin` `keyframe`
and `at_ms`, and the fallback as `platform_thumbnail` without a position; event
context reports `inbound_media.visuals` as `keyframes` for every source once
the transformer is configured. Every speech-to-text input, audio or the audio
track of video in any container, is transcoded to 16 kHz mono PCM covering at
most `T = min(CMAEG_STT_MAX_SECONDS, ⌊(CMAEG_STT_MAX_BYTES − 44) / 32000⌋)`
seconds (600 at the defaults) and uploaded as WAV, metered exactly at `⌈PCM
bytes / 32000⌉` seconds; a transcript cut at `T` is marked `[transcript covers
the first m:ss]` in turn input, and its `readEventAttachment` view carries the
notice `transcript covers only the first m:ss of the audio`. A failed transcode
still uploads the original directly when the direct path accepts it; otherwise
the transform's reason is reported. An endpoint whose `CMAEG_STT_FORMATS` omits
`wav` keeps the direct path.

The pod has **one transform slot**. A set holds it from its first job to its
last, so one FFmpeg process and one job directory exist at a time. Audio sets
are served before keyframe sets, each class in arrival order. A channel has at
most one set in the global queue; its later sets wait, never refused, in that
channel's FIFO and join the queue in order, so channels interleave fairly.

**Limits and deadlines.** Each process runs for at most 20 s (and 20 s of CPU).
A keyframe set has 30 s and an audio set 20 s, counted from when the set joins
the global queue, so they include the wait for the slot. The derivation job's
90 s cap bounds everything, including a channel FIFO wait. Every job's stdout
cap (16 KiB for the probe, 256 KiB per frame, `44 + T × 32000` bytes for audio)
is reserved in `CMAEG_MEDIA_INFLIGHT_BYTES` before it is spawned.

| Outcome | Reason reported |
| --- | --- |
| `timeout`, `cpu_limit`, or a keyframe set stopped by its deadline with no frame | `timed_out` |
| `exit_nonzero`, `output_invalid` | `decode_failed`, or `unsupported_format` when FFmpeg reported an unknown or missing decoder |
| `resource_limit`, `killed`, `sandbox_unavailable` | `transform_failed` |

**Watchdog.** One dedicated thread samples the container's cgroup every 10 ms
while a job runs and kills the job through its pidfd once `memory.current`
less `inactive_file` reaches 0.9 × `memory.max`. Page tables, VMAs and thread
stacks a job makes the kernel allocate are charged to the container but not to
the job's `RLIMIT_AS`, and with `memory.oom.group` an OOM would kill the whole
container; the watchdog kills the job first (`resource_limit`).

**Startup.** Before the listener binds, the gateway validates
`CMAEG_MEDIA_FFMPEG_DIR` (owner root), checks and sweeps `/var/cmaeg/media`,
reads `/sys/fs/cgroup/memory.max` (it must be a number, not `max`), starts the
watchdog thread, and runs `cmaeg --media-sandbox-check` through the production
launcher with a pidfd and a watchdog registration. Any failure stops startup
(`media transformer unavailable: …`, for example
`media sandbox self-check failed: sandbox_unavailable`); the old pod keeps
serving because `maxUnavailable` is 0. On success it sets
`cmaeg_media_sandbox_ready` to 1 and logs `media transformer ready` with
`dumpable=false`, `sandbox_check="passed"`, `memory_max_bytes` and
`watchdog_threshold_bytes`. Unconfigured, none of this runs and the gauge
stays 0.

**Metrics.** `cmaeg_media_transform_total{op,outcome}`,
`cmaeg_media_transform_seconds{op}` and `cmaeg_media_sandbox_ready`, described
under [Observability](#observability). Logs carry only the channel hash, op,
outcome, durations and byte counts: never FFmpeg's output, file names, media
bytes or transcripts.

**Alerts**, on the private metrics scrape while the transformer is configured:

| Severity | Condition | Meaning |
| --- | --- | --- |
| Warn | `increase(cmaeg_media_transform_total{outcome="sandbox_unavailable"}[30m]) > 0` | A job could not be started, sandboxed or observed. |
| Warn | non-`ok` jobs above 20 % of `cmaeg_media_transform_total` over 30 m | Media is failing to transform. |
| Warn | `increase(cmaeg_media_transform_total{outcome="killed"}[30m]) > 0` | Only the gateway, the watchdog and the CPU limit should kill a job. |
| Warn | `increase(cmaeg_media_transform_total{outcome="resource_limit"}[30m]) > 0` | Hostile input or a memory gate violation. |

**Operations rule.** While `CMAEG_MEDIA_FFMPEG_DIR` is set, **no `kubectl exec`
of any kind** into the gateway: an exec'd process holds the Secret environment
variables and is dumpable until its first instruction, so a job could read
them. The custom-attachment audit subcommand runs only after the flags are off
(see [Rolling back below inbound media](#rolling-back-below-inbound-media)).

**preStop prerequisite.** The shipped `exec` `/bin/sleep` preStop hook is
dumpable for the same reason. Setting `CMAEG_MEDIA_FFMPEG_DIR` requires
`lifecycle.preStop.sleep` (probe P15: the cluster accepts it).

**Accepted residual risk** (recorded on #961). On linux/amd64, with the
enablement probes recorded, a compromised FFmpeg or ffprobe runs as uid 10001
at `SCHED_IDLE` under `no_new_privs`, both filters, the limits and the
watchdog. It cannot open sockets or io_uring, create or modify any file, signal
or trace another process or read its memory or environment, change another
process's limits or scheduling, create a process, or get the container
OOM-killed, and it never overlaps another job. It can read what uid 10001 can
read (image binaries, its own input, pod metadata under `/proc`; no secret is a
readable file), choose its own validated stdout (which reaches only the agent
of the channel that sent the media), exec another image binary under the same
filters and limits, and spend for at most 20 s per process the CPU that
`SCHED_IDLE` leaves, threads up to the pod PID limit and memory up to the
watchdog threshold. Follow-ups: Landlock and per-job uids.

### Local preview image without the transformer

An explicitly authorized local preview can use the Dockerfile's `local-preview`
target before publishing the source branch. It enables inbound images and the
console while `CMAEG_MEDIA_FFMPEG_DIR` and `CMAEG_STT_URL` stay empty. The preview
base is the immutable production image pinned in that target; it retains the
non-root user, entrypoint, CA bundle, Codex and Gitleaks. It has no FFmpeg or
FFmpeg self-test report. The normal final `runtime` target and its gates are
unchanged. Never describe preview evidence as a native CI or FFmpeg test pass.

On an ARM Mac, cross-compile with Rust 1.96.1, cargo-zigbuild 0.23.4 and Zig
0.14.1, using the shared target directory and `CARGO_INCREMENTAL=0`:

```sh
cargo zigbuild --release --locked --target x86_64-unknown-linux-musl \
  -p cmaeg --bin cmaeg --jobs 2
```

Keep the active target directory exempt from disk-sweeper deletion. Record the
source commit/tree, Cargo.lock hash, compiler/tool versions and executable
SHA-256. Verify a static Linux x86-64 ELF, then supply it through the named
`local-binary` build context. The COPY-only target needs no emulated execution.
Where no Docker writes are permitted, crane can append the binary layer and
write directly to the registry without creating a local image archive:

```sh
crane mutate "$PREVIEW_BASE" --append "$BINARY_LAYER" \
  --label "org.opencontainers.image.revision=$SOURCE_COMMIT" \
  --label "org.opencontainers.image.version=local-preview" \
  --label "dev.chrono.cmaeg.packaging=local-preview-no-transformer" \
  --label "org.opencontainers.image.base.digest=$BASE_DIGEST" \
  --repo "$PREVIEW_REPOSITORY"
```

`BINARY_LAYER` is a tar containing only the verified executable at
`usr/local/bin/cmaeg`, mode 0755, owned by root. Check the pinned base's file
shape and image configuration first, and verify the overlay's executable hash
and preserved user/entrypoint after assembly. Scan the resulting image. Push
its immutable digest, record it in local `infra/cmaeg/deployment.yaml`, and
retain the source and infra commits separately; do not claim CI provenance.

Before switching the gateway, run the exact image on a disposable native
Linux/amd64 Pod in the authorized cluster, with no secrets or service-account
token and the gateway's restricted security context. Require a successful
`--media-sandbox-check` and compare `--print-openapi` with the committed JSON.
These probes establish executable/ABI compatibility, not the full Linux or
transformer suite. Then dry-run and apply the locally committed ConfigMap and
Deployment together, verify live digest/readiness, console sign-in and media
attachment behavior, and record any unperformed acceptance probes explicitly.

### FFmpeg in the gateway image

The gateway image ships a minimal, static, LGPL-only FFmpeg: `ffmpeg` and
`ffprobe` in root-owned `/opt/ffmpeg/bin` (mode 0755), the directory
`CMAEG_MEDIA_FFMPEG_DIR` names to enable the transformer on linux/amd64. The
shipped ConfigMap keeps the key empty; enabling it is a separate, infra-only
change.

**Pin.** Every value is written literally in the `ffmpeg-build` stage of
`infra/cmaeg/Dockerfile`, never through a build argument.

| | |
| --- | --- |
| Version | FFmpeg 9.0.2 "Lei", the newest point release of the latest stable branch when pinned |
| Source | `https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz` |
| SHA-256 | `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e` |
| Signature | `ffmpeg-9.0.2.tar.xz.asc`, verified with `gpg --verify` against the release signing key published at `https://ffmpeg.org/ffmpeg-devel.asc` ("FFmpeg release signing key", fingerprint `FCF9 86EA 15E6 E293 A564 4F10 B432 2F04 D676 58D8`) |

The stage checks the archive's SHA-256 and then builds with `--network=none`:

```sh
./configure --prefix=/opt/ffmpeg --enable-static --disable-shared \
  --extra-ldflags=-static --pkg-config-flags=--static \
  --disable-autodetect --disable-network --disable-everything --disable-doc \
  --disable-debug --disable-ffplay --enable-ffmpeg --enable-ffprobe \
  --enable-swscale --enable-swresample --enable-protocol=file,pipe \
  --enable-demuxer=mov,matroska,ogg,mp3,wav,flac,aac,gif,amr \
  --enable-parser=h264,hevc,vp8,vp9,mpeg4video,aac,opus,vorbis,flac,mpegaudio,gif \
  --enable-decoder=h264,hevc,vp8,vp9,mpeg4,aac,mp3float,opus,vorbis,flac,pcm_s16le,pcm_f32le,amrnb,amrwb,gif,mjpeg \
  --enable-encoder=mjpeg,pcm_s16le --enable-muxer=image2pipe,pcm_s16le \
  --enable-filter=scale,format,aresample,aformat,trim,atrim,null,anull
```

Only what the fixed argv of the [media job sandbox](#media-job-sandbox) needs
is compiled. `trim` and `atrim` serve accurate `-ss` and output `-t`; `null`,
`anull`, `format` and `aformat` serve the filter graphs FFmpeg builds; every
output goes to stdout, so the only muxers are `image2pipe` and `pcm_s16le`
(the component behind `-f s16le`). FFmpeg itself selects what these depend on,
such as the VP9 superframe bitstream filter and the rotation filters. `nasm`
builds the x86 assembly. The stage fails unless both binaries are fully static
(`readelf -d` shows no `NEEDED`), `ffmpeg -L` reports the GNU Lesser General
Public License, and `-buildconf` shows none of `--enable-gpl`,
`--enable-nonfree` or `--enable-version3`.

**LGPL-2.1-or-later compliance.** `/usr/local/share/ffmpeg` in the image holds
`LICENSE` (the source's `COPYING.LGPLv2.1`), `configure.txt` (the
`-buildconf` output), `source.txt` (the version, the source URL and the
archive's SHA-256) and `binaries.sha256`. The binaries are built from the
unmodified source at the recorded URL, with no patch applied.

**Build-time self-test.** The `media-self-test` stage copies the gateway
binary, both FFmpeg binaries and the fixtures in
`api/crates/cmaeg/tests/fixtures/media/` into a clean Alpine image and, as
uid 10001 with `--network=none`, runs
`CMAEG_MEDIA_FFMPEG_DIR=/opt/ffmpeg/bin cmaeg --media-self-test
/opt/cmaeg-media-fixtures`. Every case goes through the production path: the
transform scheduler, a job directory under `/var/cmaeg/media` and the
sandboxed launcher, with a watchdog over a disabled gauge because a build
container has no memory limit. The runtime stage copies the stage's report to
`/usr/local/share/cmaeg/media-self-test.txt`, which forces it to run: a failed
case fails the image build. On linux/arm64 the stage records that the
transformer does not apply. As with the sandbox self-check, building
linux/amd64 under QEMU user emulation is unsupported; CI builds natively.

| Fixture | Op | Passes when |
| --- | --- | --- |
| `h264_aac.mp4`, `vp9_opus.webm` | keyframes (6), audio (5 s) | as below |
| `anim.gif` | keyframes (6) | as below; with `-skip_frame nokey` a GIF may yield fewer frames |
| `voice.ogg` | audio (5 s) | as below, with at least 64000 PCM bytes |
| `tone.mp3`, `tone.amr` | audio (5 s) | as below |
| `hevc_4k.mp4` | keyframes (1) | exactly one frame and every job `ok`: 3840×2160 HEVC decodes within `RLIMIT_AS` at `-max_pixels 8294400` |

A keyframe case passes when the probe reported a duration, at least one JPEG
came back, each is at most 768 px per side and normalizes as a preview, and
no job ended `cpu_limit`, `resource_limit`, `killed` or `sandbox_unavailable`;
another failed frame job only skips its position, as in production. An audio
case passes when its job ended `ok` and the PCM is even, non-empty and behind
exactly the WAV header the gateway writes. The report is one line per case and
op, never media or FFmpeg's stderr:

```text
media-self-test case=<file> op=<keyframes|audio> outcome=<outcome> frames=<n> pcm_bytes=<n> ms=<n>
```

It exits 0 when every case passes, 1 when one fails, 2 on a usage or
configuration error (`CMAEG_MEDIA_FFMPEG_DIR` missing or not root-owned, an
unusable media root) and 125 where no sandbox exists. To repeat it by hand on
a linux/amd64 Docker host (seccomp is not reliable under emulation):

```sh
docker run --rm -e CMAEG_MEDIA_FFMPEG_DIR=/opt/ffmpeg/bin \
  --tmpfs /var/cmaeg/media:uid=10001,mode=0700 \
  -v "$PWD/api/crates/cmaeg/tests/fixtures/media:/fx:ro" <image> --media-self-test /fx
```

**Fixtures.** The fixtures are synthetic (lavfi `testsrc2` and `sine`), with
no third-party content. They were generated once with Debian trixie's FFmpeg
7.1.5 and `libavcodec-extra`, which adds the AMR-NB encoder; any full build
with these encoders works. Each is at most 64 KiB, except `hevc_4k.mp4` at
most 256 KiB, and `tests/media_fixtures.rs` keeps the directory and the
self-test's cases naming the same files. If gitleaks ever flags one,
regenerate it; never add an allowlist entry.

| File | Content | Command |
| --- | --- | --- |
| `h264_aac.mp4` | 2 s 320×240 H.264 and AAC | `ffmpeg -f lavfi -i testsrc2=size=320x240:rate=10:duration=2 -f lavfi -i sine=frequency=440:duration=2 -c:v libx264 -preset veryslow -crf 40 -pix_fmt yuv420p -g 5 -c:a aac -b:a 24k -shortest -movflags +faststart h264_aac.mp4` |
| `vp9_opus.webm` | 2 s VP9 and Opus | `ffmpeg -f lavfi -i testsrc2=size=320x240:rate=10:duration=2 -f lavfi -i sine=frequency=440:duration=2 -c:v libvpx-vp9 -b:v 40k -g 5 -c:a libopus -b:a 16k -shortest vp9_opus.webm` |
| `voice.ogg` | 3 s mono Ogg/Opus, like a Telegram `.oga` | `ffmpeg -f lavfi -i sine=frequency=300:duration=3 -ac 1 -c:a libopus -b:a 16k voice.ogg` |
| `tone.mp3` | 3 s MP3 | `ffmpeg -f lavfi -i sine=frequency=440:duration=3 -c:a libmp3lame -b:a 32k tone.mp3` |
| `tone.amr` | 3 s AMR-NB | `ffmpeg -f lavfi -i sine=frequency=440:duration=3 -ar 8000 -ac 1 -c:a libopencore_amrnb -b:a 12.2k tone.amr` |
| `anim.gif` | 2 s 160×120 animation | `ffmpeg -f lavfi -i testsrc2=size=160x120:rate=5:duration=2 anim.gif` |
| `hevc_4k.mp4` | two 3840×2160 HEVC intra frames at 2 fps (exactly 8 294 400 px, the `-max_pixels` value) | `ffmpeg -f lavfi -i testsrc2=size=3840x2160:rate=2 -frames:v 2 -c:v libx265 -g 1 -crf 45 -tag:v hvc1 hevc_4k.mp4` |

`hevc_4k.mp4` holds two intra frames rather than one because a keyframe set
asks for the frame at the middle of the duration: a one-frame clip has no
frame there, and its set fails before anything is decoded.

**Updating FFmpeg.**

1. Bump the version and the SHA-256 literally in the `ffmpeg-build` stage and
   in `source.txt`, after verifying the new archive's signature against the
   release key above.
2. Rebuild the image.
3. Re-run the P8 strace of every op on every fixture and diff the syscall
   list against Filter A. A missing call off the never-list is added with its
   justification; one on the never-list is not added, and the update stops
   for a design decision on #961.
4. Check that the build-stage self-test passes, `hevc_4k.mp4` included.
5. Check that trivy passes on the image.
6. Record the result on the CVE review issue.

**CVE review.** Trivy cannot see a binary built from source: it carries no
package metadata, so the image scan covers the Alpine packages only. FFmpeg
security fixes are reviewed monthly, and on every FFmpeg security
announcement (`https://ffmpeg.org/security.html`), in the recurring issue
"[chore] infra/cmaeg: review FFmpeg security releases for the gateway image".
Act when a fixed CVE touches an enabled component: a demuxer, parser,
decoder, encoder, filter or protocol in the configure line above, or
`libswscale`, `libswresample` or `libavutil`.

**The measured fallback.** `alpine:3.22` with `apk add ffmpeg` (FFmpeg 6.1.2)
passes the same trivy scan with 0 findings at about 180 MB, but it enables
hundreds of components, network protocols among them, links GPL libraries and
follows Alpine's backports rather than this pin. Switching to it would need a
new P8 record and an amendment on #961.

### Rolling back below inbound media

An image older than the inbound-media release cannot parse a stored channel
definition that carries a custom source's `attachments` setting, because
definitions refuse unknown fields. Nor can it parse stored activities that
carry attachments, and reply targets and interaction cards re-parse the
activities they stored for 24 hours. The order below is mandatory:

1. Set `CMAEG_MEDIA_ENABLED: "false"` in `infra/cmaeg/configmap.yaml`. If
   `CMAEG_MEDIA_FFMPEG_DIR` is set, also set it to empty: no `kubectl exec` of
   any kind is permitted while it is set.
2. Roll the gateway with `kubectl rollout restart deployment/cmaeg -n chrono-bot`
   and wait for the rollout to finish. Management then refuses new settings and
   ingress ignores stored ones.
3. Run
   `kubectl exec -n chrono-bot deploy/cmaeg -- cmaeg --audit-custom-attachments --clear`.
   It must exit 0 with `"remaining":0` and `"unreadable":0` in its summary. After
   a `conflict`, run it again; resolve every unreadable record before going on.
4. Wait at least 25 hours with the flag off, so that every reply target and
   interaction card that stored an activity with attachments has expired.
5. Only then deploy an image below the inbound-media release.

Without `--clear` the command only reports. It pages through every channel
record with the gateway's service credential, opens each with the mounted
master keys, and clears a setting by rewriting the definition without it under
compare-and-swap, re-reading once after a version conflict. A record listed
under an owner other than its definition's owner is reported but never
rewritten. Attachments do not affect the provider binding, so nothing is
rebound. A running replica may serve the old definition from its channel cache
for up to `CMAEG_CHANNEL_CACHE_TTL_MS`, which is harmless because the flag is
already off.

Stdout carries only the report: one JSON line for each definition that carries
the setting, then a summary line. Logs, including one info line with the
`record_id` and `channel_hash` of each cleared record, go to stderr. The command
never prints or logs a definition, secret, blob or owner subject.

```text
{"record_id":"<record UUID>","channel_hash":"<16 hex digits>","action":"found"}
{"scanned":12,"channels":12,"unreadable":0,"found":1,"cleared":0,"remaining":1}
```

| Field | Meaning |
| --- | --- |
| `action` | `found` (reported, not cleared), `cleared`, or `conflict` (the record changed again while clearing; it remains). |
| `channel_hash` | The 16-hex-digit hash used in logs and metrics, present only when the definition retains the channel ID from a reserved creation; otherwise `null`, and the record ID identifies the channel. |
| `scanned`, `channels` | Records listed, and those opened as channel definitions. |
| `unreadable` | Records that could not be opened as a definition, for example under a retired master key. They might carry the setting, so the audit fails closed on them. |
| `found`, `cleared`, `remaining` | Definitions carrying the setting, those cleared by this run, and those still carrying it. |

| Exit code | Meaning |
| --- | --- |
| 0 | Nothing remains and every record was readable. |
| 1 | A setting remains, a record is unreadable, or the audit stopped. A stop prints only its classification to stderr: `configuration invalid`, `service authentication unavailable`, `record store unavailable`, `record store rejected a request`, `record store response malformed` or `channel definition cannot be sealed`; never a URL, token or upstream body. |
| 2 | Usage error; stderr carries `usage: cmaeg [--print-openapi \| --audit-custom-attachments [--clear] \| --audit-routing [--clear [--allow-default-widening]] \| --media-job <op> <bin-dir> <job-dir> [arg] \| --media-sandbox-check \| --media-self-test <fixtures-dir>]`. |

### Rolling back below multi-consumer routing

Every shape the multi-consumer release stores refuses unknown fields, so an
image older than it cannot read a channel definition that stores any of:
`routing`, `secrets.consumer_keys`, an extended source list (no source, more
than four, several NyxID relays, or a custom source on a chat-keyed channel),
or `partition.custom_key`. Such an image answers the channel with 503
`channel_invalid`. Nor can it open consumer-bound references, or the
interaction cards of consumers other than `default`, sealed as token kind
`ic`. The order below is mandatory:

1. Set every multi-consumer flag the deployed release has to `false` in
   `infra/cmaeg/configmap.yaml` (`CMAEG_MULTI_CONSUMER_ENABLED`,
   `CMAEG_DYNAMIC_SOURCES_ENABLED`, `CMAEG_AGENT_EVENTS_ENABLED` and
   `CMAEG_CONSOLE_ENABLED`), roll the gateway with
   `kubectl rollout restart deployment/cmaeg -n chrono-bot`, and wait for the
   rollout to finish. Stored routing then fails closed per event: an event
   reaches `default` or nobody.
2. Run `kubectl exec -n chrono-bot deploy/cmaeg -- cmaeg --audit-routing` to
   list every channel an older image cannot read.
3. Return each listed channel to the legacy shape. Its owner does so with
   `DELETE /v1/channels/{id}/routing?cascade=true`, by removing its consumers,
   or by detaching sources, where the deployed release offers those
   operations. Otherwise the operator runs
   `kubectl exec -n chrono-bot deploy/cmaeg -- cmaeg --audit-routing --clear`,
   adding `--allow-default-widening` only with the owner's recorded sign-off.
   The audit must exit 0 with `"remaining":0` and `"unreadable":0` in its
   summary. After a `conflict` or `unbind_failed`, run it again; resolve every
   unreadable record and every `blocked` channel before going on.
4. Wait at least 25 hours with the flags off: consumer-bound event references
   last 30 minutes, interaction cards and reply targets 24 hours, and receipts
   until their event expires plus 24 hours.
5. Only then deploy an image below the multi-consumer release.

**Emergency path.** Skipping steps 3 and 4 fails closed rather than open: the
older image answers 503 `channel_invalid` for every extended channel and
refuses consumer-bound references and `ic` cards, so those channels stop
serving until they are cleared or the newer image returns. No event or click
reaches a consumer it was not routed to.

Without `--clear` the command only reports: it makes no write and no provider
call. It pages through every channel record with the gateway's service
credential and opens each with the mounted master keys. `--clear` handles each
definition that is not legacy-compatible in this order:

1. It blocks, and never writes, a definition that needs an owner decision:
   `sources_need_owner` when the legacy source and partition rules still
   refuse it once an inert `custom_key` (one no custom source uses) is
   removed; `default_would_widen` when it stores routing and
   `default_receives_all` is `false`, unless `--allow-default-widening` is
   given.
2. It removes the provider binding of every additional consumer, pending ones
   included, with the same binding `DELETE` that channel deletion uses for
   `default`. A 404 counts as removed. Any other failure leaves the record
   unchanged as `unbind_failed`; the log names the provider slug, consumer and
   binding ID, never the key.
3. It rewrites the definition without `routing` and consumer keys, and without
   an inert `custom_key`, under compare-and-swap. After a version conflict it
   re-reads the record once and classifies it again, without removing a
   binding twice; a second conflict is reported as `conflict`.

Clearing routing sends every event to `default`. When `default_receives_all`
is `false`, `default` would then see events the owner kept from it, including
events producers or chat users addressed explicitly to other consumers; that
is why such channels block without the override. A record listed under an
owner other than its definition's owner is reported as `found` and never
rewritten or unbound. The command reads `CMAEG_NON_HUMAN_PROVIDERS` exactly as
the gateway does, so run it in the gateway pod. Clearing runs in its own
process and cannot invalidate a serving replica's channel cache: a replica may
keep its cached definition for up to `CMAEG_CHANNEL_CACHE_TTL_MS` plus an
equally long stale window. With the flags off, the per-event kill switch still
applies to that cached policy, so it can keep an event from a consumer but
never send it to one other than `default`.

Stdout carries only the report: one JSON line for each definition an older
image cannot read, then a summary line. Logs, including one info line with the
`record_id`, `channel_hash`, number of bindings removed and whether
`--allow-default-widening` applied for each cleared record, go to stderr. The
command never prints or logs a definition, profile, secret, key, blob, token,
owner subject or channel ID.

```text
{"record_id":"<record UUID>","channel_hash":"<16 hex digits>","extensions":["routing","sources"],"default_receives_all":false,"action":"blocked","reason":"default_would_widen"}
{"scanned":12,"channels":12,"unreadable":0,"found":1,"cleared":0,"blocked":1,"remaining":1}
```

| Field | Meaning |
| --- | --- |
| `record_id` | The channel's record ID. |
| `channel_hash` | The 16-hex-digit hash used in logs and metrics, present only when the definition retains the channel ID from a reserved creation; otherwise `null`. |
| `extensions` | `routing` (a routing policy or consumer keys) and/or `sources` (an extended source list or a partition `custom_key`). |
| `default_receives_all` | Whether every event the stored policy can admit reaches `default`: `default` is among the default targets and every active rule's targets, no explicit addressing can exclude it, and, with a custom source attached, it answers non-human actors. Always `true` without routing. |
| `action` | `found` (reported, not cleared), `cleared` (returned to the legacy shape by this run), `blocked` (left unchanged for its owner) or `conflict` (the record changed again while clearing; it remains). |
| `reason` | Why a definition is `blocked` or in `conflict`, below; otherwise `null`. |
| `scanned`, `channels` | Records listed, and those opened as channel definitions. |
| `unreadable` | Records that could not be opened as a definition the gateway accepts, for example under a retired master key. They might store an extension, so the audit fails closed on them. |
| `found`, `cleared`, `blocked`, `remaining` | Definitions an older image cannot read, those cleared and those blocked by this run, and those still not legacy-compatible afterwards, blocked ones included. |

| Reason | Meaning |
| --- | --- |
| `sources_need_owner` | The source list or partition still needs the extended rules: no source, more than four, several relays, or a custom source on a chat-keyed channel. Its owner must detach or replace sources. |
| `default_would_widen` | Clearing routing would let `default` see events its policy keeps from it. Its owner removes routing, or the operator clears with `--allow-default-widening` after recorded owner sign-off. |
| `unbind_failed` | A provider refused to remove an additional consumer's binding; the record is unchanged. Bindings removed before the failure stay removed, and a rerun counts them as removed. |
| `conflict` | The record changed again after one re-read. Run the audit again. |

| Exit code | Meaning |
| --- | --- |
| 0 | No definition an older image cannot read remains, and every record was readable. |
| 1 | A definition remains, a record is unreadable, or the audit stopped. A stop prints only its classification to stderr: `configuration invalid`, `service authentication unavailable`, `record store unavailable`, `record store rejected a request`, `record store response malformed`, `channel definition cannot be sealed` or `provider configuration unavailable` (a clear needed to remove a binding but no provider client could be built); never a URL, token or upstream body. |
| 2 | Usage error; stderr carries `usage: cmaeg [--print-openapi \| --audit-custom-attachments [--clear] \| --audit-routing [--clear [--allow-default-widening]] \| --media-job <op> <bin-dir> <job-dir> [arg] \| --media-sandbox-check \| --media-self-test <fixtures-dir>]`. |

### Delivery worker restart alert

Delivery runs on eight workers, each owning the outbound sends of the events
routed to it. A worker that stops, for example by panicking, is replaced at its
index on the next command routed there, and dispatch continues for every
channel. This applies with all media flags off. Without replacement, one
panicked worker would stop every reply, typing indicator, update and failure
notice until the pod restarted, so a restart is a contained fault rather than an
outage. Configure this alert on the private metrics scrape:

| Severity | Expression | Meaning |
| --- | --- | --- |
| Warn | `increase(cmaeg_delivery_worker_restarts_total[15m]) > 0` | A delivery worker panicked; delivery continued. Investigate the logged error. |

The replacement has an accepted loss: commands already queued for the stopped
worker, at most 64, are lost with it. The gateway logs
`delivery worker stopped; buffered commands lost` with the worker index when it
replaces the worker, `delivery worker task ended` with `panicked=true` when it
collects the panicked task, and `delivery command dropped after worker replacement failed`
if the replacement stops before taking the command. None of these logs carries
the panic payload, which can contain message content. Find the error logged
just before the restart in the alert window to identify the fault. The lost
commands cannot be recovered from the gateway; their turns' replies, typing
indicators or updates may be missing on those channels.

### Recovering channel creation

A durable creator can reserve a UUID, save it with its own operation, and include
it as `record_id` in `POST /v1/channels`. CMAEG writes the encrypted pending
record before calling the provider. Recover a lost response through
`GET /v1/channel-records/{record_id}` with the same owner's bearer. The response
retains the original opaque channel ID, callback URL, binding ID, and record
version. `definition.creation.ready=false` means provider binding has not been
committed; event ingress and policy updates remain unavailable. Retry the exact
create body or delete the recovered channel with its current version. A creator
that has discarded transient credentials must recover and clean up the pending
record, then start a new operation with a fresh UUID. Never reuse a retired UUID
or run cleanup concurrently with an outstanding create.

Local fake-service tests cover recovery after gateway restart, exact identities
across replicas, changed-content conflicts, owner concealment, and retained
cleanup identity after provider refusal. These tests do not establish deployed
provider or Telegram evidence.

### Production deployment on 2026-09-22

CMA and CMAEG were deployed to namespace `chrono-bot` in
`gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke`, with one
ready replica each. The rollout was verified at 11:50 UTC. PR #922 merged into
`develop` at `f297f7c98ad9cfa903009eea37089c892b66283f`, closing #917.

Both native `linux/amd64` images came from successful Actions run `35717639418`,
attempt 1. Their actual source revision is
`1cbd86a67d24a3f7ab3b510010fd54f4ac60af94`, with tree
`2529cf609a48d9f07c045e8c15bce7ed85ce238e`, independently verified equal to the
merged `develop` tree. Archive sizes/checksums, immutable Docker image IDs,
architecture, labels, OCI layer digests and semantic configuration equality
were checked before transfer. Registry manifest digests and configurations were
read back after publication; source labels retain the tested revision.

The repository prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Verified deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:72a2f622e50dbc2f72afbb2e1e00a9380b7866e324eb3cfd38bacfc211be4d78` |
| CMAEG | `cma-cmaeg` | `sha256:3a573e6f1d1ee6f21f73a5ec573119e9936db693988c2b354d6afa172dc05f5a` |

All 14 CI jobs passed, including 2,896 regular backend tests, 668 MongoDB
persistence tests, all four image builds/scans and the gateway latency harness.
The harness measured warm launch p99 23.374 ms, cold launch p99 65.347 ms,
cold restore p99 55.969 ms and tool delivery p99 18.211 ms. Those measurements
use the local fake services in CI; they are not deployed Telegram measurements.

The API image contains the production bridge fixes through `2917b8a3` together
with the gateway/provider implementation. The live rollout changed only its
container image and preserved all configuration, enabled bridge flags and the
runtime default `bed09cb7`. A fresh nine-Sandbox baseline taken immediately
before rollout retained all nine Pod UIDs, container images, readiness and
restart counts afterward. This is Pod continuity evidence, not an in-flight
Telegram restart drill.

The approved non-default NyxID role `cmaeg-service` and service account
`cmaeg-prod` were provisioned with exactly `cma:gateway:records` and
`cma:provider:bindings`. Native Kubernetes Secrets supply the stable master key
ring, client credentials and a separate stable inbound rekey bearer. Values
were generated or received in memory and sent directly to Kubernetes; no
gateway credential files were written. The measured outbound token lifetime is
3,600 seconds, and this image includes automatic client-credentials renewal.
Startup against real NyxID and the compiled binary's expiry/renewal journey
passed. No existing human NyxID roles were changed. The earlier role-inventory
403 was Cloudflare error 1010 rejecting a generic Python client signature;
the administrator's account has the required authority.

`bot-gateway.chrono-ai.fun` previously had no dedicated DNS record and inherited
a wildcard pointing at an older ingress. A proxied Cloudflare A record now
points to the shared production gateway at `136.69.78.214`, matching the API's
origin. The existing certificate covers the hostname. Both HTTPRoutes are
accepted with resolved references; the backend and health policies are attached.
Direct HTTPS and public HTTPS through Cloudflare both return healthy readiness,
and HTTP redirects to HTTPS. The private metrics listener returns HTTP 200
through a local port-forward; `/metrics` on the public listener returns 404.

Public read-only smoke checks verified CMA and CMAEG readiness, CMA's exact two
service-account capabilities, authenticated record listing, typed missing-binding
and missing-channel responses, and HTTP 401 on unauthenticated record reads.
The missing-channel request exercised the deployed gateway's renewable service
credential against CMA without creating user resources. #918 remains open:
Telegram round trips, deployed latency measurements, failure notices and the
conversation restart drill have not been performed by this rollout.

### Production Bot integration rollout on 2026-09-22

The production CMA API and CMAEG were updated in `chrono-bot`, with one ready
replica each. Backend Bot integration PR #924 merged into `develop`, and PR #926
records the deployed image pins. Both public readiness endpoints return HTTP 200;
CMAEG reports `cma_reachable: true`.

The registry prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:34450d1e649a38f06239f721a082ac26c43d3ae50aa2446aae4d7c47d02100ca` |
| CMAEG | `cma-cmaeg` | `sha256:e6b41876feeb2e1857a44600a1fab4576154a80c902e13a3ef9a78e09c74b0e0` |

The images came from successful CI run `35743231350`, tested revision
`72f42ffce333dcc67fd99df6fc0ee8422615e2b7`. The tested tree was verified equal
to the merged PR #924 tree before deployment. Archive checksums, image
configuration and labels, OCI blobs, and registry digests were verified.
Native Kubernetes Secrets continue to supply gateway credentials.

The API now has `CHRONO_CMAEG_BASE_URL=http://cmaeg` and
`CHRONO_CMAEG_PUBLIC_URL=https://bot-gateway.chrono-ai.fun`. All nine existing
Sandbox Pod UIDs and image pins were preserved. A before/after comparison for
seven owner Agents passed for public lineage, profile pins, durable event
prefixes, and runtime Pod/container identity. It does not establish continuous
availability between samples, native conversation identity, dirty-file
preservation, duplicate-input absence, browser drafts/reconnect, or cleanup
safety. A subsequent read-only public API inventory found one operator-visible
Bridge in the `enabled` state. The production configuration still enables
management, ingress, allocation, execution, outbound and Telegram bridge
switches. This owner-scoped inventory is not a global bridge-absence check.

Read-only verification at 2026-09-22 17:24 UTC found no `cmaeg` catalogue service
or owner connection in NyxID. The maintainer authorized the existing personal
Telegram bot for testing and directed that NyxID configuration and deployment
remain unchanged. The current CMA Bot flow requires that service connection,
and CMA's provider reports `tools.native_mcp_url` as unsupported. Therefore the
real Telegram tools, latency, restart and failure campaign has not run, and #918
remains open. No bridge migration or removal is established by this rollout.

At this rollout, the Bot dashboard and standalone gateway SDKs were still
awaiting merge. This deployment record does not establish their production
acceptance.

### Production timing evidence rollout on 2026-09-22

At 18:07 UTC, CMAEG was updated to
`cma-cmaeg@sha256:952e60116d5db599e7516e48bcd48d9762c87078b3a6ba8f94353978dfa6a25f`
under the same registry prefix. The Deployment has one desired, ready and
available replica. The nine existing Sandbox Pod UIDs, container identities,
images and restart counts were unchanged across the rollout.

All 14 jobs in CI run `35759943816` passed, including real MongoDB persistence
contracts and the latency gate with the production logger enabled. Its native
gateway image has source revision `1fd368a43fa353b1de1740e79d6d1fd9cb06f546`,
whose tree `571b515ee746340c2b5a2cdb4a95452eefe3d63f` equals PR #929's merged
`develop` tree at `dd5f84ead36a9693928e8bcd4f5734db387686df`. Archive, image
configuration, labels and OCI blobs were checked, and the registry manifest and
configuration were read back before applying the conditional image patch.

The CI fixture measured warm launch p99 26.225 ms, cold launch p99 61.616 ms,
cold restore p99 54.632 ms and tool delivery p99 13.060 ms. These are fixture
measurements, not production Telegram evidence. Both public readiness checks
passed after deployment. An unauthenticated tool-context request returned 401,
and its `Server-Timing` header matched the production request-completion log
under the supplied request ID. This confirms deployed response timing logging;
it does not establish a provider-write sample or Telegram delivery. No NyxID
configuration or deployment changed, and no legacy bridge was migrated or
removed by this rollout.


### Production Bot dashboard rollout on 2026-09-22

PR #928 merged into `develop` as `5dbe75d0`. All 14 jobs in CI run
`35765699875` passed, including the gateway latency gate. The tested revision
`ea917d215d2f5cffdaba6f8680ced0253877e0c0` and merged commit share tree
`9d3d48af2fdc1effcd882dfc19659272f75f74fe`. The CI archive, source labels,
OCI blobs, manifest and registry configuration were verified before deployment.

At 18:58 UTC, `cma-web` was ready on the deployed image
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-web@sha256:d79f5c2331a4bb3d401a33f1e44d06d30f58b6ca034d3de3cdf811ce83b39ef4`.
Only the web container image was patched, using deployment UID, resource version,
replica count and prior-image preconditions. CMA API, CMAEG and web each have
one ready replica in production `chrono-bot`. Public API/gateway readiness and
web HTML checks passed. All nine Sandbox Pod UIDs, container IDs, restart
counts, readiness and runtime images matched the fresh pre-rollout baseline;
the runtime default also remained unchanged.

This deploys Bot creation, list, status and lifecycle controls and closes #916.
It does not establish the real Telegram acceptance campaign in #918. Native
provider tool attachment is being implemented in CMA to respect the maintainer's
instruction to leave NyxID unchanged; that path is not enabled by this web rollout.
The original bridge is still retained pending migration and acceptance evidence.

### Native CMA tools rollout on 2026-09-22

PR #931 merged into `develop` as
`37394165a4df0a5007a6e53a23efa7a8ea431068`, including every commit from #930.
All fourteen required jobs in CI run `35786266601` passed. Its tested revision
`6c2e412b44c608b582844588b942ed42c04bb5cb` and the merged commit share tree
`13cedb931a48622f579764d8706e5b492e0d90da`. Archive checksums, native image
configuration, source labels and immutable registry references were verified.
The runtime image passed its executed Linux checkpoint contract and the pinned
Codex/ACP native-tool contract. No release workflow or `main` merge was used.

At 22:17 UTC, API and web rollouts and public readiness checks had passed in
production `chrono-bot`. API, CMAEG and web each have one ready replica. The
registry prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:d41b02c608f0764856e893ba8f41771c8f9e7f0c199625e9bf1f9919523340c4` |
| CMA web | `cma-web` | `sha256:99985942a90578f0351ed51642c394bc230c324be3556f9cc08548a52810a4c3` |
| New-allocation runtime default | `cma-runtime` | `sha256:bf7fe30a8259015115beb81e8a8d6fbb4394d85e83dcef67e30ca8637e735905` |
| CMAEG, retained from its timing rollout | `cma-cmaeg` | `sha256:952e60116d5db599e7516e48bcd48d9762c87078b3a6ba8f94353978dfa6a25f` |

Server dry-runs preceded all writes. Deployment UID, resource version, replica
count and prior-image preconditions fenced the API/web patches. The runtime
default ConfigMap and API annotation changed together. Native Kubernetes
Secrets continue to supply gateway credentials. NyxID catalogue, configuration,
code and deployment were unchanged.

The initial inventory contained nine runtime Pods. The first broad comparison
correctly detected three replacements and paused acceptance work. An exact-ID,
majority-read Mongo audit then classified six as user-owned Sandboxes and three
as unowned warm reservations. All six owned Pods preserved their UID, Pod spec,
container ID, image digest, readiness and restart count. The three unowned
reservations were retired by the existing warm-pool controller and replaced
under the new default; none had an owned Sandbox record. The audit's temporary
Pod was removed with a UID precondition and absence was verified.

Public observations at 22:12:44 and 22:14:22 UTC passed for seven selected Agents:
lineage, profile pins, durable event prefixes and the associated runtime identity.
This does not establish continuous availability between samples, native
conversation identity, dirty-file preservation, duplicate-input absence, browser
drafts/reconnect or cleanup safety.

CI's fake-provider latency run recorded warm launch p99 14.038 ms (500 samples),
cold launch p99 65.830 ms (200), cold restore p99 57.369 ms (200), and tool delivery
p99 17.124 ms (200). These are CI results, not production Telegram latency.

This deploys native CMAEG tool attachment and optional owner-selected Bot
environment profiles. After admitting either new record shape, this verified API
is the minimum compatible rollback floor; the former production API cannot read
all retained fields. Existing owned Sandboxes keep their original runtime pins.
Real custom-event and Telegram acceptance, including restart and failure drills,
remains under #918. The legacy bridge remains enabled and wired pending that
evidence and the retirement work in #920.

The first disposable custom event at 22:19:38 UTC exposed a first-conversation
readiness mismatch: Responses returned 404, the gateway successfully reserved
the conversation with PUT 201, and the immediate retry returned 409 while its
Agent was still provisioning. CMAEG correctly treated that unrecognized conflict
as a refusal and returned 503. No turn was admitted and no native-tool success
is claimed from this attempt. The existing Telegram route was unchanged.

CMA now returns `429 conversation_not_ready` with `Retry-After: 1` while the
reserved conversation is provisioning. This uses CMAEG's existing bounded busy
retry and retains the event body and idempotency key. Local regressions reproduce
the former 409 refusal, require retryable preparation, and verify one response
identity after readiness and replay. Gateway coverage also exercises a missing
conversation followed by busy preparation and eventual acceptance with one PUT
and unchanged POST bodies/keys. This follow-up still needs its own successful
CI, rollout and fresh live acceptance; it is not part of the image record above.

A second disposable custom channel at 22:36 UTC reproduced the refusal. An
independent attempt to inspect its prepared conversation found the Agent had
failed with `agent_runtime_rejected` before any turn. The native-tool authority
check required a Ready Worktree even though Agent startup deliberately keeps its
Worktree Creating until the conversation opens. The original Mongo fixture had
advanced the Worktree too early and therefore missed this integration mismatch.
The corrected fixture retains the real startup state and tests the first tool
attachment as well as retirement and authority fences. Creating Worktrees may
attach tools only while their Starting Agent opens its conversation or recovers
the pending ready publication; other preparation and removal states still refuse.

Neither disposable attempt establishes native-tool, full-context, restart or
Telegram acceptance. Both channels and their temporary keys were removed; the
second test's runtime teardown was verified at 22:42 UTC. The existing Telegram
route and original Sandbox were unchanged.


### First-conversation production follow-up — 2026-09-22

PR #932 merged into `develop` as
`328093cb1e2a496a1662c9b13bf5582bf1d06381` at 23:40 UTC after all 14 jobs in CI
`35794408644` passed. The merged tree exactly matches the tested merge revision
`f302ece0b83291abdbab6ed4d68d4833a9e532d4`. Backend recorded 2,961 passing tests;
the separate disposable Mongo contracts recorded 630 passes, including native
tool attachment with the real Creating Worktree startup state. The Linux writer
boundary, image gates, SDK/frontend checks and latency gate passed too.

The verified API archive was copied to the existing production registry and its
image configuration was compared again by immutable digest. At 23:43 UTC the
API-only rollout passed readiness with
`cma-api@sha256:a14cec2c7d63bd1bf8150774bed6ce6d5484bab57b4c14d3cd34ea24d73c4875`
under the registry prefix in the prior record. UID, resource-version, replica
count and prior-image checks fenced the server dry-run and update. CMA and CMAEG
remain at one replica each in `chrono-bot`. The web/gateway deployments, runtime
default and API configuration did not change; gateway credentials remain native
Kubernetes Secrets. NyxID configuration, catalogue, code and deployment did not
change.

All nine observed runtime Pods preserved their UID, full spec hash, container ID,
image digest, readiness and restart count across this API update. Public
observations at 23:41:05 and 23:43:14 UTC passed for seven selected Agents' lineage,
profile pins and durable event prefixes. The same observation limits documented
above apply; these snapshots do not prove continuous availability or dirty-file,
native-conversation and browser-reconnect continuity.

CI's fake-provider latency gate recorded warm launch p99 22.470 ms (500 samples),
cold launch p99 62.301 ms (200), cold restore p99 55.650 ms (200) and tool delivery
p99 15.253 ms (200). These remain CI measurements, not Telegram acceptance.
The original bridge remains enabled pending the live acceptance and retirement
prerequisites in #918 and #920.
The first follow-up cold custom event at 23:45 UTC progressed past both prior
startup defects, reached a Ready Agent, then ended with provider HTTP 409 and
gateway `503 provider_unavailable`. The provider problem code was not retained,
so its exact cause is unproved. A separate warm event admitted one turn and read
the full context, but guarded tool approvals and a failed reply call meant it
was not cold-path or reply acceptance. Its channel and temporary key were
removed, and its runtime Pod's absence was verified.

### Native custom context and gateway replacement — 2026-09-23

A fresh disposable channel used the intended autonomous permission profile and
sent one signed custom CloudEvent with exactly 32,768 bytes of text. Cold
conversation preparation retried with the same event and admitted a turn without
manual preparation. The stream completed with exactly one `cmaeg.message`
containing the expected suffix and no failed terminal frame. The public Agent
log recorded one started/completed turn, successful native `readEventContext`
and `sendTyping` calls, and a successful `replyToEvent` after one failed tool
attempt. This does not establish why the first tool attempt failed.

At 00:03:39 UTC, the authenticated CMA context endpoint and the gateway MCP tool
returned the complete text unchanged, with all four exact tool names and an
879-byte sealed reference. The idle gateway Pod was then replaced using its exact
UID and normal termination grace. The replacement kept the same immutable image
and deployment spec. All ten observed runtime Pods, including the disposable
Agent, retained exact UID, full spec hash, container ID, image, restart count and
readiness; API configuration and all deployment specs were unchanged.

At 00:05:54 UTC, the new gateway process returned exactly the same full context:
SHA-256 `26ee6c7b132630f2a8d05ca131f967584eacf57d024629daf93fe6f23c0b6b19`.
The before/after digest covers the complete returned context, not just text.
This proves persisted custom-event context survives gateway replacement. It is
not Telegram ingress, Telegram delivery, or the required latency campaign.
The existing Telegram route and original bridge remain unchanged.


### Admission rollout and restored Telegram migration — 2026-09-23

PR #933 merged into `develop` as
`1e6dd9ec1fda1d58ea55eb39ce01269026e9347f` after all 14 jobs in CI
`35802834774` passed. The merged tree matches tested revision
`9701a51ac982a3db6eb576d2a94c896cef6da66e`. Validation recorded 2,965 regular
backend tests and 669 real Mongo contracts, plus the Linux writer boundary,
built runtime execution/cleanup proof, image and SDK/frontend gates. The runtime
fix skips vanished processes on ENOENT or ESRCH during writer inspection; other
inspection errors still refuse the fence.

At 01:33:49 UTC, the verified API and future-runtime-default rollout passed with
`cma-api@sha256:aaf51a96e0720aa98b5789cd6194d392459308dfab0cfe1fd5118e85c3cddee2`
and
`cma-runtime@sha256:532a64cf193957991248d98d82b7a61f6d333a2b87f5e1db0a19ae55f598ed49`
under the existing production registry prefix. All six owned runtime Pods
retained exact UID, full spec, container identity, image, restart count and
readiness. Three unowned warm reservations retired and were replaced, with
majority Mongo ownership checks before and after. Only the runtime-image default
changed in API configuration. API, web and gateway each had one ready replica.

Seven selected Agents passed lineage, profile and durable history-prefix
comparisons. The first public continuity read after rollout returned HTTP 503;
a later observation succeeded. These snapshots do not establish uninterrupted
availability or explain that first failure, and retain the earlier dirty-file,
native-conversation, duplicate-input and browser-reconnect limitations.
CI fake-provider p99 timings were warm launch 13.724 ms, cold launch 59.088 ms,
cold restore 53.679 ms and tool delivery 9.862 ms; these are not Telegram timings.

The authorized dashboard migration then failed during `mint_agent_key` at
01:35:41 UTC. The failed Bot completed durable cleanup by 01:37:43 UTC with no
channel or pending operation; a subsequent exact-namespace key inventory was
empty. This does not prove key creation itself failed, because cleanup may have
revoked a minted key. CMA's adapter rejects NyxID's offset-form expiry metadata;
inspection also found PATCH requests where the existing key and conversation
update contracts require PUT. These are CMA client compatibility defects.

The original Telegram route was restored active at 01:36:46 UTC with its original
key and fields and no competing active route. The bridge was re-enabled at
01:36:57 UTC, revision 7. The original Sandbox, Workspace, Worktree and Agent were
preserved. No Telegram test message was sent, and no new gateway channel was
created. NyxID catalogue, service configuration, roles, code and deployment were
unchanged. Telegram acceptance remains incomplete, although GitHub issue #918
was closed separately at 01:28:36 UTC without recorded Telegram evidence. Bridge
retirement in #920 remains open and gated on that evidence.

A subsequent disposable live key check reproduced a further CMA contract
mismatch: NyxID rejects API-key `platform: telegram`; this metadata identifies
the client, independently of the Telegram channel platform. With the supported
`generic` value, key creation, exact expiry-instant comparison, callback update
by PUT and readback succeeded at 02:18:48 UTC. The provider returned UTC expiry
with `+00:00`. The disposable key was revoked and its absence verified. No
channel route or Telegram message was created by this check. CMA's route-key
request and metadata validation now use `generic`; the channel stays Telegram.

### NyxID client compatibility rollout and capacity refusal — 2026-09-23

PR #934 merged into `develop` at
`4c06aad4e9ec1870e9d217db0d8c91f4a5f97bf1`. All 14 jobs in CI `35810259814`
passed, including 2,967 regular backend tests, 669 real Mongo contracts, the
Linux writer boundary and all image, frontend and SDK gates. The eight gateway
tests include the dashboard journey through CMA and the compiled gateway,
lifecycle/restart recovery, equivalent offset expiries and cleanup after altered
key metadata. CMA now uses NyxID's existing PUT update methods and `generic`
API-key client platform. It accepts equivalent expiry instants while refusing
changed fractional values; canonical timestamps in CMA remain unchanged.

The API artifact's tested revision
`88577a7c066ea93f48d27ac097006dd7a82da682` and merged revision have identical
tree `cf2e9ed93ab1e181239c23e14d9535162e28d345`. Its archive checksum, image
configuration, architecture and source labels were verified before transfer,
and the immutable registry digest and configuration were read back afterward.
The production API now uses
`cma-api@sha256:8f80d527535370b2150716608629fa97737c2d4236203b9926872256fa072e4f`
under the existing production registry prefix.

At 03:14:58 UTC, the API-only rollout passed its settled health checks. CMA API,
CMAEG and web each had one ready replica. All nine existing runtime Pods retained
their exact UID, full spec, container identity, image, restart count and readiness;
configuration, runtime default and the gateway/web deployment specs were unchanged.
Seven selected Agents passed lineage, profile and durable history-prefix
comparisons. The first post-rollout public reads returned HTTP 503 before the
later checks succeeded. These observations do not establish continuous
availability, and retain the earlier native-conversation, dirty-file, duplicate
input and browser-reconnect coverage limits.

CI's fake-provider p99 measurements were warm launch 34.270 ms, cold launch
64.211 ms, cold restore 57.593 ms and tool delivery 17.612 ms. They are not
Telegram measurements.

The authorized dashboard migration created
`bot_73405efb35c54f349f2bac6918751a10` at 03:16:05 UTC and reached Active by
03:16:09 UTC. Its Agent, Sandbox and Workspace profile pins remained versions
2, 5 and 3 of the original selected profiles. The gateway channel and level-2
provider binding were created successfully, with all requested profile fields
honoured and none refused. This verifies the production Bot creation path; it
does not establish provider readiness or a Telegram round trip.

Provider runtime creation then hit namespace `sandbox-quota`: the new profile
requests 4 CPUs and 8 GiB, while existing requests used 36,875 millicores and
74,816 MiB against limits of 40 CPUs and 80 GiB. Kubernetes rejected creation
of the exact new runtime Pod. The new Bot was disabled and its newly created
route deactivated. At 03:24:03 UTC the original route was restored with its
original key, followed by bridge re-enablement at 03:24:13 UTC, revision 9.
The original Sandbox, Workspace, Worktree, Agent and recorded deliveries remain
intact. The new Bot and its provisioning binding are retained for a capacity
decision; no existing user runtime was stopped or resized.

A proposed change to requests of 44 CPUs and 88 GiB passed Kubernetes server
dry-run but has not been applied. It leaves the quota's CPU/memory limits and Pod
count unchanged and requires the maintainer's capacity decision. No NyxID
catalogue, service configuration, role, code or deployment was changed. No
Telegram test message was sent. The #918 acceptance evidence, #912 independent
documentation exercise and #920 bridge retirement remain incomplete.

## Native Telegram interaction rollout

Deploy the CMA interaction-record endpoint before the gateway that uses it.
It uses the existing `cma:gateway:records` service identity and a distinct durable
collection. The gateway encrypts every record with its existing key ring under a
separate cryptographic domain; retain old keys through the 24-hour card lifetime.
NyxID must support bounded `reply.metadata.reply_markup`, signed callback_query
relay, transport callback acknowledgment, and exact outbound-message UUID edits.
After deploying that adapter, refresh the existing bot's webhook registration so
Telegram `allowed_updates` includes `callback_query`; existing webhook settings
do not change merely because a new adapter image is deployed.

Verify a native choice, edit, and login link in the authorized test chat; restart
a gateway replica between card display and clicking it. Verify another actor,
chat or stale revision is refused, duplicate delivery creates no second provider
turn, and two chats with equal platform message IDs edit their exact own message.
Neither transport acknowledgment nor a confirmed card send proves child-bot
provisioning succeeded; verify the provider's creation receipt separately.

## Lark and Feishu card rollout

Keep `CMAEG_LARK_CARDS_ENABLED=false` through the reader deployment. This preserves
existing numbered-text replies and does not alter Telegram cards. Enable it only
after every gateway reader supports the optional `lark` identity in sealed
interaction records and the deployed NyxID relay accepts `metadata.card` for
sends/edits and authenticated `card.action.trigger` callbacks. The upstream
adapter contract is present in pinned NyxID main `2ab239cecbda8`. Source compatibility does not
prove a real bot's event subscription, scopes or actual platform rendering.

Before activation, use an explicitly authorized test chat to verify a choice,
replacement and link; restart a gateway between displaying and clicking a card.
Verify private callbacks without `chat_type`, current chat/sender/group revocation,
another actor, a copied message and stale revision. A single successful click
must have one stable event ID and provider idempotency key across redelivery.
Test both Lark and Feishu bot configurations before claiming both work live.
No upstream deployment or new upstream feature is required by this CMA change.

Disabling the gate stops new issuance; retained current cards still resolve and
edit. Their records keep the normal 24-hour expiry and original expiry across
replacement. Keep compatible readers and encryption keys until all issued Lark
cards expire. A rollback that removes the callback reader before expiry abandons
those controls. Existing Telegram record bytes and callback IDs are unchanged.

Cards are limited to 4,000 displayed text characters, 100 elements/actions and
28 KiB serialized card JSON (32 KiB relay request), without truncation. A card
cannot combine native media or a native thread reply. Structured replies in
threaded or nonhuman contexts keep their existing text rendering because the
callback contract does not retain the required audience. The same text behavior
is retained for named consumers while their interaction gate is off. This choice
is made before any durable card write or platform send; invalid context proofs
and failures during a native send never trigger a second text send. Named consumers require the separate consumer-interaction gate
and retain their original provider binding and conversation. Native forms are not
advertised; their fields stay plain text. Image URLs in a card become an explicit
link with a degraded rendering report; they are never invented platform image
handles. A malformed/lost edit receipt stays unknown, and an old revision remains
fenced even if its replacement is not confirmed.

## Coordinated complete-reply rollout

Keep `CMAEG_NYXID_COMPLETE_REPLIES=false` until the NyxID dependency is ready.
This default preserves existing short and long reply behavior and its known
chunk/URL/reply-token limitations. Enabling the switch is the operator's explicit
attestation of the following deployed contract; the gateway has no wire-level
capability negotiation or per-part completion metadata.

1. Upgrade and verify NyxID's ordinary and thread reply paths. Both must accept
   the full existing `/api/v1/channel-relay/reply` text, split at final platform
   limits without truncation or excerpt limits, track every send and return a
   valid 200 `{message_id,platform_message_id?}` receipt only after all parts are
   accepted. Exercise the exact thread part-count cap and one part over it
   (formerly 64). An over-cap request may be explicitly refused before any
   platform send or use a verified fallback preserving the full content; it must
   never silently send a prefix. The 2 MiB gateway budget does not guarantee
   NyxID/platform acceptance of every payload. One reply token must authorize all
   internal parts. Partial/failed/uncertain sends must return non-200, even when an earlier platform part has succeeded.
2. Verify the full body budget through NyxID and intervening proxies: CMAEG allows
   at most 2097152 serialized JSON bytes, including escaping and identifiers.
   Its 128 KiB tool ingress and 256 KiB automatic output remain unchanged;
   sanitization/JSON can expand bytes fourfold. A larger aggregate is refused as
   `relay_request_too_large` before token claim or IO, without truncation.
3. Measure ordinary and thread completion with Chinese, emoji, long URLs, code,
   block degradation and maximum escaping expansion. Confirm all platform parts
   and intact content at the destination. Exercise a partial provider failure,
   lost/malformed response and a 429 after an earlier successful part; verify
   the logical send is unknown and is not replayed. Success means platform
   acceptance, not recipient delivery/read confirmation.
4. Verify timing against the fixed 15 s complete ordinary exchange and 18 s
   non-routed NyxID reply tool wait, including pacing/queueing. The outer 18 s
   wait also covers structured card/control requests that might fall back to
   complete text after engine eligibility checks; their native transport stays
   at its configured timeout. Templates/media retain their separate budgets and
   update/typing tools retain 8 s. Durable routed sends retain
   their 20 s window and 30 s claims/target leases. A compatible runtime image
   uses a 70 s opaque tool-proxy wait; existing Sandbox runtimes retain their
   pinned image and may still have a 10 s cutoff. This does not require a live
   Sandbox restart. Long platform sends may exceed these budgets; the 64-part
   cap is not a completion-time promise. Do not infer failure to send from a
   timeout. Legacy part/card/edit/non-media template exchanges keep the configured
   timeout (`CMAEG_NYXID_REPLY_TIMEOUT_MS=5000` by default). Native media,
   including WhatsApp templates with image, video or document headers,
   retains 60 s/65 s for non-routed sends, which fits the compatible runtime
   proxy budget. The routed 20 s window remains independent; no lease or claim
   duration is extended.
5. Set `CMAEG_NYXID_COMPLETE_REPLIES=true` only after those acceptance checks.
   Native Telegram chunking and interactive/edit one-part constraints stay in
   effect. Acks, progress notices and intentional replies remain separate.
   Complete-mode 429s preserve the route cooldown and return
   `unknown:rate_limited_unconfirmed` without retrying; other lost/invalid/failed
   receipts are also unknown and suppress automatic fallback answers.

Rollback selects `false` for future sends and restores legacy behavior; it must
not replay uncertain requests from complete mode. No durable delivery or
exactly-once platform effects are promised. Local fake transport and paused-time
integration tests establish the CMAEG contract only. They do not establish that
a deployed NyxID, proxy or IM platform satisfies it. Record actual deployment
versions, all-parts platform observations and timing evidence before activation.


### Relay text edit rollout

`CMAEG_RELAY_TEXT_EDITS_ENABLED` defaults to `false` in code. The deployment
ConfigMap records the desired enabled state after the compatible-reader rollout;
it is not evidence of live activation. Absent or exact `false` preserves the legacy serialized message
reference payload for both legacy chunking and complete replies. Empty, padded,
non-Unicode or other boolean spellings fail startup. This flag is independent
of complete replies and native media.

1. Deploy compatible readers to **all** gateway replicas with the flag false.
   Older readers reject the new optional `relay_message` field because their
   sealed payload uses strict unknown-field rejection.
2. Verify the deployed NyxID text update endpoint and its correlated receipt
   for each advertised platform, then explicitly enable the flag. Only confirmed,
   single-part, bounded ordinary text receives edit authority: Telegram,
   Lark/Feishu (4,000 UTF-16 code units) and Discord (2,000). Native attachments,
   interactive controls, multipart replies and unsupported platforms do not
   acquire this authority. Context retains Telegram's existing editing capability;
   Lark/Feishu/Discord advertise editing only when the flag is enabled.
3. To stop new issuance, set the flag false. Compatible readers continue to
   accept and execute previously issued enriched references; legacy native
   Telegram and interactive-card edits retain their existing behavior.

**Rollback floor:** once an enriched reference has been issued, disabling the
flag does not make it readable by an older gateway binary. `message_ref` has no
embedded expiry, so there is no timed wait after which downgrading is safe.
Keep a reader-compatible release until an explicit audit proves no enriched
references remain usable. Do not use event-reference TTL as a rollback window.
No reader, platform or destination authority is weakened by disabling issuance.


## WhatsApp button and list rollout

Keep `CMAEG_WHATSAPP_INTERACTIONS_ENABLED=false` until every gateway reader is
upgraded. A source must have `verify_raw_event:true`; trusted normalized messages
cannot issue these controls. Existing text, templates, other platforms and
unrelated WhatsApp buttons keep their previous behavior. Structured replies on
trusted-normalized sources, or for named consumers while their interaction gate
is off, retain their existing numbered-text rendering without a provider context
lookup. Other valid contexts that cannot host native controls also keep the text
path. Invalid context proofs and native-send failures never fall back or resend.
Only private, unthreaded
events qualify; named consumers also require `CMAEG_CONSUMER_INTERACTIONS_ENABLED`. Cloud API does not identify a human versus
an automated account; CMA binds a choice to the exact authenticated numeric
sender and private destination without inventing a human classification.

NyxID source `2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f` already accepts
`reply.metadata.interactive` and includes original `interactive.button_reply` /
`interactive.list_reply` plus `context.id` in signed callback bodies. WhatsApp
message editing is absent upstream and remains unsupported. Source inspection
and fake-transport tests do not establish a real bot's Graph permissions,
customer-service window, or platform rendering. No NyxID code or deployment is
part of this change.

Before enabling, use an authorized private test conversation to verify both a
reply-button click and list selection through a real WhatsApp account. Verify
another account, a copied message, an unbound/expired record and a modified
revision/action cannot invoke a provider. Restart a gateway between display and
click; replay a callback and verify one logical provider event/idempotency key.
Temporarily revoke the sender ACL and verify the card remains unused until a
new permitted attempt. A confirmed send must include both a NyxID outbound UUID
and platform message ID before the durable record becomes clickable.

CMA accepts one `buttons` block with 1–3 options (20-character titles), or one
single `choice` with 1–10 list rows (24-character titles), optionally alongside
text blocks. It refuses multi-select, multiple controls, mixed forms/cards/links,
native attachments and threaded replies before durable writes or sends. Labels
are unique, nonempty and never truncated. Both kinds use a conservative 1,024
character body, 16 KiB interactive JSON and 20 KiB complete relay body. Button
styles and preselection become plain labels (`✓ ` for selected options), with a
degraded rendering report. Current Meta reference limits are documented at
[reply buttons](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/interactive-reply-buttons-messages)
and [lists](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/interactive-list-messages).

The first durable card write is the delivery commit point. Only an explicit
HTTP 429 gets one paced retry with the same locator and channel credential.
Timeouts, 5xx, missing/malformed receipts and failed receipt binding remain
unknown, without an automatic resend or usable MessageRef. An unbound record
never authorizes a click. A callback temporarily unable to write its CAS returns
503 for retry; a consumed click keeps its original timestamp, values, reference,
revision and stable event ID on redelivery. A different later click cannot reuse
the consumed card. These are one-shot controls; send a new card for another
choice because WhatsApp has no edit transport.

Disabling issuance does not invalidate already issued controls. Keep compatible
readers and master keys for their normal 24-hour lifetime. Old Telegram record
bytes and locator formats remain unchanged. No rollout may claim real-platform
acceptance until the authorized live acceptance checks above are recorded.

The existing public option-label bound of 64 UTF-8 bytes also applies.

## Owner-initiated delivery rollout

Owner-initiated delivery has a process default of
`CMAEG_OUTBOUND_MESSAGES_WRITES_ENABLED=false`. The production manifest enables
it after the compatible-reader rollout. Enable only after all gateway
readers include the owner outbound operation kind and deployed NyxID supports
`POST /api/v1/channel-relay/send` with its assigned-key, allow-initiation and
addressable-conversation checks. This does not create or broaden upstream routes.
Media/templates require their existing independent rollout flags. Turning the
writer gate off stops new preparation and commit, while retained receipts and
pre-commit cancellation stay available. There is no background outbound scheduler;
prepared operations require explicit commit. Never retry an unknown send under a
new key. Operation records use the existing encrypted interaction collection and
immutable seven-day TTL; no database migration or additional credentials are needed.

The production authoring configuration also enables
`CMAEG_RELAY_RAW_VERIFICATION_WRITES_ENABLED` independently of dynamic sources
and multi-consumer routing. Owners can select the already implemented raw-event
verifiers for Discord and WhatsApp. This does not subscribe a Discord Gateway
WebSocket or change an existing source's trust mode. Native media, cards,
templates, interactions and multi-consumer writers retain their separate gates.

After activation, verify public capability discovery and an owner-scoped
prepare/read/cancel lifecycle with the existing saved destination. Repeating the
same preparation must return its original operation; a changed payload under
that key must conflict. Cancel before commit and confirm a subsequent read
retains cancellation. These checks exercise the production record store without
sending a platform message. They do not establish real-platform delivery; that
requires an authorized explicit commit and a platform receipt.


## Named-consumer callback rollout

Keep `CMAEG_CONSUMER_INTERACTIONS_ENABLED=false` until all gateway readers support
`callback_owner` in encrypted `ic` interaction records. The flag controls new
named Telegram, Lark/Feishu and WhatsApp controls, including replacement controls;
it does not disable callbacks for retained issued cards. Platform gates and all
existing sender/chat/source admission checks still apply. Plain replacement text
can retire a consumed card while new control issuance is disabled.

A new named card persists its exact consumer ID, provider binding, original
conversation ID, partition key, source and single-consumer routing snapshot.
Consumption verifies the current consumer still exists, is not pending and has
a credential for that same provider binding. Rotating the credential is allowed;
replacing the binding is not. The exact callback actor, chat/thread, route,
platform message and revision/action must still match. Current ACL refusal does
not consume the card. A durable CAS stores the chosen action and first timestamp;
retries reuse that result and consumer-scoped idempotency key, even after restart
or a display-name change. A callback never fans out or falls back to `default`.
Default cards omit the new field and retain their existing bytes and dispatch.
Legacy named records without an owner snapshot remain non-executable.

Before activation, verify a named consumer's card across restart, credential
rotation, duplicate delivery, current ACL revocation, removal/pending state and
provider-binding replacement in authorized test chats. Confirm the callback
reaches only the original consumer conversation. Keep compatible readers and
keys until all issued cards expire; disabling issuance alone is not permission
to remove the retained callback reader. Local fake-transport tests establish the
CMA contract; live platform verification is still required before claiming it.

Remaining CMA dependencies: ordinary named-consumer ingress routing/fan-out and
`publishEvent` execution are not supplied by this callback change. Hand-off
lineage currently persists root/hop/path/reply policy, but not the original human
actor, conversation and callback audience. Hand-off controls therefore remain
refused until CMA persists and validates that original authority through the
publish path. This is a CMA implementation gap, not a NyxID limitation; never
substitute the publishing agent's identity for the original human audience.

The default-publisher lineage correction only makes the sealed reader consistent
with the channel policy: `default` may appear once as a publisher in a bounded
path. It does not change the recipient encoding (`consumer` stays absent for
`default`), tool authentication, channel/record binding or expiry checks. Deploy
compatible readers everywhere before any future writer emits default-publisher
lineage; older readers reject it. Existing references require no migration.

There is still no `publishEvent` REST/MCP execution handler. The pure routing
resolver's `published` input resolves targets but does not authorize publication.
Before that input can be used in production, a CMA executor must authenticate
the exact channel consumer against its unexpired parent reference, check current
`publish.enabled` and allowed targets, preserve the original reply audience,
extend the authenticated lineage without revisits, and enforce the channel's
hop limit, per-root budget, rate limits and idempotency. Target resolution alone
does not provide these checks. Do not expose the tool or enable hand-off controls
until that executor and its delivery path are implemented and verified.

## Typed WhatsApp template rollout

WhatsApp template expansion retains the default-off template gate. Before enabling,
confirm that all CMA writers accept typed header image/video/document references
and body currency/date-time fallbacks, and verify an approved template with the
deployed NyxID/Meta adapter. Native attachment uploads and initiated sends remain
separately gated. This expansion grants no callback, arbitrary metadata, destination
or media-upload authority. Template media links are forwarded to Meta; they are
not downloaded by CMA or rewritten as native attachment uploads.
Media-header replies allow up to 60 seconds for upstream processing and require
a native platform receipt; a deadline or incomplete receipt leaves an unknown
outcome, without retrying the send. Text-only templates retain the normal budget.

## Native Telegram message effects rollout

Deploy compatible readers with `CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED=false` first.
Keep the flag false until every gateway replica understands the optional native
admission/effect fields in sealed EventRef/MessageRef payloads. Old references keep
existing reply/edit behavior and explicitly lack delete/reaction authority. No
migration, new bot credential, NyxID deployment, or user reconnect is required.

After enabling new issuance, confirm `readMessageCapabilities` on a newly delivered
native Telegram `message_ref`. Only confirmed CMA bot messages qualify. A multipart
reply's existing reference names its first part; it never authorizes deleting or
reacting to all parts. Telegram/Lark/Feishu/WhatsApp/Discord **relay** sources remain
unsupported, even if the same channel also has a native Telegram source. Existing
references can be used when issuance is disabled again; their authority lasts at
most 24 hours from the native send attempt. The containing message reference can
still support legacy edits after this authority expires, so 24 hours is not a safe
rollback horizon for old binaries that reject unknown payload fields. Keep a
reader-compatible release until an explicit audit or reference replacement proves
that no enriched references remain usable.

Use one stable idempotency key per explicit delete/reaction action. The persisted
receipt survives gateway restart. A concurrent/abandoned intent returns unknown,
and a timeout, invalid acknowledgement or failed receipt write must never cause
automatic resend. The same key with another payload/action conflicts. A Telegram
429 is returned as rate limited; CMA does not perform a hidden retry. Only a
confirmed Telegram `true` response reports confirmed success.

The effect path rechecks the current channel record, owner, exact source bot,
consumer, provider binding, agent key and routing authority before the durable
claim and again before the request. It reapplies current actor/chat/group admission
to evidence retained from authenticated ingress, including reply-to-bot facts.
A revoked user/source/consumer/key therefore cannot reuse an old reference.
External requests and pending intent outcomes are never logged with credentials
or raw references. Existing sends incur no additional record reads when disabled.


### Relay availability for authorized user testing (2026-10-27)

The desired deployment configuration enables native relay uploads, ordinary text
edits, Lark/Feishu cards and WhatsApp controls/templates after compatible gateway
readers are deployed and predecessor Pods have drained. Existing saved source,
owner, actor, destination, channel-key and provider-permission checks still apply;
WhatsApp edits and native thread operations remain unsupported. Retained message
references and callbacks require compatible readers even if issuance is disabled.

Six uncached health observations identified NyxID `0.73.1` at
`2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f`. The authenticated platform catalog
advertised image/file/audio/video on Telegram, WhatsApp, Lark, Feishu and Discord
and text edits on all except WhatsApp. Matching source implements typed media,
JSON2 cards and callbacks, WhatsApp buttons/lists and approved templates. This
establishes the available integration contract for the user's testing rollout;
it does not establish actual bot permissions, template approval, proxy limits,
all-replica identity or successful external delivery. No NyxID change is needed.

For this authorized testing rollout, the controlled platform checks described
above remain outstanding acceptance work, rather than a prerequisite for making
these implemented features available. Root verifies deployment and isolated
Custom Channel behavior; external platform sends still require a designated
test destination. Record platform acceptance separately from feature availability.

`CMAEG_NYXID_COMPLETE_REPLIES` stays false pending complete-send acceptance.
The implemented ordinary complete-request exchange budget is 15 seconds, and
its non-routed owning tool waits up to 18 seconds. A compatible runtime provides
a 70-second opaque tool-proxy wait; retained runtimes can still have the earlier
10-second cutoff. Verify the deployed splitter's complete receipts, timing and
runtime compatibility before enabling full sends, without interrupting active
Sandboxes. Source budgets and desired configuration are not production acceptance
evidence; do not widen the routed delivery window beyond its leases.
