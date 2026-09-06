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
   A stopped worker or elapsed client deadline does not prove that an earlier
   external upload can no longer finish; preserve its exact cleanup authority.
4. List the provider's complete paginated inventory, resolve only CMA's matching
   deployment/namespace labels, and delete each resolved Sandbox via the provider
   API. Verify each absence; do not use namespace-wide Kubernetes deletion.
5. Resolve and confirm the exact CMA database name from live configuration.
   Preserve `checkpoint_archive_tombstones` and all still-needed exact object
   deletion intents before resetting historical application collections. Prefer
   a reviewed collection-by-collection reset that leaves this cleanup authority
   intact. `dropDatabase()` is permissible only after a protected, verified
   export of that authority and an explicit restore plan: retain exact opaque
   physical keys, schema versions and retry state, verify the restored inventory
   against the export, and restore it before starting any new cleanup worker.
   A generic backup without a verified restoration procedure is insufficient.
   Never drop the URI's default database, an admin database, NyxID's database or
   a database whose identity was inferred from a shell default. Historical-state
   reset authorization does not authorize losing uncertain-upload ownership.
6. Clean only exact historical CMA object keys proven orphaned by the retained
   inventory. Do not delete the shared external bucket or apply broad lifecycle
   expiry to active checkpoint metadata/tombstones. Failed/ambiguous cleanup is
   retained for reconciliation, not reported as completed. A temporary GET 404
   after DELETE does not expire authority for an earlier ambiguous upload; absent
   actual backend evidence bounding its write lifetime, keep its tombstone.
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

### Browser Harness Prerequisites

The staging harness lives in `web/e2e/`. Run commands from `web/` after `npm ci`:
`npm run e2e:typecheck`, `npm run e2e:mechanics` for local browser mechanics, and
`npm run e2e` for the configured remote target. The default remote run is only
three read-only smoke cases. Listing cases with `npm run e2e -- --list` executes
no journey. No staging fixture or credential is supplied by the repository.

Read-only smoke requires `CMA_E2E_BASE_URL` (one exact HTTPS origin),
`CMA_E2E_PUBLIC_ID` (an existing published Trigger), and `CMA_E2E_OUTPUT_DIR`
(a fresh absolute evidence directory outside the checkout, under an existing
parent). Existing leaves and symlinks are refused. Use
`CMA_E2E_BROWSER_PATH` when selecting an already installed browser. Mutation
journeys additionally require `CMA_E2E_MODE=mutation`,
`CMA_E2E_MUTATION_TARGET` exactly equal to the origin, `CMA_E2E_FIXTURE_OWNER`,
and `CMA_E2E_FIXTURE_FILE`. The manifest must match that origin and owner.
It and every referenced auth state, hook and credential-sentinel file must be
private regular files outside the checkout, with mode `0600`; never commit them
or reference a symlink into the repository.

The manifest contract in `web/e2e/environment.ts` requires separate owner and
second-owner identities, a distinct visitor identity for each exported visitor
case, baseline services, exact immutable profile revisions, all required Trigger
states, OAuth/API-key/device-code connections, controlled fault fixtures, and
four real embed hosts (Markdown, HTML, JavaScript and React). It also records
the declared CMA/NyxID commits and immutable API/web/runtime image references.
The external hooks must actually complete ordinary signup/connection browser
flows, arm/release the specified owned fault, and observe the required runtime
state. Placeholder success hooks are not evidence. Missing prerequisites fail
the campaign; they must not become skipped cases reported as completed.

Preflight reads the supplied identities and capabilities before any mutation.
Visitors need Launch/use-own/delete-own capabilities without direct Sandbox
creation, profile management or Trigger publication; owner fixtures need the
relevant creation/publication capabilities. Provision an explicitly authorized
disposable campaign with sufficient capacity: J1's two live always-new Sandboxes
cannot pass under the default visitor one-Sandbox limit. Do not silently loosen
quotas or substitute an administrator for a fresh visitor. Signup account cleanup
remains an external operator responsibility; the harness never calls NyxID
administrative account/role handlers.

J0 verifies the newly registered account has exactly `launch_triggers`,
`use_own_sandboxes` and `delete_own_sandbox`, returns to the original placement,
and receives a completed first assistant answer without another CMA command.
J5 requires the pinned Workspace fixture to identify a verifiable public Git
repository, a live Ornn catalogue with a published skill release, readable
deployment resource bounds, and a service catalogue containing a declared
baseline service. It verifies the repository before publication, uses the live
skill/resource/service controls, and forks both Agent and Sandbox Profiles as
the second owner while checking provenance and copied content. J6 needs capacity
for three Agents: two for the existing home/rotation proof and one admitted while
Paused through normal creation. That third creation deliberately loses only the
browser response, retains the real admission in the cleanup ledger, retries the
same key/body, then reloads without an explicit revive or another input command.
These additions are campaign requirements, not remote execution evidence.

The owner-bound append-only `cleanup.ndjson` ledger contains only resources
actually created by this campaign, including late child resolutions from its
admitted Launches. Reused resources are not cleanup-owned. Cleanup stops Agents
before deletion, retires published objects, verifies terminal state/absence, and
keeps polling a cancelled Launch even after a cancellation conflict. A started
Launch with unresolved children is not safe to clean merely because it reports
`cancelled` or `failed_terminal`; retain the ledger for reconciliation and do not
delete possible parents while creation authority remains ambiguous.

