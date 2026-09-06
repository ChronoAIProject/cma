# CMA Operations

This runbook covers deployment-owned operations for the Sandbox/Agent model.
The current runtime contract is **3**, not the contract-2 image named in the
original #832 plan. Product semantics are defined in [CMA Model](CMA_Model.md);
public routes are in [REST](API_REST.md).

## Delivery Authority

For this milestone, the maintainer's explicit instructions override the release
step in #832: finish all development and local gates first, then deliver reviewed
changes to `develop`, build images and deploy directly to production. **Never
merge or push `main`, invoke the release workflow or change NyxID.** A direct
deployment is not a GitHub release and creates no release tag by implication.

The one-time reset of historical CMA data and CMA-owned provider Sandboxes is
authorized for this refactor. It is not permission to delete another service's
database, bucket or runtime. Ordinary deployments after reset must preserve all
owner work under `CLAUDE.md` section 5. An evidence failure requires a local fix
and direct redeployment, followed by the affected production journeys again.

Current local checks are not production approval. The Release Evidence section
below records incomplete final gates explicitly. Do not advertise public
readiness or call the milestone complete while any required gate is unproved.
Controlled test enablement for the final journeys is distinct from public
publication and must be withdrawn if verification fails.

## Prerequisites

- Verify the exact cluster context, namespace, deployment id, database and bucket
  before any write. The recorded production namespace is `chrono-bot`, deployment
  id `chrono-bot-prod` and database `chrono-bot`; re-read live configuration rather
  than relying solely on this historical record.
- MongoDB must support sessions, transactions, snapshot reads and majority
  acknowledgment. The application identity needs its documented collection,
  index and validator privileges. A standalone local Mongo process is not a
  substitute for transaction/recovery tests.
- The sandbox provider must accept the pinned image, nonroot home and requested
  resources. The service-account/SecretProviderClass relationship and mounted
  provider key must resolve CMA's own authority, never a similarly named class
  in another namespace.
- Supply referenced secrets out of band. No secret value belongs in a manifest,
  commit, command argument, screenshot or evidence transcript. Keep the existing
  authentication encryption key stable unless an explicit migration is designed.
- External checkpoint bytes use the existing chrono-storage bucket
  `chrono-platform-bot`, not a renamed GridFS bucket. Verify the machine identity
  can upload, read and delete exact objects through the platform proxy and that
  DELETE is independently confirmed by GET 404. Metadata remains in MongoDB.
- The provider-facing model endpoint and baseline services must be usable for a
  fresh visitor. A successful admin account test does not establish that fact.
- Build, label, scan and inspect all three immutable image digests. Check the
  runtime contract label and pinned Codex/bridge versions against the final
  image. Historical Agent-home evidence is a revalidation input, not a pass.

The historical cluster context in [Sandbox Provider](Sandbox_Provider.md) is
`gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke`.
Confirm it is still the target before any context-pinned command. Do not change
account/project context globally while other work may be active.

## NyxID Prerequisites

CMA uses two independent origins. `CHRONO_NYXID_BASE_URL` is the API/OAuth
origin; `CHRONO_NYXID_PUBLIC_URL` is the browser registration/connect origin and
must be one canonical HTTPS origin. Production configuration names
`https://nyx-api.chrono-ai.fun` and `https://nyx.chrono-ai.fun` respectively.
`CHRONO_SERVER_PUBLIC_URL` remains the CMA browser origin so its host-only cookie
and callback return to the same application.

Read `GET /auth/signup-options`. It returns only `email_signup`,
`social_providers`, `invite_code_required` and `register_available`.
`register_available: false` means no usable registration path is advertised:
the provider may have disabled all methods, or its configuration could not be
obtained safely. Do not advertise account creation from a guessed provider URL.
`INVITE_CODE_REQUIRED` is **NyxID's** operator setting, not a CMA environment key.
If it is enabled, public signup is not established without an authorized invite
path. Under this delivery's authority, report the prerequisite and stop public
enablement rather than modifying NyxID.

