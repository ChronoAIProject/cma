# CMA Event Gateway operations

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
The catalog REST base is `https://bot-gateway.chrono-ai.fun/v1`, with OpenAPI at
`https://bot-gateway.chrono-ai.fun/openapi.json`. The `/v1` base is necessary
because the NyxID OpenAPI parser discovers paths but does not apply `servers`.
The public/internal service is auto-connected and forwards caller bearers;
channel ownership and credential-specific gateway checks remain authoritative.

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
applies with all media flags off. `--audit-custom-attachments [--clear]` is an
operator command, not a serving mode: it builds the same runtime, binds no
listener, starts no worker and exits when its report is printed (see
[Rolling back below inbound media](#rolling-back-below-inbound-media)).

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
| `CMAEG_MEDIA_TOOL_PROVIDERS` | comma-separated provider slugs | `cma_codex` when that provider is configured, otherwise empty | 0..=8 distinct slugs, each naming a `CMAEG_PROVIDERS` entry; startup fails otherwise. Profile bindings of these providers install the gateway MCP tools, so only their agents are told how to open an attachment through a gateway tool. Set it to the empty string to render no tool hints. A separate key because provider entries refuse unknown fields. |
| `CMAEG_MEDIA_ENABLED` | `true` or `false` | `false` | Master switch for inbound media. Exactly `true` or `false`; any other value, including `True` or `1`, fails startup. Off, every media message is refused with 202 `attachment_unsupported`, and every Lark or Feishu rich-text post with `content_unsupported`, exactly as before media existed. On, authenticated Telegram media from the NyxID relay and the native webhook, and Lark and Feishu image, file, audio and media messages from the relay, are admitted as activity attachments, and Lark and Feishu posts as text the observer flattens from the raw event (protocol §6); Lark stickers stay refused. It also governs custom sources: off, management refuses a custom `attachments` setting with `custom_attachments_disabled` and ingress ignores a stored one; on, the setting can be stored and the items it names in custom events are admitted as attachments. Enabling it requires the `cmaeg` container's memory request to equal its limit at 1536Mi or more; `tests/infra_manifests.rs` enforces this. |
| `CMAEG_MEDIA_MAX_BYTES` | unsigned decimal, bytes | `20971520` | 1..=26214400. Most bytes fetched for one attachment; Telegram and NyxID serve at most 20 MiB themselves. Validated at startup even while `CMAEG_MEDIA_ENABLED` is `false`, like every media budget below; an out-of-bounds or non-decimal value fails startup naming the key. |
| `CMAEG_MEDIA_INFLIGHT_BYTES` | unsigned decimal, bytes | `67108864` | `CMAEG_MEDIA_MAX_BYTES`..=1073741824. Pod-wide bytes held at once by fetched originals and transform outputs; a reservation larger than the budget takes all of it, so one maximum-size original can always proceed. |
| `CMAEG_MEDIA_RESPONSE_BYTES` | unsigned decimal, bytes | `25165824` | 4194304..=134217728. Pod-wide weighted bytes of media response bodies in flight. Each body releases its share at a hard deadline even when its reader never polls, so a stalled client cannot hold it until the load balancer's 660-second timeout. |
| `CMAEG_MEDIA_MAX_JOBS` | unsigned decimal | `6` | 1..=64. Pod-wide concurrent media jobs. Each channel runs at most two at once, so one channel cannot hold every job permit. |
| `CMAEG_MEDIA_CACHE_BYTES` | unsigned decimal, bytes | `33554432` | 0..=268435456. Capacity of the per-pod derived-media cache, weighted by bytes, with a 30-minute lifetime; `0` disables caching. |
| `CMAEG_MEDIA_RATE_PER_MINUTE` | unsigned decimal | `60` | 1..=6000. Media fetch starts per channel per minute, with a burst of the same size. |
| `CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR` | unsigned decimal, bytes | `268435456` | 20971520..=4294967296. Download bytes per channel per hour, counted in KiB; enforced once download links exist. |
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
| `CMAEG_PUBLIC_URL` | origin URL | Required | Externally reachable origin used in channel callback and tool URLs, without a trailing slash. HTTPS, or loopback HTTP for local tests. |
| `CMAEG_MASTER_KEYS` | secret string | Required unless file source is set | Kubernetes Secret environment value containing `kid:base64key[,kid:base64key]`; each key decodes to 32 bytes and the first key is current. |
| `CMAEG_MASTER_KEYS_FILE` | file path | Optional alternative | Compatibility file source for the same key ring; mutually exclusive with `CMAEG_MASTER_KEYS`. |
| `CMAEG_CMA_BASE_URL` | origin URL | Required | CMA record-store origin. HTTPS, or loopback/`.svc` HTTP; no credentials, path, query, fragment, or trailing slash. |
| `CMAEG_CMA_SERVICE_TOKEN` | secret string | Required unless file source is set | Kubernetes Secret environment value authenticating inbound gateway rekey. Also used for outbound CMA authentication when the service-account client pair is absent. |
| `CMAEG_CMA_CLIENT_ID` | string | Optional pair | Dedicated NyxID service-account client ID, injected from Kubernetes Secret. Must be set together with its client secret. |
| `CMAEG_CMA_CLIENT_SECRET` | secret string | Optional pair | Exchanges renewable outbound CMA tokens at the configured NyxID origin using `client_credentials` and `openid roles`. The account role grants `cma:gateway:records` and `cma:provider:bindings`. |
| `CMAEG_CMA_SERVICE_TOKEN_FILE` | file path | Optional alternative | Compatibility file source for the same bearer; mutually exclusive with `CMAEG_CMA_SERVICE_TOKEN`. |
| `CMAEG_NYXID_BASE_URL` | origin URL | Required | NyxID origin for creator authentication, with the same URL restrictions as the CMA origin. |
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
| `CMAEG_ACK_TEXT` | string | `Working on it…` | Progress acknowledgement for long turns; nonempty text up to 1024 bytes without controls. |
| `CMAEG_HEARTBEAT_MS` | positive integer, ms | `60000` | Inactivity after acknowledgement before a heartbeat, capped at five per turn. |
| `CMAEG_FAILURE_TEXT` | string | `Sorry, I couldn’t finish that request. Please try again.` | Short failure notice; same bounds as acknowledgement text. |
| `CMAEG_DRAIN_SECONDS` | positive integer, seconds | `600` | Maximum shutdown drain for tracked turns and HTTP requests. |
| `CMAEG_BUSY_RETRY_MAX_MS` | positive integer, ms | `180000` | Maximum interval for retrying upstream busy refusals, including first-conversation preparation. Jitter is added after the provider's `Retry-After` minimum. |

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
| `cmaeg_media_prepare_seconds` | `outcome` | Time a media turn spent on its eager items (transcripts and excerpts), recorded only when at least one ran. `outcome` is `ready` when every eager item was ready, otherwise `pending` when any was still being prepared at the budget, otherwise `unavailable`. The `cmaeg.media.prepare` span carries `items`, `eager`, `outcome` and the channel's hash, never a file name, transcript or excerpt. Uses the media buckets. |
| `cmaeg_media_fence_wait_seconds` | none | Time a turn waited for an earlier turn of its conversation to return from its provider start, recorded only for a turn that had such a predecessor. A turn waits at most the busy-retry bound plus the preparation budget (`CMAEG_MEDIA_PREPARE_BUDGET_MS`) plus five seconds and then proceeds out of order with a `conversation order fence timed out` warning. Uses the media buckets. |
| `cmaeg_media_transform_total` | `op`, `outcome` | Sandboxed media jobs (see [Media job sandbox](#media-job-sandbox)). `op` is `probe`, `frame`, `audio` or `sandbox_check`; `outcome` is `ok`, `timeout` (the gateway's deadline), `cpu_limit` (the 20 s CPU limit), `resource_limit` (the memory watchdog), `killed` (any other SIGKILL), `exit_nonzero`, `output_invalid` (over its stdout cap or failing validation) or `sandbox_unavailable` (exit 125, or the job could not be started or observed). The `cmaeg.media.transform` span carries `op`, `outcome`, `ms`, `cpu_ms` and `stdout_bytes`, never FFmpeg's output, a file name or media bytes. Zero while `CMAEG_MEDIA_FFMPEG_DIR` is empty. |
| `cmaeg_media_transform_seconds` | `op` | One sandboxed media job, from spawn to reap. Uses the media buckets. |
| `cmaeg_media_sandbox_ready` | none | `1` once the media transformer's startup sandbox self-check passed; `0` while the transformer is not configured. |
| `cmaeg_stt_audio_seconds_total` | none | Speech-to-text audio seconds counted for finished calls: the metered seconds, or the provider's reported usage when it is larger. |
| `cmaeg_stt_channels_at_budget` | none | Distinct channels refused by their own `CMAEG_STT_CHANNEL_SECONDS_PER_HOUR` budget within the last hour. It is refreshed on each such refusal and at least every 256 charges. |
| `cmaeg_turns_inflight` | none | Admitted local consumers; decremented on completion or cancellation. |
| `cmaeg_channels_cached` | none | Locally retained positive and negative cache entries, including entries awaiting eviction. |

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
each caller starts after cache invalidation, and the eight callers share one
restore flight. It asserts 25 additional CMA reads, no additional JWKS fetches,
launch p99 at most 2000 ms, and restore wait p99 at most 800 ms. The initiating
caller owns the `cold=true` metric sample; all eight wait on the cold cache.
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

Run the gateway as a separate stateless workload with its public listener and
private metrics listener on separate service ports. Readiness is `/readyz`;
liveness is `/healthz`. Readiness gates loaded master keys, parsed provider policy and draining state.
Its CMA reachability field is diagnostic only; remote outages do not remove a
warm replica from service. A public route must never forward the private scrape port.
`infra/cmaeg/` declares a separate `cmaeg` Deployment with **one replica** in
`chrono-bot`, matching the maintainer's 2026-09-22 deployment decision. CMA API
also keeps one replica. The gateway listens on 8090; its private metrics listener
uses 9091 and is not exposed through the public Service. The HTTPRoute attaches
to `gateway-infra/chronoai-gateway` at `bot-gateway.chrono-ai.fun`; public DNS and
certificate coverage remain cloud-side prerequisites. The single-label hostname
fits the existing wildcard certificate.

Use native Kubernetes Secrets, injected as environment variables through explicit
`secretKeyRef` entries. `cmaeg-master-keys` supplies `CMAEG_MASTER_KEYS`, and
`cmaeg-cma-service-token` supplies `CMAEG_CMA_SERVICE_TOKEN`,
`CMAEG_CMA_CLIENT_ID`, and `CMAEG_CMA_CLIENT_SECRET`. Populate both Secrets
out of band. `secret.yaml` contains names and key contracts only, never values.
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
permission and attribute changes; a test fails if it ever does.

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
to a native Telegram thumbnail when the attachment has one. Every
speech-to-text input, audio or the audio track of video in any container, is
transcoded to 16 kHz mono PCM covering at most
`T = min(CMAEG_STT_MAX_SECONDS, ⌊(CMAEG_STT_MAX_BYTES − 44) / 32000⌋)` seconds
(600 at the defaults) and uploaded as WAV, metered exactly at
`⌈PCM bytes / 32000⌉` seconds; a transcript cut at `T` carries the notice
`[transcript covers the first m:ss]`. A failed transcode still uploads the
original directly when the direct path accepts it; otherwise the transform's
reason is reported. An endpoint whose `CMAEG_STT_FORMATS` omits `wav` keeps the
direct path.

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
| 2 | Usage error; stderr carries `usage: cmaeg [--print-openapi \| --audit-custom-attachments [--clear]]`. |

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