Traces, HAR, video, default screenshots and Playwright raw DOM error contexts
are disabled. Only guarded canonical CMA-page screenshots and curated reports
may enter evidence; check DOM, browser storage, password fields and configured
credential sentinels before capture. Keep temporary raw runner output outside
the checkout and out of published artifacts. The custom report contains static
case names, status, duration and safe prerequisite failures, not assertion dumps.

The scheduled workflow is GET-only. Mutation execution is manual-only in the
protected `staging-e2e-mutation` environment, on the isolated labelled self-hosted
runner with externally provisioned fixtures and trusted `develop` checkout.
Adding the harness or enabling a job does not authorize a staging campaign.
Preflight creates the evidence leaf exclusively with mode `0700` and marks it
with a fresh run identity only after prerequisite validation succeeds. The
reporter checks that identity before writing; artifact upload requires the
current harness step's validated-setup output and lists only curated filenames.
A failed preflight cannot authorize uploading an old output directory.

### Evidence Classification

Local mechanics checks use intercepted browser HTML and prove harness behavior,
not staging journeys. Shared backend/SDK corpus replay proves deterministic
public contract composition, not a live NyxID, kernel or storage deployment.

On 2026-09-07, the built-UI local profile-authoring checks passed 3/3 at
1440, 390 and 320 pixels. They exercised skill/service unavailability and retry,
preserved draft values and skill pins, delayed/unavailable resource bounds, and
repository stale-result rejection, typed failure, retry and canonical URL
preservation. The separate local startup checks passed 5/5: Workspace and Agent
creation from both Paused and Reviving parents, lost-response same-key/body retry,
accepted reload without another create or manual revival, and durable sign-in
notice replay/clearing while preserving unrelated first-message recovery without
a fake turn or browser write. Startup captures cover both themes at all three
widths, including the unsettled dialog and accepted/replayed states.

Run these local suites from `web/` against an existing production-build loopback
preview (default `http://127.0.0.1:4342`) with Node 26.8.1 and an installed browser:

```sh
npm run build
npx playwright test --config e2e/profile-authoring.config.ts
npx playwright test --config e2e/agent-startup.config.ts
```

Use `CMA_AUTHORING_PREVIEW_URL` or `CMA_STARTUP_PREVIEW_URL` to select a different
loopback preview, and `CMA_E2E_BROWSER_PATH` to select the browser executable.
Each suite writes screenshots and structured local-boundary attachments to its
own fresh OS temporary directory (`cma-profile-authoring-browser-*` or
`cma-agent-startup-browser-*`), outside the checkout. HTTP responses are
intercepted local fixtures, including recorded backend response shapes; the
sign-in destination is an inert example. No provider login, remote creation,
actual repository resolution, runtime revival or remote failure is executed by
these checks. They do not advance any remote gate or journey in the tables below.

The official Node/Python Responses client checks use the real loopback CMA
composition with scripted runtime input and in-memory persistence. Their pinned
client versions and exact verification boundary are recorded in
[API_Responses.md](./API_Responses.md#6-official-sdk-conformance); they are not
production runtime evidence.
TypeScript replay labels each case by its actual boundary: `wire` calls the
typed operation and checks the emitted request against the recorded request;
`preflight-validation` proves local rejection without a network call;
`serializer-protection` proves that typed serialization prevents the malformed
request; `browser-only` checks navigation URL construction or the server-owned
callback composition. Protected malformed requests and OAuth callbacks are not
SDK HTTP round trips. Their recorded problem documents are checked separately.
`raw-wire-only` body-limit probes use bounded compact byte expansion in the
backend corpus. They cannot be emitted by the SDK's structured serializer and
prove only recorded error decoding on the SDK side, not a typed HTTP replay.
Rust replay makes the same distinction between actual typed HTTP, SDK preflight,
unrepresentable wire/type shapes and browser-owned navigation. Deserialization
failure while constructing a typed fixture is not SDK validation. Both kinds of
local refusal independently check the recorded problem decoder without claiming
that the protected malformed request was sent over HTTP.

Backend fault scenarios exercise real public handlers and services with selected
Core ports returning source-backed typed failures. They prove the public error
projection, not that an external outage or an impractically large model boundary
was reproduced. For example, a publish revision-overflow port failure does not
claim construction of billions of retained Trigger revisions. The inventory
separates HTTP problems from protocol failure outcomes; error-only streams do
not satisfy successful-stream coverage. A retrieved failed Response is a valid
resource projection, not an HTTP refusal. Propagated runtime failure classes are
open strings, so their fixtures are representative protocol examples, not an
exhaustive catalog of provider errors. Fixed lifecycle stop reasons remain
separately enumerated in the protocol inventory.

Remote harness reports identify `executionTarget: configured_remote_origin` and
`mocked: false`, but supplied commit/image pins remain
`provenanceSource: operator_declared` and `provenanceVerified: false` until
independent deployment evidence verifies them. Smoke reports supply no pins.
Do not relabel declared provenance as remotely observed identity. The deliberate
browser transport-response-loss case is local fault injection; provider failures,
restart/rotation and runtime observations require their real audited hooks.

No remote campaign has been run by adding this harness. G-A1-G-A5 still require
independent proof against the final pinned kernel/runtime; no browser pass can
establish prompt order, skill hot reload or exact conversation restore alone.

### Required Final Gates

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