The required pre-existing default visitor role has these exact values:

```json
{
  "name": "CMA visitor",
  "slug": "cma-visitor",
  "description": "Can start CMA agents from trigger buttons and use their own sandboxes",
  "permissions": [
    "cma:trigger:launch",
    "cma:sandbox:use",
    "cma:sandbox:delete"
  ],
  "is_default": true
}
```

The role-administration contract is NyxID's `POST /api/v1/admin/roles`, not a
route CMA calls. The JSON above is a verification reference, **not authorization
to create or change the role in this delivery**. Do not add sandbox-create,
profile-management, Trigger-publishing or administrator permissions. A fresh
user must receive the intended defaults from the existing provider configuration.

CMA derives a visitor from `LaunchTriggers` without `CreateSandbox`; it does not
trust a role slug. Hosted OAuth/API-key/device-code connection pages are also
NyxID-owned. CMA creates short-lived links as the visitor and polls their result,
without registering another callback or handling a service secret in its UI.

## Configuration Inventory

Every quoted `CHRONO_*` key read by `cma/src/config.rs` and its modules is listed
below. `.github/scripts/check-config-keys.sh` checks manifest/reference parity;
`.github/scripts/check-config-docs.mjs` checks this documentation. Secret values
are referenced in `infra/api/secret.yaml`, never stored in ConfigMaps.

| Key | Contract / Default |
|---|---|
| `CHRONO_SERVER_BIND` | Listen address, default `0.0.0.0:8080` |
| `CHRONO_SERVER_PUBLIC_URL` | CMA browser origin; default local `http://localhost:8080`, production explicit |
| `CHRONO_SERVER_CORS_ORIGIN` | Comma-separated exact allowed origins; default local `http://localhost:5173` |
| `CHRONO_AUTH_MODE` | `provider` in production; `trusted` is local fixed-subject development only |
| `CHRONO_TRUSTED_SUBJECT` | Local trusted-mode subject; default `local-developer`, irrelevant in provider mode |
| `CHRONO_NYXID_BASE_URL` | Required API/OAuth HTTP(S) origin in provider mode |
| `CHRONO_NYXID_PUBLIC_URL` | Required exact canonical HTTPS browser origin in provider mode |
| `CHRONO_NYXID_CLIENT_ID` | Provider-issued OAuth client id, not a chosen display name |
| `CHRONO_NYXID_CLIENT_SECRET` | Required OAuth client secret reference |
| `CHRONO_AUTH_ENCRYPTION_KEY` | Secret reference: standard-base64 encoding of exactly 32 random bytes |
| `CHRONO_MONGODB_URI` | Required secret reference, `mongodb://` or `mongodb+srv://` |
| `CHRONO_MONGODB_DATABASE` | Database name; code default `chrono_bot`, production explicitly `chrono-bot` |
| `CHRONO_DEPLOYMENT_ID` | Required stable Kubernetes-label-compatible CMA ownership identity |
| `CHRONO_SANDBOX_BASE_URL` | Required canonical HTTP(S) provider origin |
| `CHRONO_SANDBOX_API_KEY_FILE` | Required readable nonempty mounted provider key; never a ConfigMap value |
| `CHRONO_SANDBOX_NAMESPACE` | Required DNS-label namespace for CMA-owned provider resources |
| `CHRONO_SANDBOX_IMAGE` | Required immutable OCI reference ending in `@sha256:<64 lowercase hex digits>` |
| `CHRONO_UPSTREAM_BASE_URL` | Required canonical HTTPS model API base without trailing slash |
| `CHRONO_UPSTREAM_WIRE_API` | Exactly `responses` |
| `CHRONO_PLATFORM_BASE_URL` | Required canonical HTTPS platform proxy origin |
| `CHRONO_BASELINE_SERVICES` | Default `chrono-llm,ornn-api,ornn`; unioned into newly admitted Sandbox scope |
| `CHRONO_SANDBOX_CPU` | Seed default Sandbox Profile size; default `1`, accepts positive cores or millicores |
| `CHRONO_SANDBOX_MEMORY` | Seed default size; default `1Gi`, accepts positive whole Mi/Gi |
| `CHRONO_DEFAULT_AGENT_MODEL` | Seed default Agent model; default `gpt-5.6-sol`, nonblank |
| `CHRONO_DEFAULT_AGENT_EFFORT` | Seed default effort; default `high`, one of `minimal,low,medium,high,x_high` |
| `CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS` | Per-command bound 1-600; default 600 |
| `CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS` | Total ordered-command bound, at least command bound and at most 1800; default 1800 |
| `CHRONO_CLONE_DEPTH` | Git clone depth 1-1000; default 50 |
| `CHRONO_CHECKPOINT_BUCKET` | Required external chrono-storage bucket; existing production bucket `chrono-platform-bot` |
| `CHRONO_STORAGE_CLIENT_ID` | Required machine identity for checkpoint storage, distinct from browser OAuth client |
| `CHRONO_STORAGE_CLIENT_SECRET` | Required storage machine-secret reference |
| `CHRONO_ACTIVITY_RETENTION_DAYS` | Activity TTL, 1-3650 days; default 90 |
| `CHRONO_SERVER_RATE_ADMISSION_PER_MINUTE` | Pre-auth network budget/burst; default 6000 |
| `CHRONO_SERVER_RATE_HEALTH_PER_MINUTE` | Separate probe budget, minimum 30; default 120 |
| `CHRONO_SERVER_RATE_DEFAULT_PER_MINUTE` | Ordinary request budget/burst; default 600 |
| `CHRONO_SERVER_RATE_CREATE_PER_MINUTE` | Creation request budget/burst; default 10 |
| `CHRONO_SERVER_RATE_INPUT_PER_MINUTE` | Agent input request budget/burst; default 120 |
| `CHRONO_SERVER_TRUSTED_PROXY_CIDRS` | Exact trusted load-balancer peers; empty by default, production explicit |
| `CHRONO_SERVER_TRUSTED_PROXY_HOPS` | Trusted rightmost forwarded hops, 1-16; default 1 |
| `CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS` | Empty or the complete build-pinned set; must not overlap trusted peer ranges |

