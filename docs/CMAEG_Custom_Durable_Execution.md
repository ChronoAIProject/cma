# Self-service durable Custom channels

New channel creation may set `durable_custom_execution: true`. This immutable
owner choice stores the marker and default-only routing together. When the owner
supplies a reserved `record_id`, the definition is retained before binding side
effects; the older non-reserved create API keeps its existing bind-before-store
ordering. It works for both profile-created consumers and existing CMA Agent
targets. Authenticated Custom events then use the existing durable admission,
provider preparation, execution, delivery and recovery machinery without an
operator adding the channel UUID to `CMAEG_ROUTED_CHANNEL_RECORDS`. The newly
opted-in default consumer explicitly accepts non-human Custom events; this changes
no provider binding, credential, destination or current authority check.

The creation request must contain a Custom source. Every configured consumer
must have explicit provider response-recovery paths. Discovery exposes
`relay_authoring.custom_durable_execution_enabled` and the existing
`response_recovery_providers` list. These describe deployment configuration;
they do not claim that a provider connection is healthy. Native and relay events
on a mixed channel retain their own selection rules. Event payloads and unsigned
binary-mode CloudEvent headers cannot enable this mode or grant routing rights.

Missing or false creation fields preserve the old serialized definition and
creation fingerprint. Existing `routing` alone never enrolls a channel. Full PUT
updates retain the saved marker and routing, including old clients that do not
know the field. A stale client may echo the same marker; mismatches are refused.
The marker cannot be added, cleared or changed by updates; use a
new channel to change execution mode. Existing-channel retries keep their original
execution path. Explicitly enrolled historical record IDs retain their existing
operator selection and replay behavior.

Attachments still require the existing Custom `attachments` configuration and
media capability gates. Existing-Agent media delivery and late transcript
continuations still require the media continuation writer/index and compatible
Agent Manager; this feature does not bypass those dependencies, origin checks,
source/consumer authorization, byte bounds, or exact retained event context.
New existing-Agent attachment contracts are refused before effects with
`custom_existing_agent_media_unavailable` unless the concrete provider continuation
adapter, media writer, index and media service are ready. The same condition is
checked on fresh attachment admission, after committed replay lookup. Retained
unchanged configuration remains editable while issuance is paused.
Original event commands and media continuations keep their independent existing
idempotency keys and replay rules.

## Deployment and rollback

The checked-in ConfigMap enables `CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED=true`.
With routed recovery/admission and multi-consumer authoring enabled, discovery
advertises `relay_authoring.custom_durable_execution_enabled: true`. Existing
channels keep their saved execution mode; only new channels can opt in.

### Reader evidence and image promotion (2026-11-08)

Reader support was added by
[`ee4540d9d512a673d47dd3364a3a9276e40b05d6`](https://github.com/ChronoAIProject/cma/commit/ee4540d9d512a673d47dd3364a3a9276e40b05d6).
The release base `619655da83fb4437cc613f743f70ac81a9653938` on develop contains
that commit. A read-only production check on 2026-11-08 at 03:07 UTC found:

- [Live OpenAPI](https://cma.nyxid.ai/event-gateway/openapi.json) exposes
  `POST /v1/github/apps`, `PUT /v1/github/apps/{app_ref}/webhook-secret`, and
  `POST /v1/github/apps/{app_ref}/verification`.
- Authenticated `GET https://cma.nyxid.ai/event-gateway/v1/channel-capabilities`
  returned HTTP 200 with `relay_authoring.custom_durable_execution_enabled: true`,
  `multi_consumer_writes_enabled: true`, and
  `response_recovery_providers: ["cma_codex", "nyxbot"]`.

The discovery field proves the serving binary includes the Custom reader; its
observed value was already **true**, so this manifest change records the enabled
production setting. This read did not create a channel or prove event delivery.

The checked-in Gateway image pin still names the older `615c6f23` source, which
predates the reader and lags production. It is not the rollback target for this
release. Image pins are unchanged here: Shining's release promotion must pin
images containing the develop head above and this configuration change. Retain
the actual compatible live Gateway image as the rollback target and record its
digest during promotion; do not redeploy the stale checked-in image pin. This
discovery check establishes reader capability, not an exact live image digest.

### Activation and rollback procedure

1. Preserve the verified live reader for rollback and deploy the compatible
   release reader to every Gateway replica. On a deployment without a compatible
   reader, first deploy with `CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED=false` and
   wait for predecessor Pods to drain before enabling authoring.
2. Verify routed recovery, admission, multi-consumer authoring, provider replay
   paths and the shared synchronous delivery registry. With the checked-in flag
   enabled, confirm discovery reports true after promotion.
3. Create a fresh owned Custom channel using ordinary owner APIs, send one
   authenticated event, retain its HTTP/SSE result and durable response, and
   verify first-event, replay and permitted attachment delivery. Do not send a
   second event to guess an ambiguous first outcome.

Set `CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED=false` to roll back authoring and
confirm discovery reports false after configuration rollout. This stops
authoring new opted-in channels. It does not demote
retained marked channels to the legacy runner. Disabling durable admission
refuses new events while exact accepted retries and recovery continue. If the
routing reader, provider replay capability or required durable dependencies are
missing, marked Custom events fail closed rather than launching a second legacy
turn. Keep recovery and compatible readers available while retained work exists;
a pre-marker binary is not a safe rollback target after the first marked write.
The routing rollback audit refuses `--clear` for marked records before any
consumer unbind or record mutation, even when default widening is allowed.

## Regression coverage

Tests cover absent/false wire and fingerprint compatibility; gated create before
side effects; both profile and existing-Agent targets; reserved replay and mode
conflict; old-client PUT after writer shutdown; actual Custom-only automatic
selection; unmarked routing records and mixed-source legacy traffic; missing
store/replay/routing/admission without fallback; original signed-body replay;
unsigned target rejection; and public signed inline attachment delivery and tool
reads through the same routed HTTP fixture with and without operator selection.
Rust tests execute in GitHub CI; live acceptance is a separate deployment step.
