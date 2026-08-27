# CMA Trigger operations

This is the deployment and incident runbook for the GitHub Trigger workflow
defined in [CMA_Trigger.md](./CMA_Trigger.md). The design contract remains the
authority when this runbook omits a product rule. `CLAUDE.md` section 5 remains
the authority for session continuity and cannot be waived by a feature flag or
an incident response.

**Runbook owner:** [`chronoai-shining`](https://github.com/chronoai-shining).
The owner keeps this procedure, alert catalogue, saved-dashboard mappings, and
drill evidence current and leads the Trigger incident handoff unless an
incident commander assigns another responder. Ownership does not grant access
to credentials, repository contents, or another user's Launch.

**Evidence status:** this runbook specifies the required recovery drills but
does not claim they ran. A drill is complete only when its redacted transcript
and alert/dashboard intervals are linked from
[CMA_Trigger_Release_Readiness.md](./CMA_Trigger_Release_Readiness.md).

## 1. Non-negotiable boundaries

- A rollout control may stop a new Launch or freeze its next durable effect. It
  must never terminate, restart, rotate, blank, or degrade a running session.
- A capability kill switch affects subsequent GitHub capability use. It does
  not end the conversation or discard the workspace.
- Effective revocation is reported only after Layer 4 acknowledges a higher
  generation tombstone or NyxID confirms agent-key revocation. Until then the
  state is `revocation_pending`. A disconnected stale policy is bounded by its
  60-second lifetime plus at most 5 seconds of accepted clock skew.
- Trigger cleanup deletes only CMA-owned baseline, checkpoint, and diff-artifact
  objects named by the retained Launch plan. It never enumerates or deletes a
  sandbox, fork, branch, commit, pull request, or any other GitHub resource.
- A provider timeout after a possible write is reconciled from numeric identity
  and retained intent. It is never converted into a blind retry.
- No prompt, task input, repository file content or path, request body, OAuth
  value, agent key, cookie, authorization header, or provider response body may
  enter an activity record, support response, or structured worker log.
- Sponsored credentials remain disabled for the MVP. Every public Launch uses
  the contributor's selected NyxID GitHub connection.

## 2. Worker activation prerequisites

The public preview, review, and support surfaces can run without provider
workers. The in-process scheduler starts only when all of these exist:

1. CMA runs in provider authentication mode and constructed the typed NyxID
   GitHub broker.
2. `CHRONO_PLATFORM_BASE_URL`, `CHRONO_STORAGE_CLIENT_ID`,
   `CHRONO_STORAGE_CLIENT_SECRET`, and `CHRONO_TRIGGER_WORKSPACE_BUCKET` produce
   a durable Trigger workspace store.
3. `CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID` and
   `CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8` produce a usable policy signer.
4. The normal `SessionService` is available for sandbox, session, prompt,
   checkpoint, and runtime-policy effects.
5. The one executor implements every closed `LaunchEffectKind`, including
   `Cleanup`.

Startup logs `Trigger provider workers are disabled` with the exact
`missing_prerequisites` list when any item is absent. Trusted local auth never
starts provider workers. Do not work around this gate by constructing a partial
executor.

The scheduler still starts when `CHRONO_TRIGGER_ENABLED=false`, provided all
prerequisites exist. That is intentional: orchestration records a durable pause
while the independent capability lane can apply the kill switch to reach that
already exists.

## 3. Deployment controls

All controls are non-secret ConfigMap values. Invalid booleans, cleanup modes,
numeric identities, percentages, page sizes, page budgets, and concurrency
values fail startup rather than falling back to a broader policy.

| Key | Initial value | Operational meaning |
|---|---:|---|
| `CHRONO_TRIGGER_ENABLED` | `false` | Environment admission. `false` refuses new publisher and Launch admission and pauses existing orchestration at its next durable boundary. |
| `CHRONO_TRIGGER_ORCHESTRATION_PAUSED` | `true` | Emergency freeze for new Launch effects and not-yet-dispatched prompts. It does not interrupt a session. |
| `CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH` | `true` | Requests capability revocation and prevents renewal. It remains independent from orchestration admission. |
| `CHRONO_TRIGGER_RUNTIME_CAPABILITIES` | empty | Exact comma-separated capabilities provided by the pinned runtime image. Empty fails Trigger admission closed while ordinary sessions remain available. |
| `CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS` | empty | Comma-separated exact NyxID subject IDs allowed to publish or launch. Empty denies all. |
| `CHRONO_TRIGGER_REPOSITORY_IDS` | empty | Comma-separated numeric upstream GitHub repository IDs. Names are not accepted. Empty denies all. |
| `CHRONO_TRIGGER_USER_COHORT_PERCENT` | `0` | Stable SHA-256 subject cohort from 0 through 100. Zero admits no contributor. |
| `CHRONO_TRIGGER_CHANGED_FILE_LIMIT` | `1000` | Maximum files in one reviewed publication, bounded from 1 through the complete-review schema ceiling of 1,000. Retain pinned-provider measurement evidence for the selected deployment value. |
| `CHRONO_TRIGGER_CLEANUP_MODE` | `dry_run` | `dry_run` persists an exact plan but has no deletion port. `execute` sends eligible cleanup through the normal fenced effect worker. |
| `CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS` | `5` | Cadence of each independent lane, 1 through 300 seconds. Missed ticks are skipped. |
| `CHRONO_TRIGGER_WORKER_PAGE_SIZE` | `50` | Candidate rows per page, bounded by the public core maximum. |
| `CHRONO_TRIGGER_WORKER_MAX_PAGES` | `20` | Hard page ceiling per lane and pass, 1 through 100. |
| `CHRONO_TRIGGER_WORKER_CONCURRENCY` | `4` | Hard concurrent candidate ceiling per lane, 1 through 32. |

The public MVP's Trigger kernel is not a contributor or publisher control. The
composition root authoritatively pins it to Codex, every Launch review discloses
`kernel: codex`, and Start/provisioning fail closed if that selection drifts.
Runtime version strings and capability lists are never parsed to infer it.

Admission requires all of the environment, publisher, numeric repository, and
user-cohort decisions to permit the request. Turning on only the environment
flag cannot expose a Trigger accidentally. A README re-click returns the
contributor's current Launch before new-admission checks, so a pause does not
strand its review or support view.

The only accepted runtime capability names are `conversation`,
`durable_conversation`, `workspace_checkpoint`,
`restricted_service_gateway`, `credential_refresh`, and
`credential_revocation`. Names are case-sensitive; whitespace, duplicates, and
unknown values fail startup. Declaring a name is necessary but not sufficient:
admission also intersects the declaration with the Codex adapter's conversation
and snapshot support and the configured workspace and credential ports. A
missing capability refuses review and is checked again at Start and immediately
before sandbox provisioning. It never silently weakens a Trigger session.

### Trigger runtime listener boundary

Codex permission-profile host rules do not scope ports. The Trigger profile
therefore allowlists exactly `127.0.0.2` and the runtime reserves every
privileged port on that address that could otherwise expose a credential or
control endpoint:

| Listener | Purpose | Sandbox result |
|---|---|---|
| non-loopback pod IP on `9090` | Actual runtime supervisor ingress | Denied by the managed network allowlist |
| `127.0.0.1:9091` | Actual model credential proxy | Denied by the managed network allowlist |
| `127.0.0.2:9090` | Closed supervisor-denial front | `403` with `x-cma-trigger-loopback-isolation: denied` |
| `127.0.0.2:9091` | Closed model-proxy-denial front | `403` with `x-cma-trigger-loopback-isolation: denied` |
| `127.0.0.2:9092` | Signed Layer 4 platform and GitHub gateway | Permitted only after exact signed-policy evaluation |

A Trigger runtime must resolve a non-loopback ingress address and must have
both model and platform upstreams. Wildcard or loopback supervisor ingress,
missing upstream configuration, or a collision on any reserved listener exits
startup. The supervisor does not bind and expose `/readyz` until both denial
fronts and both credential proxies have bound successfully, so a partial
topology is never Ready.

Every Reach activation runs the pinned `runtime_gateway_probe_v1` through the
same Codex command sandbox that will receive the first prompt. The probe must
positively observe both closed denial fronts, a `blocked-by-allowlist` denial
for an unlisted private loopback address and a public destination, and an
allowed numeric-resource read through `127.0.0.2:9092`. Connection refusal,
missing or different headers, malformed receipts, or any broader reach is a
typed failure and no prompt is sent. Release evidence must additionally show
that raw `CONNECT` cannot tunnel to the real supervisor or model proxy.

Both credential proxies parse their configured base and the requested URI as
structured URLs. They pin scheme, origin, and service-path prefix and reject
literal, percent-encoded, recursively encoded, mixed-dot, slash, or backslash
traversal before credential lookup or outbound execution. Rejection logs only
the closed refusal fact; paths, headers, credentials, and upstream bodies are
not retained.

### Same-runtime policy recovery and audit

A provider may restart the supervisor inside an otherwise unchanged runtime,
leaving the opaque runtime handle unchanged while erasing the supervisor's
in-memory GitHub policy. Readiness reports this as the explicit empty tuple:
`installed=false`, `revoked=false`, no Launch/generation/fence/expiry identity,
and an optional closed refusal code. CMA observes readiness while holding the
session lifecycle fence, pairs the strictly typed snapshot with the exact
current handle, and redelivers the current signed policy or tombstone to that
same handle before allowing further GitHub use.

This is policy redelivery, not session recreation, runtime rotation, evidence
of revocation, or permission to widen authority. An unavailable readiness
endpoint, unsupported version, unknown refusal code, malformed tuple, or
inconsistent tuple fails closed. CMA never reconstructs dynamic policy state
from the runtime image's static capability advertisement.

Every GitHub request evaluated by the Layer 4 gate emits exactly one audit
decision event. Its stable identity is target
`cma_trigger::github_capability_audit`, `event_name` value
`cma_trigger_github_capability_decision`, and `schema_version=1`. The event
contains `allow` or `deny`, one closed decision code, Session and Launch
identity, operation class, and only the approved numeric upstream/fork
repository and owner identities. It never contains a request path or body,
canonical repository name, NyxID service ID, credential, header, or provider
response. Missing, duplicate, or open-shaped decision events are release and
incident stop conditions.

## 4. Staged activation

Apply one control change at a time and retain its configuration revision in the
change record.

1. Deploy code and database indexes with the initial values above. Confirm all
   four scheduler lanes (`effects`, `capabilities`, `workspaces`, and `cleanup`)
   emit heartbeats and no provider worker prerequisite is missing.
2. Keep orchestration paused and the capability kill switch on. Set
   `CHRONO_TRIGGER_RUNTIME_CAPABILITIES` to the exact capabilities demonstrated
   by the pinned runtime image, then verify one supported and one deliberately
   omitted-capability review. Add one approved publisher subject and one
   disposable public fixture's numeric repository ID.
3. Set the user cohort to a small non-zero percentage. Record deterministic
   in-cohort and out-of-cohort fixture subjects; admission remains masked by the
   environment and orchestration gates until the later steps.
4. Leave cleanup in `dry_run`. Review every eligible plan as described in
   section 8.
5. Turn `CHRONO_TRIGGER_ENABLED=true` while orchestration remains paused. Verify
   previews, reviews, re-clicks, support reads, and typed `feature_paused`
   projections.
6. Turn the capability kill switch off only after the signed Layer 4 evaluator,
   expiry, replacement, and revocation evidence gates are attached.
7. Unpause orchestration for the disposable repository and cohort. Verify that
   the recorded in-cohort subject is admitted and the out-of-cohort subject gets
   the typed cohort denial, then run the fault-injected Launch-to-PR drill before
   widening either allowlist or cohort.
8. Promote cleanup to `execute` only after the dry-run inventory and deletion
   canary meet section 8.

Never combine a runtime-image adoption with the Trigger cohort change. A runtime
image change rotates sessions under the normal session continuity protocol and
must be evaluated independently.

## 5. Pause and kill-switch matrix

An external call already in progress may finish or become ambiguous and enter
reconciliation. A flag cannot recall bytes already written. The next effect is
always claimed only after a durable Launch lease and a fresh rollout decision.

| Launch state | Orchestration pause | Capability kill switch | Session and user work |
|---|---|---|---|
| `draft` | No new Start or provider effect. Existing review remains readable. | Normally no reach exists to revoke. | No session is created or stopped. |
| `auth_required` | Keep the failed effect on its original intent. Successful connection revalidation queues read-only reconciliation of that intent and returns to `resume_state`; it never repeats Start or blindly executes the provider write again. | Revoke retained reach if one exists and the failure requires it. | Any existing session and workspace remain intact; recovery creates neither. |
| `preflight`, `fork_pending`, `fork_ready`, `branch_pending`, `workspace_pending`, `sandbox_pending`, `session_pending` | Finish only the external boundary already in progress, retain its result, then persist `feature_paused` before another claim. | Persist `revocation_pending`, then seek tombstone or agent-key acknowledgement. | A session that already exists stays intact; no inferred rollback deletes earlier resources. |
| `prompt_pending` | Do not claim a not-yet-dispatched prompt. A committed dispatch fence follows the seven-state at-most-once recovery contract and is never blindly resent. | Revoke subsequent GitHub reach without ending the session. | Conversation, workspace, and visible history remain. |
| `ready` | No new automatic GitHub/checkpoint effect is claimed. | Stop renewal and converge to effective revocation. | The session remains available for conversation, inspection, export, and recovery subject to the now-revoked GitHub reach. |
| `contributing`, `pr_pending` | Freeze the next effect. Reconcile a possibly written checkpoint, branch, or PR before settling. | Stop subsequent session GitHub use; CMA-owned ambiguous writes still reconcile under their retained intent. | Workspace, fork, branch, commits, and PR are preserved. |
| `pr_open` | No new Trigger write is claimed. | Revoke remaining session reach. | The open PR remains on GitHub and the session is not terminated. |
| `failed_retryable` | Preserve retry time and durable intent but do not retry while denied. | Revoke reach independently. | Recovery information and session linkage remain visible. |
| `completed`, `failed_terminal`, `cancelled` | No ordinary orchestration. Retention execution is also frozen in `execute` mode until rollout permits it. | Converge any remaining reach to revocation before cleanup eligibility. | GitHub work and sandboxes are never selected by Trigger cleanup. |

An existing `owner_credential_unavailable`, `service_inactive`,
`session_rotating`, `session_recovering`, `recovery_required`, or
`revocation_pending` block is not overwritten by a rollout pause. Only a block
that was durably recorded as `feature_paused` is automatically resumed when the
same Launch becomes permitted again. Activity actions
`trigger_orchestration_pause` and `trigger_orchestration_resume` carry the
stable `rollout_reason` values used by support tooling.

## 6. Scheduler and structured telemetry

The process runs three independent interval loops:

| Lane | Candidate source | Worker |
|---|---|---|
| `effects` | Active Launch states | Normal fenced effect execute/reconcile worker |
| `capabilities` | Launches that may hold short-lived reach | Fenced renew/revoke worker |
| `cleanup` | Terminal Launches past `workspace.delete_after` | Plan-only worker in `dry_run`; normal effect worker in `execute` |

Each page is only an inventory. The worker must still acquire the Launch lease
and optimistic revision. Cursor repetition stops the pass, `max_pages` bounds a
healthy but large inventory, `buffer_unordered` bounds lane concurrency, and
`MissedTickBehavior::Skip` prevents a slow pass from creating a timer backlog.
A slow provider effect therefore cannot delay capability revocation.

Every pass emits the info-level message `Trigger scheduler pass settled` with:

```text
lane, pages, candidates, completed, deferred, paused, failed,
not_acquired, no_work, errors, page_budget_exhausted, actionable,
elapsed_milliseconds
```

Every worker settlement also emits `launch_id`, `correlation_id`, action/effect,
outcome, fence, and safe policy generation or rollout reason. The durable
activity record may additionally contain only these allowlisted identifiers:

```text
subject, launch ID target, correlation ID, revision digest,
numeric upstream/fork/account IDs, exact UserService ID,
Launch state, closed effect kind, provider request ID,
fencing token, policy generation, cleanup target count, rollout reason
```

Use Launch state/event timestamps for conversion and step-latency panels. Use
scheduler heartbeats for queue and process health, effect records for retry and
reconciliation rates, capability events for renewal/revocation, and cleanup
records for retention backlog. Do not add high-cardinality prompt, path, branch
name, URL, or response-body labels.

## 7. SLOs and alerts

Provider-caused waits and explicit user-action blocks are measured separately
from CMA availability. A typed blocked state is a settled result, not a hidden
success and not an infinite spinner.

| Signal | Objective | Alert |
|---|---|---|
| Scheduler heartbeat | Every lane reports within `max(3 * interval, 60s)` | Page if any lane is absent for 2 objective windows while the process is Ready. |
| Candidate pass | No `errors`, `failed`, or repeated cursor in 99.9% of passes | Warn on 3 consecutive affected passes; page on 12 affected passes or 5 minutes. |
| Page budget | `page_budget_exhausted=false` | Warn after 3 consecutive passes; page if it persists for 15 minutes because work is arriving faster than the configured bound. |
| Lease contention | A candidate normally acquires or is stale by the next pass | Warn when `not_acquired / candidates > 0.5` for 5 minutes with no completion; inspect the support lease before intervention. |
| Durable step settlement | 99% of admitted non-user-blocked steps settle, defer with a retry time, or enter a typed block within 10 minutes | Page any Launch with no event and an expired lease for 10 minutes. |
| At-most-once dispatch | Zero automatic repeat after `dispatch_fenced` without positive pre-write failure proof | Page on any recovery-required dispatch that is automatically claimed as execute. |
| Capability expiry/revocation | No accepted stale policy beyond 60 seconds plus 5 seconds skew; confirmed revocation may be earlier | Page on policy acceptance outside the bound, generation rollback, or numeric-resource mismatch. Warn when `revocation_pending` survives beyond the bound plus two scheduler intervals. |
| Cleanup eligibility | 99% of eligible plans appear within 15 minutes; execute completions within 24 hours | Warn on a 15-minute plan backlog; page on a 24-hour execute backlog or any widened/tampered target rejection. |
| Session continuity | 100% across Trigger flags and cleanup | Page immediately if a flag or Trigger cleanup stops, rotates, blanks, or deletes a running session. |
| Credential/content leakage | Zero | Page immediately on any credential, prompt, repository path/content, or provider body detected in logs, activity, support, or artifacts. Preserve the evidence under incident policy and revoke authority. |

Dashboard panels must include README preview-to-review, review-to-Start, and
Start-to-ready conversion; state duration percentiles; effect execute versus
reconcile; retry/defer/failure class; fork readiness; initial dispatch state;
checkpoint/restore; branch/PR result; capability generation and revocation;
provider status/rate limiting; lease contention; scheduler page budget; and
cleanup backlog. Repository/dashboard infrastructure is owned outside this
repository, so its saved view must use the stable fields above and carry a link
to this runbook. The deployment-specific dashboard and alert-rule URLs are
release evidence, not values that this repository can invent. Public exposure
remains blocked by ledger row `O-02` until those URLs and notification targets
are attached and exercised.

### Severity and alert routing

| Severity | Meaning | Acknowledge and route |
|---|---|---|
| `SEV-0` | Credential/content disclosure, cross-owner or unauthorized effect, session loss/interruption, destructive cleanup, automatic duplicate task, or accepted policy beyond the security bound | Page the platform on-call and runbook owner immediately; appoint an incident commander; engage security plus the affected identity/runtime owner within 15 minutes |
| `SEV-1` | Systematic inability to Start or recover, unknown external-write outcome without safe evidence, broken revocation lane, missing audit authority, or sustained SLO breach affecting multiple users | Page the platform on-call and runbook owner; assign identity, runtime, storage, or provider owner as indicated; update at least every 30 minutes |
| `SEV-2` | Bounded provider degradation, retry/backlog growth inside safety bounds, isolated stuck Launch, or expected legacy-runtime acknowledgement noise | Notify the owning team during the staffed window; assign a responder and review before widening rollout |
| `SEV-3` | Isolated usability/documentation defect with no authority, data, continuity, or duplicate-effect risk | File and triage through the ordinary product queue |

Provider and user-action blocks keep their typed product state even when an
incident is open. Severity is an operator response classification; it never
rewrites a Launch failure class or makes an unsafe retry permissible.

The following stable alert IDs are the source links that saved alert rules must
point back to. Each deployment maps them to its actual dashboard, rule, and
notification target in release evidence:

| Alert | Default severity | First response |
|---|---:|---|
| [`TRIGGER-SECURITY-LEAK`](#trigger-security-leak) | SEV-0 | Pause orchestration, engage security, preserve redacted evidence, and request revocation while tracking acknowledgement or expiry |
| [`TRIGGER-SESSION-CONTINUITY`](#trigger-session-continuity) | SEV-0 | Stop rollout changes, preserve the session ledger/runtime evidence, and apply forward-safe rollback |
| [`TRIGGER-DUPLICATE-DISPATCH`](#trigger-duplicate-dispatch) | SEV-0 | Fence the Launch, pause orchestration, and preserve all delivery generations; never send a compensating prompt |
| [`TRIGGER-UNAUTHORIZED-EFFECT`](#trigger-unauthorized-effect) | SEV-0 | Enable both pause and capability kill switch and preserve numeric identity/policy evidence |
| [`TRIGGER-CAPABILITY-BOUND`](#trigger-capability-bound) | SEV-1 | Keep revocation pending, stop renewal, inspect acknowledgement/expiry, and do not claim effective revocation early |
| [`TRIGGER-STUCK-LAUNCH`](#trigger-stuck-launch) | SEV-1 | Use the owner-safe support lookup, inspect lease/effect evidence, and schedule only read-only reconciliation |
| [`TRIGGER-SCHEDULER-HEALTH`](#trigger-scheduler-health) | SEV-1 | Compare all four lane heartbeats, page budget, cursor, and lease contention before changing concurrency |
| [`TRIGGER-PROVIDER-DEGRADED`](#trigger-provider-degraded) | SEV-2 | Honor retry times, preserve public/session reads, and pause new external effects if failures spread |
| [`TRIGGER-CLEANUP-DEBT`](#trigger-cleanup-debt) | SEV-2 | Return cleanup to dry-run on target/audit anomalies and inspect exact retained plans |

<a id="trigger-security-leak"></a>
**TRIGGER-SECURITY-LEAK.** Do not copy the detected value into chat, an issue,
or another log. Record only the safe correlation and detector rule, restrict the
artifact under incident policy, request revocation of affected authority, and
determine every surface that received the value. Keep the state
`revocation_pending` until the
runbook's acknowledgement or expiry evidence exists. A suspected false positive
stays SEV-0 until a security reviewer proves the marker was synthetic and no
real value was stored.

<a id="trigger-session-continuity"></a>
**TRIGGER-SESSION-CONTINUITY.** Freeze Trigger rollout changes and follow
section 11 without stopping the session. Record session ID, runtime generation,
checkpoint ID, last durable event, and observation gap. Runtime replacement is
performed only by the normal reconciliation authority.

<a id="trigger-duplicate-dispatch"></a>
**TRIGGER-DUPLICATE-DISPATCH.** Freeze the exact Launch and preserve every
delivery state, fence, payload hash, recovery generation, and owner action.
Never delete a message or send another to test the path. Determine whether an
automatic second owner turn was actually accepted before downgrading severity.

<a id="trigger-unauthorized-effect"></a>
**TRIGGER-UNAUTHORIZED-EFFECT.** Enable orchestration pause and the independent
capability kill switch. Preserve numeric upstream/fork/account IDs, exact
UserService ID, policy digest/generation, ref identity, and provider request ID.
Do not delete or rewrite the resulting GitHub resource as incident response.

<a id="trigger-capability-bound"></a>
**TRIGGER-CAPABILITY-BOUND.** Inspect the signed-policy generation and expiry,
Layer 4 tombstone acknowledgement, NyxID agent-key revocation result, and
runtime connectivity. Report `revocation_pending` until positive enforcement or
the 60-second lifetime plus 5-second skew bound; a revoke request is not proof.

<a id="trigger-stuck-launch"></a>
**TRIGGER-STUCK-LAUNCH.** Follow section 9 with one Launch or correlation ID.
Never break a live lease, edit effect state, or call a provider write by hand.
Only the exact-fence operator command may authorize another read-only evidence
pass; only the owner can authorize an ambiguous initial-task replacement.

<a id="trigger-scheduler-health"></a>
**TRIGGER-SCHEDULER-HEALTH.** Compare `effects`, `capabilities`, `workspaces`,
and `cleanup` heartbeats independently. Look for page-budget exhaustion,
repeated cursor, expired lease, checkpoint deferral, and provider latency. Keep
the capability lane available during an orchestration outage. A slow workspace
capture must affect only the `workspaces` lane. Do not raise
page/concurrency/retry bounds as a first response.

<a id="trigger-provider-degraded"></a>
**TRIGGER-PROVIDER-DEGRADED.** Follow section 10. Keep previews, Launch reads,
existing session conversation, diff export, and support available. Persist the
retry/reconciliation time and pause new effects at durable boundaries when the
failure is broad.

<a id="trigger-cleanup-debt"></a>
**TRIGGER-CLEANUP-DEBT.** Follow section 8. Unexpected vocabulary, tenant
mismatch, or missing audit is a stop condition: return to `dry_run`. Do not edit
the retained plan, enumerate sandboxes, or touch GitHub resources.

## 8. Cleanup dry-run and promotion

The `trigger-workspace-v1` policy assigns the workspace a concrete
`delete_after` 30 days after import. Cleanup becomes eligible only when the
Launch is terminal, a workspace exists, the time has passed, no completion was
recorded, and retained reach is absent or effectively revoked.

An import that staged objects but never committed a workspace uses the internal
`trigger-workspace-import-v1` policy and the Launch terminal timestamp as its
cleanup boundary. Its public cleanup plan still contains only the closed
`baseline` target. The executor resolves that target to the Launch-owned import
inventory, confirms deletion of every deterministic 4 MiB chunk, and needs no
GitHub access. A failed deletion remains ordinary durable cleanup debt and is
retried from the same retained plan.

In `dry_run`, the planner:

1. Acquires the normal persisted Launch lease.
2. Calls the aggregate's closed `plan_cleanup` transition.
3. Retains the exact ordered target set and `cleanup_planned` event.
4. Releases the lease and records `trigger_cleanup_plan`.
5. Has no object-store, GitHub, or sandbox deletion port.

Review the support snapshot for every canary plan. Its targets are represented
as a count to the operator surface; the durable aggregate may contain only
`baseline`, `checkpoint { checkpoint_id }`, and
`diff_artifact { artifact_id }`. Reject promotion if serialized plan vocabulary
contains `sandbox`, `fork`, `branch`, `commit`, `pull_request`, a free-form
object key, or a target from another owner/Launch.

Before changing to `execute`:

1. Keep repository and publisher allowlists at the disposable canary.
2. Confirm every plan's retention policy, `delete_after`, plan timestamp, owner,
   Launch, target count, and effective capability revocation.
3. Confirm object-store deletion treats an already absent exact object as
   success and a tenant/object mismatch as terminal security failure.
4. Run one partial-failure canary and prove reconciliation repeats the identical
   owner/Launch-bound keys.
5. Confirm GitHub and sandbox request counters remain zero for the cleanup.
6. Set `CHRONO_TRIGGER_CLEANUP_MODE=execute` and watch one target set complete.
7. Return to `dry_run` on any unexpected target, delete error class, or audit
   gap. Do not edit a retained plan to make it pass.

Cleanup execution uses the same write-before-effect claim, fencing token,
retry/reconciliation contract, and bounded scheduler as every other effect.
There is no second deletion implementation.

## 9. Support procedure

The caller must hold the narrow `read_activity_log` operator capability. Query
exactly one identifier:

```text
GET /api/v1/admin/triggers/support?launch_id=<uuid>
GET /api/v1/admin/triggers/support?correlation_id=<safe-correlation>
```

Supplying both or neither is `invalid_trigger_support_lookup`. A deployment
without Trigger persistence returns `trigger_support_unavailable`. The response
contains the owner and publisher subject IDs, numeric repository/account IDs,
exact service/session IDs, state/block, safe lease fence and timestamps,
cleanup summary, closed effect records, and at most 500 timeline events. It has
no prompt, input, path/content, credential, raw object reference, or provider
body.

Investigate in this order:

1. Confirm `launch_id`, immutable revision digest, owner, publisher, numeric
   upstream ID, fork ID, account ID, and exact service ID agree.
2. Read the last timeline event and current `blocked_reason`.
3. If a lease exists, compare `expires_at` with canonical UTC. Do not break a
   live lease. After expiry, let the normal worker acquire a higher fence.
4. Match the effect `intent_key`, state, attempt, reconciliation attempt, fence,
   provider request ID, failure code, and next attempt time.
5. For `recovery_required`, attach authoritative provider/runtime evidence and
   use only the explicit recovery operation. Never rewrite the effect state.
6. For `revocation_pending`, establish tombstone acknowledgement, confirmed
   agent-key revocation, or TTL expiry. Do not label it revoked early.
7. Correlate the activity action and structured worker line. Treat a missing
   activity entry as an observability incident even when the Launch ledger
   proves the state transition.

For a generic external effect, the only explicit operator action is:

```text
POST /api/v1/admin/triggers/launches/{launch_id}/reconcile
```

It requires `control_any_session`, one durable `Idempotency-Key`, and exact
`expected_revision`, effect kind, retained intent key, effect fence, and failure
correlation from the support snapshot. Acceptance changes only that effect from
`recovery_required` to `reconciling`; the next pass is read-only. Wrong or stale
evidence is refused atomically. For an ambiguous initial prompt it may inspect
the retained turn evidence, but it cannot authorize a replacement dispatch.

An initial-prompt replacement is authorized only by the Launch owner through
`POST /api/v1/launches/{launch_id}/recover-initial-prompt`, bound to the retained
payload hash and an explicit acknowledgement that the task may execute twice.
An operator must not make that acknowledgement on the owner's behalf. The full
state, resource-count, cancellation, and recovery procedure is
[CMA_Trigger_Recovery.md](./CMA_Trigger_Recovery.md).

## 10. Provider outage and circuit breaking

- Keep public reads, existing session conversation, diff export, support, and
  durable inspection available. Provider health does not fail global CMA
  readiness.
- Persist retry or reconciliation time before releasing a worker lease. Honor a
  bounded provider `Retry-After`; never spin inside one lease.
- A known pre-write transient failure may retry under the effect budget. A
  timeout after possible bytes were written enters reconciliation or
  `recovery_required` according to the effect contract.
- When failures affect multiple Launches, set
  `CHRONO_TRIGGER_ORCHESTRATION_PAUSED=true`. Keep the capability lane operating
  unless its own enforcement endpoint is unavailable.
- If revocation enforcement is unavailable, leave `revocation_pending`, keep the
  kill switch on, and use policy expiry as the bounded disconnected backstop.
- Do not remove the pod from service merely because GitHub or NyxID is down;
  that would hide session and support reads that still work.

The bounded page, concurrency, retry-time, and per-effect reconciliation budgets
are the in-process circuit breakers. Increasing them during an outage usually
amplifies the outage and requires an explicit reviewed configuration change.

## 11. Rollback

Rollback is a forward-safe control sequence, not deletion and not a downgrade
of durable state.

1. Set `CHRONO_TRIGGER_ORCHESTRATION_PAUSED=true`. Confirm each affected Launch
   either finishes its current external boundary or records a durable pause.
2. Set `CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH=true`. Track
   `revocation_pending` until acknowledgement or the TTL bound. Do not stop the
   session to accelerate it.
3. Set `CHRONO_TRIGGER_CLEANUP_MODE=dry_run` to prevent new object deletions.
   Let an already committed delete settle or reconcile; do not manufacture a
   success receipt.
4. Set `CHRONO_TRIGGER_USER_COHORT_PERCENT=0`, clear both allowlists, then set
   `CHRONO_TRIGGER_ENABLED=false`.
5. Verify README re-clicks, Launch/event reads, support, session conversation,
   and safe export still work.
6. Keep the current database schema and a worker-capable image available until
   every claimed effect and capability debt is settled. Do not deploy an older
   writer that cannot read the new aggregate.
7. Re-enable through section 4 after the defect is fixed and the affected
   reconciliation paths are drilled.

No rollback step deletes or resets a fork, branch, commit, PR, workspace record,
or session. Removing a README link prevents discovery but is not a state or
credential rollback.

## 12. Schema, migration, and backfill

Startup creates bounded indexes for owner listings, immutable public revisions,
current Launch uniqueness, lease lookup, current-only worker inventory,
current-only non-revoked capability inventory, correlation lookup, and retention
lookup. Deploy with all rollout gates closed and verify index creation before
admitting the canary.

Launch progress history lives in `trigger_launch_events`, uniquely indexed by
Launch ID and sequence and indexed separately by safe correlation ID. Each
Launch document retains only the latest 512 envelopes plus its event
high-water. Event append and aggregate replacement commit in one transaction;
never repair one side independently. A legacy Launch without the top-level
high-water remains readable from its embedded history and is archived and
compacted on its next successful mutation.

The added blocked reason, effect, capability, cleanup, lease-support, and event
fields are additive. Missing optional fields retain their documented defaults.
There is no bulk backfill that infers provider effects, capability revocation,
cleanup completion, or user confirmation from older data. Such inference would
invent authority. A pre-feature database has no Trigger rows and needs only the
indexes. Experimental Trigger rows that fail validation are quarantined from
public exposure and handled as migration evidence, not rewritten in place.

Before and after deployment, record:

```text
collection document counts; index names and readiness;
Launch counts by state and blocked reason;
live versus expired lease counts; effect states and recovery debt;
active/revocation_pending/revoked reach counts; cleanup eligible/planned/completed counts
```

Any count change not explained by a user action, worker event, or retention
receipt blocks rollout widening.

## 13. Retention, export, and deletion

- Request activity is retained by `CHRONO_ACTIVITY_RETENTION_DAYS` and the
  MongoDB TTL policy. The allowlist controls content independently of duration.
- Immutable revisions and Launch metadata are audit/reconciliation authority
  and are not silently expired. Retirement prevents a revision from creating a
  new Launch; it does not rewrite an existing Launch.
- Initial-prompt outbox/effect state is embedded in the Launch ledger. It is
  retained as the proof that prevents duplicate automatic dispatch and is not
  separately deleted as an orphan row.
- Workspace baseline/checkpoint/diff bytes follow the snapshotted
  `trigger-workspace-v1` plan and the fenced cleanup procedure above.
- GitHub forks, branches, commits, and PRs are user/provider-owned and remain
  outside CMA deletion, including privacy deletion and rollback.
- Sandbox teardown remains exclusively in the existing session reconciliation
  lifecycle and is never enumerated by Trigger retention.

A subject export uses the authenticated Trigger draft/revision/Launch/event and
diff endpoints for that subject. An operator may add the redacted activity and
support projections under the normal privacy authorization process. Never use
raw object-store keys or credentials as export material.

For an approved erasure request, first retire publisher revisions or cancel
contributor Launches under the subject's authority, request capability
revocation, wait for positive enforcement or expiry, and let eligible CMA-owned
workspace objects complete fenced cleanup. Preserve the minimum immutable
security/audit tombstone required by policy. This MVP exposes no public command
that deletes another subject's metadata and claims no legal-hold capability;
any database-level metadata erasure requires the platform's separately approved
data-controller procedure and must not break effect deduplication or credential
revocation evidence.

## 14. Required drills and evidence

Before public exposure, attach redacted evidence for:

- disabled, paused, publisher-denied, repository-denied, and cohort-denied
  admission, plus a README re-click of an existing Launch in each mode;
- a pause at every effect boundary, resume from `feature_paused`, and proof that
  every pre-existing non-rollout block remains unchanged;
- capability tombstone acknowledgement, agent-key revocation acknowledgement,
  provider failure, disconnected runtime, expiry, and generation rollback;
- worker crash before claim, after claim/before call, after possible write, and
  after provider success/before completion commit;
- page-budget exhaustion, cursor repetition, lease contention, provider rate
  limiting, and scheduler recovery on the next pass;
- cleanup dry-run vocabulary, tenant mismatch, partial deletion, idempotent
  repeat, and zero GitHub/sandbox calls;
- control-plane replacement while a Trigger session is running, proving the
  same session ID, history, workspace, and conversation continue; and
- support lookup and activity redaction with planted prompt, input, path,
  credential, and provider-body markers that never appear.

Evidence must name the build revision, environment, UTC interval, configuration
revision, numeric fixture repository, expected result, actual result, and
redaction applied. A production restart is never performed solely to collect a
drill transcript.

## 15. Recovery drill procedure

The minimum release drill proves that a possible successful GitHub write can be
recovered after worker/control-plane loss without duplicating a resource,
interrupting the session, or granting an operator write authority. Run it in an
authorized staging/canary environment against the disposable public fixture.
Never restart or deploy production solely for this drill.

### Preconditions

1. Record the evidence fields required by
   [CMA_Trigger_Release_Readiness.md](./CMA_Trigger_Release_Readiness.md#2-evidence-record),
   including final build/runtime digests and configuration revision.
2. Limit publisher, repository, and contributor admission to the fixture.
   Leave cleanup in `dry_run`; verify the capability lane and all four
   scheduler heartbeats (`effects`, `capabilities`, `workspaces`, and `cleanup`)
   before injecting a fault.
3. Record numeric upstream, account, existing fork-network, branch, PR, Launch,
   session, and provider-key metadata as redacted IDs and counts. Confirm the
   fixture contains no secret and no privileged workflow.
4. Verify the operator has only the narrow support and reconciliation
   capabilities. The Launch owner is a separate disposable contributor and is
   available for owner-only recovery decisions.
5. Route every alert ID in section 7 to its expected test notification target
   and mark the drill interval so synthetic alerts cannot be mistaken for an
   unannounced incident.

### Stuck external-write scenario

1. Start one Launch and inject a fault that lets the GitHub fork write succeed
   but loses the response before CMA can commit completion. Stop the responsible
   worker after the durable effect claim; do not stop the session or provider.
2. Prove the retained effect contains one intent key and fence and that a later
   worker performs only numeric-identity reconciliation. The external fork
   count must be one.
3. Withhold or fail the bounded read-only evidence lookup until the effect
   reaches `recovery_required`. Confirm no automatic execution claim occurs and
   `TRIGGER-STUCK-LAUNCH` reaches the test target.
4. Query the support endpoint by the one Launch ID. Match owner, immutable
   revision, numeric upstream/account identities, exact service ID, effect kind,
   intent key, fence, correlation, attempts, and expired lease. Confirm the
   projection contains none of the planted prompt/path/credential/provider-body
   markers.
5. Submit one operator reconciliation command with the exact expected revision,
   effect kind, intent key, effect fence, and failure correlation. Retry an
   ambiguous response only with the same idempotency key and byte-identical
   body.
6. Restore the read-only provider lookup. Prove it finds the one existing fork,
   attaches its numeric identity, and advances through branch, workspace,
   sandbox, session, and prompt without issuing another fork write.
7. Complete a reviewed diff, branch publication, and explicit pull request.
   Record exactly one Launch, compatible fork, Launch branch, session, automatic
   initial owner message, contribution commit path, and pull request.

### Continuity and rollback scenario

1. While the resulting session is running a turn, replace the staging/canary
   control plane through its normal deployment mechanism. Do not ask the owner
   to revive or recreate anything.
2. Confirm the same session ID, history, configuration, installed skills,
   workspace/checkpoint, GitHub capability policy, and in-flight conversation
   resume. A new runtime is allowed only through the normal recorded
   reconciliation/rotation path.
3. Apply section 11's rollback controls one at a time. Confirm new effects pause
   at durable boundaries, revocation remains pending until acknowledged or
   bounded expiry, cleanup returns to dry-run, and the existing session remains
   readable and conversational.
4. Re-enable through section 4. Confirm only a recorded `feature_paused` block
   resumes automatically and the Launch/resource counts remain unchanged.

### Authority and cleanup companions

The same campaign also exercises an inactive/disconnected NyxID service,
expiry with and without refresh authority, exact-runtime credential replacement
acknowledgement, tombstone or agent-key revocation, generation rollback, cleanup
partial failure, and tenant mismatch. These are distinct evidence records even
when run in one window. A mock acknowledgement or policy TTL calculation cannot
replace the required staging/provider observation.

### Pass, abort, and evidence

Pass only when every expected alert fired and recovered, no secret/content
marker escaped, the session remained continuous, every external resource count
matched, the ambiguous write was reconciled read-only, and the rollback/re-enable
sequence preserved all user work.

Abort and keep rollout closed on any second provider write, automatic replacement
prompt, live lease break, cross-owner data, credential/content marker, missing
audit event, unexpected cleanup target, policy acceptance beyond its bound, or
session/history/workspace interruption. Preserve the evidence; never delete or
reset the provider resource to make the next run look clean.

The evidence package includes the redacted command/request shapes, support and
activity projections, state/event timeline, resource-count diff, saved
dashboard interval, fired-alert links, rollout revisions, expected versus actual
table, redaction review, and independent reviewer sign-off. Until that package
is linked to ledger row `O-01`, the drill status is **not run**.

## 16. Incident escalation and handoff

The first responder records only the safe Launch/correlation IDs, build and
configuration revision, UTC interval, alert ID, severity, typed state, and
whether rollout/capability/cleanup controls changed. The incident commander
assigns separate owners for containment, provider/identity evidence,
runtime/session continuity, storage/cleanup, user communication, and review as
needed. One person may hold multiple roles, but the record keeps the decisions
separate.

Escalate to security immediately for any possible credential/content exposure,
cross-owner read/effect, unauthorized repository/ref/operation, policy-bound
violation, or evidence tampering. Escalate to the runtime/session owner for any
history, workspace, conversation, identity, or capability discontinuity.
Escalate to the NyxID/identity owner for service authority, refresh,
replacement, revocation, or agent-key ambiguity. Escalate to the storage owner
for snapshot, artifact, retention, or cleanup-target ambiguity. GitHub support
is contacted only after CMA/NyxID evidence establishes a provider-side issue and
the incident commander approves the redacted case.

At handoff, state what is known, unknown, fenced, and still externally possible.
Include the exact next safe read-only action and its authority. Never hand off
with "retry" as an instruction when an external write may have occurred. The
incident closes only after durable state and provider truth agree, alerts and
reconciliation debt clear, user impact is accounted for, evidence is redacted,
and the runbook owner records the follow-up and rollout decision.