`RUST_LOG` controls tracing and defaults to `info`; it is not a `CHRONO_*`
configuration field. Log level changes never authorize credential-value output.
Keep model/service/provider configuration out of ad hoc frontend build flags.

Execution policy is pinned before each effect and participates in its operation
fingerprint. A changed timeout or clone-depth environment value must not change
an already admitted replay plan. Existing Sandboxes likewise keep their pinned
profile and service scope. Startup defaults seed missing default profiles; they
are not a live rewrite mechanism for published profiles or existing settings.

## Platform Settings

Settings are durable and revision-checked through `/api/v1/admin/settings`.
The Operations UI calls the same public API. Read first, retain its revision and
submit `expected_revision`; reconcile conflicts without automatic overwrite.

| Section | Shipped defaults |
|---|---|
| `defaults` | `sbp_default`, `wkp_default`, `agp_default`, each revision 1 |
| `inactivity.pause_after_seconds` | 900; valid range 60-86400 |
| `retention.checkpoint_grace_days` | 7 |
| `retention.agent_log_days` | 90 after terminal state, subject to integrity rules below |
| `limits.max_sandboxes_per_user` | 3 |
| `limits.visitor_max_sandboxes` | 1 |
| `limits.visitor_max_launches_per_day` | 5 |
| `triggers.enabled` | false |
| `triggers.orchestration_paused` | false |
| `marketplace.enabled` | true |

Enabling Triggers permits new public use. Orchestration pause freezes admitted
progress rather than manufacturing a failed Launch. Inventory/retirement,
settings, activity and runtime reads each require their operator capability.
Do not make an administrator's ability depend on an unverified frontend role.

