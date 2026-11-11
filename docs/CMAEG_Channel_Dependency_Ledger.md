# Channel dependency and activation ledger

Friday slice of #1571, audited 2026-11-11 against develop
`a80cc399b7fc715954a868ea300dabeb39b79c89` plus this PR. This is source and
local-fixture evidence, not provider acceptance or activation approval.

Inventory: **7 source variants, 8 channel families, 13 integration variants**.
The source enum is in `api/crates/cmaeg-core/src/channel.rs:70`; the six relay
platforms are in `api/crates/cmaeg-core/src/channel/platform.rs:6`. Lark and
Feishu share one native source type but have separate provider endpoints.
GitHub and Custom are included for completeness, although they are not chat
platforms. Reply sinks, rich-content options and connection custody modes are
not additional channel families.

All channels depend on CMA record storage/authority, the configured agent
provider, and reply delivery. Management uses authenticated owner/organization
access; NyxID relay adds envelope verification and routing. Native automatic
execution also needs durable routing/recovery storage. A flag is not proof of
provider readiness, permission scopes, credentials, or end-to-end delivery.

## Activation and dependencies

Names below have the `CMAEG_` prefix. Unless explicitly stated otherwise,
boolean code defaults are **false** (`config/routing.rs:252`). Exact stored
sources and accepted work are preserved when authoring is paused. Flags that
stop new admissions must not be used to abandon durable recovery.

| Integration variant | External dependencies | New setup / intake gates | Verified locally; remaining gap |
|---|---|---|---|
| Native Discord | Discord Gateway v10/REST, bot token and privileged intents; durable per-bot lease/checkpoint; recoverable agent provider | `NATIVE_DISCORD_WRITES_ENABLED`; runtime `NATIVE_DISCORD_ENABLED`; explicit record selection or `NATIVE_DISCORD_DISCOVERY_ENABLED`; `ROUTED_ADMISSION_ENABLED`, `ROUTED_RECOVERY_ENABLED`, `MULTI_CONSUMER_WRITES_ENABLED` for automatic setup | Disabled authoring refuses before provider effects; UI hides unconfirmed automatic setup. Selected is not connected; #1586 and real traffic acceptance remain open. |
| Native WhatsApp | Meta Graph/Cloud API, app/WABA/phone custody, token permissions, verified webhook | `NATIVE_WHATSAPP_WRITES_ENABLED`, routed admission/recovery and multi-consumer; groups, flows, media and other features have separate gates | Disabled writes/admission make zero Graph/agent calls and store no channel. Existing exact sources survive. No 24h Meta acceptance. |
| Native Lark | Lark app tenant/bot identity, app credentials for verification and encryption, webhook and Lark Open API | `NATIVE_LARK_WRITES_ENABLED`, `NATIVE_LARK_INTAKE_ENABLED`, routed admission/recovery, multi-consumer | Both deployment gates hide setup; disabled writes/admission/routing refuse before platform/agent calls. No 24h Lark acceptance. |
| Native Feishu | Feishu regional API and separate app identity/credentials/webhook | Same gates as native Lark | Same deterministic gate/refusal cases exercised for the Feishu region. No 24h Feishu acceptance. |
| Direct Telegram | Telegram Bot API/token, webhook secret and delivery endpoint | **No general off switch**. Message events/topic creation/lifecycle and media features are separately default-off | Existing protocol/credential tests; general UI/new-admission suppression is missing. |
| NyxID relay Telegram | NyxID relay, signed envelope/JWKS, route key; Telegram account/bot custody at relay | **No per-platform off switch**. Native-event verification advertised enabled | Compatibility/trust tests; full platform suppression and provider acceptance missing. |
| NyxID relay Lark | NyxID relay authority and Lark app routing; optional resource proxy scope | **No per-platform off switch**. Native-event verification enabled; `LARK_RESOURCE_WRITES_ENABLED` gates resource bindings only | Compatibility/trust tests; full platform suppression and provider acceptance missing. |
| NyxID relay Feishu | NyxID relay authority and Feishu regional app routing | Same relay behavior as Lark | Compatibility/trust tests; full platform suppression and provider acceptance missing. |
| NyxID relay Discord | NyxID relay authority and upstream Discord Gateway ownership | **No per-platform off switch**. `RELAY_RAW_VERIFICATION_WRITES_ENABLED` gates raw mode; explicit trusted-normalized remains allowed | Raw-mode refusal before provider effects; full platform suppression and provider acceptance missing. |
| NyxID relay WhatsApp | NyxID relay authority and upstream Meta custody | Same raw/trusted-normalized gates as relay Discord | Raw-mode refusal before provider effects; full platform suppression and provider acceptance missing. |
| NyxID relay Slack | NyxID relay authority and upstream Slack app/scopes | **No per-platform off switch**. Explicit trusted-normalized remains allowed | Trust-required refusal tests; full platform suppression and provider acceptance missing. |
| GitHub | GitHub App/installations, webhook signature, repository approval, integration/guard stores | `GITHUB_ENABLED`; authoring also requires the integration stores/approval path | Disabled source creation/update refused; no GitHub entry in the generic producer picker. Real repository round-trip acceptance is separate. |
| Custom | Caller HMAC and event contract, configured agent provider; optional result webhook | **No legacy ingress off switch**. `CUSTOM_DURABLE_EXECUTION_ENABLED` enables only the new durable mode with recovery/admission/multi-consumer | Durable-mode default-off and retained-mode contracts exist. Turning that flag off does not disable legacy Custom. |

