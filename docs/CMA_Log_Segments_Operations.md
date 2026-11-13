# Agent log segmentation configuration and activation

Part of #1348–#1351. Segmentation is an additive reader/writer migration with
independent process configuration and durable admission gates. Deploying an
image, enabling a read optimization, or installing the schema does not authorize
new segmented publications or native recovery.

## Process configuration

These switches accept only the exact strings `true` and `false`. They default
to `false`; malformed values fail startup. They are parsed once. Changes require
the release owner's API Pod rollout and do not edit durable gates or records.

| Setting | Scope |
| --- | --- |
| `CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED` | Enables the independently configured transcript source. It does not enable title, usage, response, writer or recovery behavior. Qualify segmented/current-state and long-history compatibility before writer activation. |
| `CHRONO_AGENT_USAGE_BOUNDED_READS_ENABLED` | Enables the usage reader through strict process configuration and production store composition. The disabled path preserves the legacy reader. Current legacy filtered-query coverage is not proof of bounded work on segmented histories. |
| `CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED` | Default-off process dispatch for natural v1-to-v2 adoption. Off skips the entire adoption preflight, including gate queries and transactions; ordinary v1 work uses its original Mongo commands and existing v2 roots remain compatible. On permits discovery only: the durable writer gate still selects and fences every new adoption. |
| `CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED` | Permits new personal capacity-recovery admission after current authorization and both durable cohort gates succeed. Status and exact already-admitted continuation retain fresh authority checks when admission is disabled. This switch does not provide a native proof capability or authorize ordinary reconnect/input replay. |
| `CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED` | Enables the independent parent-readiness pacing path. It uses persisted parent state and existing retry fences; it does not authorize a missing native recovery capability, alter writer admission, or revive a stopped parent. |

The independent response flags remain documented in
[CMA bounded responses](CMA_Bounded_Responses.md). Their legacy optimization
switches are not the durable segmented-writer controls.

The runtime has a separate startup switch,
`CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED`, default `false` in both runtime
image variants. It accepts `false`/`0` and `true`/`1`; other values fail startup.
With it off, bridges allocate no native observer and bypass the recovery JSON
decoding and state locks on every input/output frame. Proof requests are refused.
Turning on the API admission switch cannot enable observation in an existing
runtime. A compatible runtime must observe the original successful session
load/new and exact prompt/permission correlations before it can issue proof.
Runtime replacement remains subject to the retained-Sandbox continuity rules.
Once native correlation is lost, later load success cannot erase the uncertainty;
those cases remain blocked for explicit reconciliation.

A recovery challenge names one 30-second snapshot and is single-use. A newer
writer fence supersedes an older lease; consumed and expired fences cannot be
reissued. State changes, pending output retention, output progress, output ACKs,
and gaps between proof and attachment refuse that snapshot. Retry through the
fenced recovery worker with renewed authority; never replay a prompt or create
another conversation to resolve uncertainty. Output following a successful
attachment uses the existing acknowledged stream and original durable receipts.

## Activation prerequisites

The reviewed combined artifact must have strict local gates, its exact develop
CI artifact, complete ordinary writer roots, indexed response/transcript reads,
canonical format-2 recovery schema and exact runtime grants. The original result
witness is retained; draft v1 work is never registered or relabeled. Read-only
consumer fallbacks must not audit, quarantine or mutate retained history.

The segmentation operator entry performs read-only schema/authority preflight
before any explicit control or audit action. See the
[operator contract](notes/2026-11-11-segment-operator-entry.md) and
[coherent recovery inventory](notes/2026-11-11-segments-authority-backup.md).
Existing non-segmentation store initialization is unchanged; no segmentation
sidecar DDL or adoption is performed by service startup.

The pre-review findings remain activation gates until their executable evidence
and exact-head review pass. In particular, current retained reopening needs its
bounded original-history proof on segmented data. Generation maxima alone do
not establish its history digest. Missing native state/output confirmation is
an explicit unsupported capability, never permission to create a replacement
conversation or replay input.

## Kill switch and recovery floor

Writer activation requires both the process adoption switch and the durable
cohort gate. Roll out the process switch only for the controlled writer stage.
For rollback, close the durable gate first and resolve exact admitted uncertainty
with the switch still on. Once those preparations have drained, roll out
`CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED=false` to restore the original
request-path operation count. Durable closure alone, while dispatch stays on,
still performs discovery; it must not be reported as zero-overhead rollback.

Capacity recovery retries an uncertain Mongo commit on the same session and
transaction, up to five commit attempts. It never repeats the mutation or native
input as part of that retry. A later transient or unclassified failure cannot
erase an earlier uncertain result. If the bound expires, retain the original
operation and reconcile its exact durable receipt; an absent receipt remains
unknown, including after restart. A gate-admission receipt alone does not prove
that storage recovery published. No status or kill-switch operation promotes
that absence to a known abort.

Close new durable writer/recovery admission first when a stage fails. Preserve
original receipts, carrier result witnesses, metadata, log rows and native homes.
Accepted uncertainty resolution and settlement use their original operation and
current authority; closure does not establish KnownAbort or authorize replanning.

After the first v2 adoption, retain the compatible reader artifact and all
configuration needed to serve v2. Switching an optional reader off is not a valid
rollback if that selects a whole-history legacy fallback. Qualify the minimum
reader configuration with retained adopted data and with new admission closed.
An older executable that refuses v2 is excluded from the recovery set.

Only the release owner changes production configuration, grants, schema or
cohorts. Record each stage's nonzero route samples, source work/bytes, checkout
wait, error/timeout rates and exact source/image/configuration. Source merge,
flag activation and metadata discovery do not establish completed recovery or
issue acceptance.