Visitor daily charging happens only when idempotency proves a new Launch was
created. Exact replay and later reviewed Start do not consume another unit.
Exhaustion is `resource_quota` with the next UTC reset time. Sandbox counts are
transactional, so concurrent Launches cannot bypass the visitor's one-Sandbox
limit. The request and visitor daily counters are process-local: production
must remain **one API replica**, with Recreate rollout, until a durable atomic
ledger replaces them. Restart resets of process-local counters are a known
operational limitation, not proof of a distributed quota.

## One-Time Reset And Direct Deployment

Rehearse against a disposable staging database/provider scope first and retain
the dry-run target inventory. Do not run a migration-era binary concurrently
with the new model. Never reuse a generic shell variable such as `HOME` to name
a target, and never delete by an unvalidated glob or an entire shared namespace.

1. Finish implementation, independent review, local gates and all three image
   builds/scans. Submit the separately reviewed CLAUDE contract change and the
   completed implementation to `develop`. Do not touch `main` or release workflows.
2. Record the exact old and replacement image digests, manifest revisions,
   Kubernetes context, database, deployment identity and provider resource ids.
   Keep Trigger enablement off. Retain a recoverable database backup or record
   the expressly authorized irreversible loss of historical CMA state.
3. Scale only the context-pinned `chrono-bot/cma-api` Deployment to zero and
   verify its pods and in-flight workers are gone. Wait for admitted provider
   effects to settle. The old control plane must not recreate a just-deleted pod.
4. List the provider's complete paginated inventory, resolve only CMA's matching
   deployment/namespace labels, and delete each resolved Sandbox via the provider
   API. Verify each absence; do not use namespace-wide Kubernetes deletion.
5. Resolve and confirm the exact CMA database name from live configuration.
   Only then execute its authorized `dropDatabase()` with the operator connection.
   Never drop the URI's default database, an admin database, NyxID's database or
   a database whose identity was inferred from a shell default.
6. Clean only exact historical CMA object keys proven orphaned by the retained
   inventory. Do not delete the shared external bucket or apply broad lifecycle
   expiry to active checkpoint metadata/tombstones. Failed/ambiguous cleanup is
   retained for reconciliation, not reported as completed.
7. Apply the reviewed ConfigMaps and secret references. Install the new API/web
   immutable digests and pin `CHRONO_SANDBOX_IMAGE` to the final contract-3 runtime.
   Verify the manifests' health endpoints, resources, nonroot security, runtime
   label and stream timeouts against the built images.
8. Start exactly one new API instance. Wait for collection/index initialization,
   default profile seeding and `/ready`; verify `/livez`, `/openapi.json`,
   `/api/v1/defaults`, operator inventories and the three default revisions.
9. Verify the **existing** NyxID visitor/registration/baseline prerequisites
   without changing them. After prerequisite and image smoke checks pass, enable
   only the controlled test campaign and exercise all final journeys with
   disposable, unadvertised test Triggers. Pause/disable it on systemic failure.
   If a missing provider prerequisite requires new authority, leave public
   enablement closed and report that exact blocker.
10. Open public use only after the required verification has passed. Publish the
    requested public research Trigger using the three actual public profile
    revisions; retain its descriptor and real click-through evidence. Record the
    final `develop` commit and deployed API/web/runtime digests together.

Health becoming Ready is not proof of checkpoint restoration, baseline visitor
access or correct embed behavior. The API pod's termination grace must exceed
its bounded HTTP and worker drains; review it whenever those timeouts change.
No ordinary rollout may delete owner Sandboxes to conceal a recovery defect.

## Checkpoints And Cleanup

The external storage service is reached through the platform machine-credential
flow and `/api/v1/proxy/s/chrono-storage-service`. The existing bucket is retained
under its new checkpoint configuration name. MongoDB owns checkpoint manifests,
writer fences, publication authority and deletion intents; external objects own
the bytes. GridFS is not an acceptable substitute for the required external
persistence boundary.