## Code and test evidence

Paths in this section are relative to `api/crates/cmaeg/` unless prefixed `web/`.

- Effective native advertisement: `src/http/relay_capabilities.rs:124` (Lark),
  `:177` (Discord), `:182` (WhatsApp). Relay modes remain advertised in `:109`.
  Effective storage/routing prerequisites are composed in
  `src/serve_source_settings.rs:67`. Discovery describes configuration, not
  a live provider health check.
- Source write gate: `src/channels/write_gate.rs:82`; it precedes provider
  writes and allows exact carry-through/removal. Native Discord socket worker
  returns without creating a client/task when runtime selection is disabled:
  `src/discord/lifecycle.rs:125`. Authoring alone does not activate a socket.
- Discord: `tests/management/native_discord.rs`, especially
  `native_creation_refuses_missing_stable_id_gates_and_nonrecoverable_provider_before_effects`;
  its successful creation/restart cases cover retained definitions/status.
- WhatsApp: `tests/management/native_whatsapp.rs`,
  `native_whatsapp_management_refuses_missing_proof_or_rollout_requirements_before_writes`
  and `native_whatsapp_management_retains_exact_source_and_credentials_with_authoring_off`.
- Lark/Feishu: `tests/management/native_lark_disabled.rs`,
  `disabled_lark_and_feishu_creation_has_no_external_effects` tests both regions
  with writes, admission, and routing gates individually off. The platform
  transport and agent provider see zero calls; no record is stored.
  `tests/management/native_lark.rs` proves retained-source/history revocation
  works while platform access and issuance are off.
- Relay/Telegram: `tests/management/platforms.rs` and `write_gate.rs` cover
  exact legacy carry-through, raw verification opt-in and refusal before
  provider effects. They do not implement a platform-wide kill switch.
- GitHub: `tests/management/github_write_gate.rs`; Custom:
  `src/config/routing.rs` and `web/src/services/channelForm/customExecution.test.tsx`.
- Web: `web/src/services/channelForm/HiddenNativeChannels.test.tsx` tests each
  native activation gate, unknown/failed discovery, new producer attachment,
  enabled setup and a retained Discord edit refused before any fetch.
  `DiscordProviderEligibility.test.tsx` retains the distinct explanation for
  an ineligible agent provider when Discord itself is enabled.
  Lark retention fixtures are split into files below 500 lines; saved source
  editing, revocation, and exact uncertain-request retries remain covered.

## Defaults, verification limits and missing acceptance

The audited **repo manifest explicitly enables** GitHub, native Discord runtime,
writer/discovery, native WhatsApp writer, native Lark writer/intake and the
shared routing flags (`infra/cmaeg/configmap.yaml`). These are overrides of
code defaults, not live-state readbacks or evidence of qualification. This PR
changes no deployment configuration and grants no activation approval.

Of the 13 variants, 4 native variants have the new UI-hide checks plus management
refusal coverage. GitHub and Custom have narrower existing feature gates.
The 7 legacy chat variants (direct Telegram plus all six relays) lack general
per-platform UI/API/new-admission suppression. A follow-up must add a separate
new-admission/discovery policy without disabling accepted-work recovery.

**0 variants have a complete candidate-bound 24h acceptance receipt in this
slice.** Prior aggregate release observations showed successful management
reads but no exercised native Discord flow; they cannot establish delivery
success. #1586 is an unresolved native intake incident, not qualification.

Missing for each exposed third-party variant: approved test account/bot,
provider credentials/scopes and callback setup, exact candidate/environment
and configuration record, >=24h and >=1,000 valid events, >=99.9% durable
receipts/full replies, zero confirmed loss/duplicate/truncation, binding repair
<=5 minutes, contention/provider-start-lag and restart/recovery evidence.
Lark/Feishu multi-image provider acceptance remains unverified after superseding
#1192. Native typing/groups/voice activation decisions are separate from these
tests. Keep #1571 open; local fixtures do not satisfy its real-provider gate.