Publish only after bounded upload, size/digest verification and readable bytes.
Use immutable physical object identities so cleanup of a losing upload cannot
delete a concurrent winner. Retention preserves the two recorded checkpoints
and does not remove an active recovery reference. Record deletion intent in the
same transaction that removes publication authority, then perform bounded HTTP
cleanup after commit. Never keep a Mongo transaction open around external HTTP.

A DELETE followed by GET 404 is necessary but insufficient while an earlier
ambiguous upload can still finish. The client upload deadline is not a remote
write-lifetime guarantee. Retain minimal `checkpoint_archive_tombstones`
indefinitely for uncertain stages, containing only the exact opaque physical
key, schema version and bounded retry state. Confirmed absence permits Sandbox
purge to finish, but never relinquishes cleanup ownership of that key. Recheck
with exponential backoff from one minute to one hour, in bounded pages, so a
write arriving after any client deadline is still reclaimable. Restart must
resume this cleanup; timeout or lost response must not silently erase the only
record of an orphan. No tombstone contains credential fields. Final external
storage and delayed-upload cleanup evidence remains a required deployment gate.

## Rollback And Alerts

Pause orchestration and new public enablement before investigating a systemic
admission/recovery defect. Preserve existing resources and histories. Prefer a
forward fix built from the completed development branch. Reverting the web image
alone is safe only if its public contract still matches the API. Do not roll an
old Session-era binary onto the new database, or an incompatible runtime onto
new manifests. Restoring a pre-reset backup is a separate destructive epoch
transition, never an ordinary rollback that preserves post-reset work.

Alert on sustained readiness/store failures, reconciliation timeouts, unresolved
credential revocations, checkpoint publication/cleanup backlog, rejected
integrity/fence checks, first-message recovery-required state, repeated runtime
absence, capacity failures, abnormal HTTP 429/503 rates and inactive baseline
services. Record object/operation/request ids and failure classes, not secrets.
Do not convert observation timeouts into permanent resource failure.

Inspect operator inventories and durable event heads first. Do not repair a log
by deleting one event, relabeling a checkpoint, editing a lease or changing an
execution fingerprint. A worker may resume only from its recorded authority.

## Restart Drills

Run on disposable owned resources, then repeat the required production proof.
Keep a dirty Worktree, a home-local installed program and two distinct Agents
with identifiable durable conversations. Capture ids, checksums, image digests,
provider generations and event heads before each fault.

| Fault / Boundary | Required result |
|---|---|
| Restart during `pausing` | Resume the recorded checkpoint/fence operation; no lost home data |
| Restart during `reviving` | Restore the same hierarchy and exact Agent conversations |
| Runtime image drift | Rotate both Agents, dirty Worktree and installed software under contract 3 |
| Install failure/timeout | Typed failed step; no credential or proxy activated early |
| Clone failure / Worktree conflict | Recorded failure without duplicate Workspace/Worktree |
| Agent start timeout | Named unavailable/failed state, no guessed readiness |
| First-message ambiguity | Recovery-required card and no blind duplicate dispatch |
| Worker crash between steps | Reclaim its durable operation under a new fence |
| Browser closed mid-Launch | Reopen the owned Launch and resume retained progress |
| Popup blocked / same-tab return | Supported handoff path, no credential stored and no implicit Start |
| NyxID unavailable for requirements | Unavailable state, not satisfied services or new work |
| Ambiguous/late checkpoint upload | Retained cleanup intent, no leaked winner deletion, verified eventual absence |

## Agent Log Capacity


CMA validates an agent's complete durable log before accepting a mutation. To
keep that validation within a bounded MongoDB snapshot, normal log writes stop
at either 10,000 entries or 64 MiB of exact encoded BSON, whichever is reached
first. The permanent store error is `agent_log_capacity_exceeded`. Existing log
pages remain readable, and the owner can still stop and delete the agent; start
a new agent for further conversation.

Lifecycle publication has two internal reserves that normal conversation and
title writes cannot consume. A stop or startup-failure transition may use at
most another 64 entries or 512 KiB, and deletion may use a final 64 entries or
512 KiB. These staged ceilings ensure a full stop reserve cannot strand the
required deletion record. Each atomic append is separately limited to 64 log
entries so one transaction cannot materialise an unbounded command batch.

`retention.agent_log_days` is the maximum time logs are retained after their
agent becomes terminal; it does not increase these per-agent capacity limits.
Never delete individual `agent_log` rows to reclaim capacity. A missing row or
an entry above the aggregate high-water mark is treated as durable corruption.

After an ambiguous Mongo commit, CMA reconciles the complete authority snapshot
at the commit session's operation time. The request fails closed as unavailable
if reconciliation has not completed within 30 seconds. Because cancelling an
in-flight Mongo driver future is unsafe, that read-only check finishes in the
background; at most 16 such checks may remain pending per API process.

## Release Evidence

Status as of 2026-09-07: local implementation and deterministic/browser checks
are progressing. **No final production reset, deployment or public-readiness
claim is recorded here.** Historical experiments must remain labelled with their
original image and environment. Update this register with actual observations,
not a prediction that a passing unit test will reproduce in production.

Every evidence entry must include: gate/journey id; timestamp and environment;
CMA commit; API/web/runtime immutable digests; NyxID/provider version or commit;
redacted identity class; prerequisites; exact actions; observed result; report
and screenshot locations; request/resource ids; cleanup; remaining limitations.
Keep raw credentials and real handoff URLs out of reports.

| Gate | Final required evidence | Current final status |
|---|---|---|
| G-A1-G-A5 | Distinct Agent homes, prompt order, actual skill hot reload, shared auth/model access, exact conversation capture/restore | Pending on final contract-3 digest; historical section 8 evidence is not final |
| G-S1 | Install with proxy socket/credential absent; installed home software survives pause/revive | Pending final image/production proof |
| G-S2 | Fresh default visitor with baseline-only profile reaches ready Agent | Pending existing NyxID prerequisite and production proof |
| G-P1 | Restart during pausing/reviving and two-Agent dirty-home rotation | Pending final image/production proof |
| G-T1 | README/camo, HTML, strict-CSP script and React click-through with attribution | Local publisher fixtures/browser checks; real hosts pending |
| G-T2 | OAuth, API-key and device-code connection assist completion | Deterministic adapter/UI checks; real provider journeys pending |
| G-T3 | 50 visitor clicks admit at most 5 new Launches with typed reset | Local quota checks; production burst pending |
| Storage | External streamed bytes, integrity, ambiguity, retention and delayed-upload cleanup | Adapter integration and final provider proof pending |

| Journey | Required observation | Final status |
|---|---|---|
| J0 | Account creation from landing returns to same Trigger and ready Agent, one explicit pre-sign-in click, zero post-sign-in clicks | Pending |
| J1 | Always-new Sandbox path | Pending |
| J2 | Reused Sandbox with new Workspace | Pending |
| J3 | Reuse both; Agent ask/continue/new and resume-existing branches | Pending |
| J4 | Missing service, connection assistant, then explicit continuation | Pending |
| J5 | Publish profile, second-owner fork, Trigger wizard, embed and stats | Pending |
| J6 | Public-profile trust, new Sandbox/Workspace, two Agents, stop/delete, pause/revive | Pending |
| J7 | Retired Trigger and paused orchestration freezes rather than fails | Pending |

Security evidence must additionally prove: no credential in browser storage,
DOM or captured bodies; badge sets no cookie; cross-site form POST refused;
visitor cannot directly create a Sandbox, manage Profiles or publish Triggers;
foreign resource/handoff reads concealed; no leak through errors or telemetry.
Capture landing, trust review, connection assistant, progress and Agent views
in both themes and at desktop/compact widths.

The portable public, publisher and operator browser proofs under `web/src/`
record mocked-public-boundary rendering, routing, accessibility and request
contracts. Their screenshots are valuable local evidence but are not real
NyxID, provider, checkpoint or production journey passes. Backend, SDK, secret,
image, manifest and structure gates all remain mandatory before delivery.
