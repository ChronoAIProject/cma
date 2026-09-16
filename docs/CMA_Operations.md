# CMA Operations

This runbook covers deployment-owned operations for the Sandbox/Agent model.
The current runtime contract is **3**, not the contract-2 image named in the
original #832 plan. Product semantics are defined in [CMA Model](CMA_Model.md);
public routes are in [REST](API_REST.md).

## Delivery Authority

For this milestone, the maintainer's explicit instructions override the release
step in #832: finish all development and local gates first, then deliver reviewed
changes to `develop`, build images and deploy directly to production. **Never
merge or push `main`, invoke the release workflow or change NyxID configuration
or administration.** A direct
deployment is not a GitHub release and creates no release tag by implication.

The maintainer's 2026-09-14 continuity requirement in bridge issue #850 revokes
the historical refactor reset authority. No database reset, blanket provider
cleanup, forced pause or image-driven replacement of existing live Sandboxes is
permitted. Existing user resources receive read-only verification. Faults and
cleanup use exact run-owned disposable resources with owner and lineage checks.
Changing an image default does not authorize rotation of retained runtimes.
An evidence failure requires a local fix and an isolated rerun before rollout;
unproved compatibility keeps the new feature disabled.

Current local checks are not production approval. The Release Evidence section
below records incomplete final gates explicitly. Do not advertise public
readiness or call the milestone complete while any required gate is unproved.
Controlled test enablement for the final journeys is distinct from public
publication and must be withdrawn if verification fails.

## Bridge Continuity Observation

Issue #850 is a prerequisite for enabling milestone 17. Use
`.github/scripts/cma-bridge-continuity-observer.mjs` to capture read-only public
Agent and Kubernetes evidence for exact selected Agents. The observer issues no
Agent commands, runtime exec, lifecycle calls, credential changes or cleanup.
It stores hashes of durable events instead of message/tool bodies, and refuses
gaps, duplicate sequences, incomplete pagination and unavailable observations.

Keep the configuration and receipts outside Git. The saved NyxID access-token
file must be owned by the operator and inaccessible to other users. Configuration
selects the exact verified cluster context, namespace and deployment identity:

```json
{
  "schema": "cma-bridge-continuity-config-v1",
  "origin": "https://your-cma-origin.example",
  "context": "your-verified-kubernetes-context",
  "namespace": "your-cma-namespace",
  "deploymentId": "your-cma-deployment-id",
  "tokenFile": "/absolute/private/nyxid/access_token",
  "agentIds": ["agt_selected_existing_agent"],
  "watch": { "durationMs": 60000, "intervalMs": 2000, "sampleTimeoutMs": 15000 }
}
```

```sh
node .github/scripts/cma-bridge-continuity-observer.mjs capture /private/continuity.json /private/before.json
node .github/scripts/cma-bridge-continuity-observer.mjs capture /private/continuity.json /private/after.json
node .github/scripts/cma-bridge-continuity-observer.mjs compare /private/before.json /private/after.json
node .github/scripts/cma-bridge-continuity-watch.mjs /private/continuity.json /private/watch.json
```

Capture refuses to overwrite receipts. A comparison fails on changed Agent
ancestry, creation identity or profile pin, lost/changed historical event prefix,
loss of live availability, replaced Pod, changed container/image or restart count.
Normal appended history and transitions among Ready, Working and Waiting for
approval are permitted. Review identity and history regressions against the
requirement to resume the exact retained work. A temporary API interruption may
be allowed by the deployment scope; an observer finding does not create a
zero-outage permission gate or authorize repairs or cleanup. Record a failed
command separately and use new receipt paths for its corrected rerun.

The bounded watcher uses the same protected token and exact resource scope. It
polls sequentially, retains intermediate read failures and later recoveries,
compares each complete observation with the last complete observation, and
records public read timings, hashed event prefixes, durable cursors and runtime
identity. Each poll rereads the event prefix so changed historical records are
detectable. The receipt explicitly records intervals between samples,
incomplete samples and scheduled polls skipped by slow observations. Event and
tool bodies and external exception text are excluded from the receipt.

`durationMs` is between 1 second and 1 hour; `intervalMs` is between 250 ms and
60 seconds; `sampleTimeoutMs` is between 250 ms and 60 seconds. Neither interval
nor sample timeout may exceed the duration, and at most 1,200 polls may be
scheduled. A sample timeout cancels the current read before a later poll starts.
The total deadline, SIGINT or SIGTERM aborts active HTTP/body reads and kills and
reaps the active `kubectl` process. No new poll starts after stopping. Cleanup
and writing the receipt can finish after the sampling deadline.

The watcher reserves a new mode-0600 output file before making reads. Existing
files and symlinks are refused. Its `completed` status and zero exit code mean
the bounded observation finished; inspect `assessment`, `findings`, `samples`
and `gaps` for the recorded evidence. Temporary outages remain findings after
recovery. Cancellation or setup failure writes an incomplete receipt and exits
nonzero. These statuses do not decide deployment permission or establish exact
resume by themselves.

Successful sampled observations prove only their declared coverage. They do not
prove availability between samples, native conversation identity, dirty-file
preservation, exactly one runtime input, old-tab drafts, stream reconnect or safe
cleanup. The result names these unproved properties explicitly. Complete them
with the disposable previous-version live Agent upgrade/rollback campaign in
#850/#873, including approval waits, sibling Agents, paused history and Trigger
origin, before deploying or enabling bridge changes. Keep accepted work and
existing runtime cohorts intact through both feature disable and rollback.

The shared IO admission ledger adds `agent_commands` and
`agent_command_orders` collections without changing the retained Agent or log
record schemas. Command input, response identity, immutable acceptance facts and
queue order commit in one Mongo transaction, with unique authority/key and
Agent/order indexes. Resource observations are fenced in the same transaction.
There is no TTL on command keys. An unknown commit returns an unavailable
outcome; only a retry with the same key may resolve that reservation. No runtime
input is sent by the reservation adapter.

Command workers claim one per-Agent dispatch lane with a bounded 1–60 second
lease and a monotonic fence. Renewal, release and refusal require the exact live
claim. A refusal of unclaimed queued work instead compares its exact observed
revision and proves the absence of both a binding and a live lease atomically;
this permits revoked/expired work to finish while an Agent is paused or busy.
Normal messages retain their order while controls retain the active turn
observed at admission. In one transaction, the conversation store binds the
claimed command to its exact input, metadata, operation, turn and runtime
generation, and commits the Agent and semantic logs. Runtime input follows only
a confirmed commit. Ambiguous-commit verification includes the exact command
binding, since identical controls can otherwise have identical Agent/log facts.
A bound command cannot be claimed for dispatch again, including after lease
expiry or release. Existing queued messages and Trigger first-message delivery
retain priority. Production admission and credential custody currently accept
matching personal callers; a channel envelope does not establish a personal caller.

Command outcome recovery reads the complete authoritative Agent log in one
snapshot and records the observed cursor and delivery separately from the
immutable dispatch binding. It reconstructs turn completion, interruption and
approval waits, retains unresolved delivery until runtime evidence arrives, and
never clears a newer command's lease. Runtime loss preserves uncertain writes
as typed failures even when retirement logs the turn interruption first. The
durable Agent log provides the recovery source if the response projection write
is interrupted; live fragments cannot establish completion.

Initial conversation opening persists the native identity and its open receipt
before Ready publication appends `ConversationOpened`. History readers accept
that unpublished generation only when the completion log matches the exact
Agent's durable open receipt and no Ready state or turn has been published. This
keeps early Responses admission observable across that startup boundary without
changing retained identities, log order or worker recovery. Published conversation
identity and generation still have to match their semantic log.

The production command scheduler scans nonterminal records in stable ID pages
of 50 through the additive `agent_command_work_scan` index. Successful pages
advance the cursor even if their work remains blocked. Up to eight tasks run
concurrently; slow discovery occupies a separate task. Store IO is bounded to
15 seconds, execution attempts to 45 seconds, and 30-second lane leases renew
every 10 seconds with a five-second renewal bound. Any uncertain renewal stops
the attempt. Settlement rereads the ledger before release or refusal: a binding
always selects reconciliation, and an unavailable read cannot prove an absent
dispatch. Shutdown stops new work, drains for up to 25 seconds, then cancels
remaining attempts; their durable leases/bindings support the next replica.

Unclaimed message preparation obtains fresh authority and checks the current
record again before entering the existing Sandbox revival pipeline. It holds no
short dispatch lease and sends no input. Preparation is bounded to 15 seconds;
an admitted revival remains durable if the attempt is cancelled. The resource
execution adapter checks current owner kind and target state, rereads the renewed
claim after credential IO, and retains the actual actor. It obtains an opaque
resource execution proof before dispatch. The proof binds the exact command,
owner, policy revision, ancestry and pinned profile and has a monotonic lifetime
of at most 60 seconds. Membership-dependent proof expires at the earlier
membership deadline. Reads and retries cannot renew an existing proof.

Both in-memory and Mongo conversation commits verify that proof against their
own current resource snapshot. Mongo retains the real parent write that
serializes policy changes with dispatch, and checks expiry again before commit.
The ordered runtime writer checks held policy, lifetime, target conversation,
runtime generation, operation and turn after queueing and before invoking the
runtime handler. Refusal before handler invocation returns a pre-write failure
and leaves the writer usable. Cancelling a pending send also prevents later
handler invocation without retiring the writer; if its caller disappeared before
recording the result, the durable binding still requires reconciliation. Once
handler invocation begins, uncertain delivery retires the writer generation and
requires reconciliation. Outcome evidence can commit after authority expires or
is revoked.

The production admission and execution boundaries support an independently
authorized human actor on a shared or organization-owned Agent. The actor supplies
their own retained sign-in; runtime and connector credentials remain with the
resource owner. Preparation obtains separate Agent interaction and Sandbox
management decisions, fences the exact pending command and schedules one durable
revival step. Mongo writes a fresh command-row marker alongside the parent and
event to serialize concurrent claim/refusal writers. Ambiguous commit recovery
uses that exact marker and remains possible after the proof expires. Organization
resource creation/credential provisioning and bridge delegation remain incomplete.
Human command admission and replay use the [Responses v2 contract](API_Responses.md).

`command_authorization_audits` is an additive collection with an immutable audit
ID and a `(command_id, _id)` index. New human admission, resource dispatch binding
and command-authorized Sandbox revival append evidence in the same transaction
as the accepted effect. Admission retries and AlreadyScheduled/AlreadySatisfied
revival observations retain prior evidence without appending another entry.
Each entry separates credential custody from the actual resource policy revision,
owner and actor, and records the capabilities supplied by personal ownership,
operator authority, organization roles and exact share revisions/modes. Revival
has separate Agent and Sandbox observations. These are historical observations,
never current permission; revocation does not delete them or replay accepted work.

No raw access/refresh token, input content, private runtime configuration or native
conversation identity is copied into audit evidence. Audit persistence is internal
and does not widen command status or recipient history projections. Audit IO is
part of acceptance: an audit write failure or expiry before commit rolls back the
whole effect. Old serialized command receipts and Agent/Sandbox aggregates retain
their existing formats. Older code can continue reading them during rollback,
while historical unaudited effects remain explicitly unaudited. There is no audit
TTL or cleanup migration in this change. Release upgrade/rollback evidence still
requires the disposable continuity campaign.

New authenticated Trigger launches also retain an immutable initiating-actor
sidecar, distinct from the Trigger identity that performs resource creation.
This evidence is fenced with the exact Launch effect when Agent provenance is
created. Older launches remain valid with an unknown initiating actor. Rollback
acceptance for these new writes is still open: an older whole-document Launch
writer can discard the sidecar, and an older provenance validator can reject a
distinct initiating actor. Before rollout, a compatible rollback writer/reader
and the previous-version disposable continuity campaign must preserve this
evidence; deleting or relabeling it is not a recovery procedure.

Ordinary CMA provider authentication refuses NyxID relay access, callback and
reply tokens, including relay key/scope claims. NyxID userinfo can project the
bot owner's subject and RBAC from a relay access token; that projection does not
grant CMA dashboard or owner-operation authority. The local claims inspection
only denies restricted credentials. Every other credential still goes through
the existing provider verifier. Bridge ingress must validate its own callback
and scoped execution authority before admitting work.

The disposable cluster bootstrap is
`.github/tools/cma-bridge-continuity/bootstrap.py`. Install its pinned Python
requirements, then supply the repository, a fresh private directory outside Git,
the verified cluster context and fixture owner subject. It creates a random
`cma-bridge-e2e-<run>` namespace with a resource quota, a separate Mongo replica
set Pod, provider server/key, namespace-scoped service accounts and roles, and a
credential-free home seed. It reads the deployed component pins and copies only
the existing NyxID application and storage client credentials; it never copies
production database access, encryption keys, provider keys, user homes or history.
Runtime NetworkPolicy blocks other namespaces and metadata endpoints.

The bootstrap journal records exact namespace and resource UIDs before later
mutation. Its guard refuses production namespaces, foreign markers, replacement
resources, unjournaled resources and broader cluster roles. The only fixture
cluster role permits reading the named `gvisor` RuntimeClass. `resume.py` accepts
the same private directory and reconciles exact creation intents; an ambiguous
create needs the same run marker and creation-spec fingerprint. A disappeared
journaled object is not silently recreated. Neither tool exposes blanket cleanup.

The operator needs ordinary workload creation plus permission to create/bind
the declared fixture roles. Bootstrap checks these create permissions with
SelfSubjectAccessReview before creating resources. A later admission denial
leaves the completed journal intact and must not be worked around by borrowing
a more privileged production service account. Resume requires the journaled
create intent as well as the matching resource fingerprint and namespace UID.
Initialize only the journaled Mongo Pod's replica set, wait for
the new home seed, create its snapshot and pin its observed UID before creating
the saved previous-version API manifests. Checkpoint objects for each run-owned
Sandbox use the existing `cma/checkpoints/<sha256(sandbox_id)>/` prefix, recorded
alongside the exact Sandbox identity. Existing-user checkpoint prefixes remain
outside the campaign's cleanup authority. Creating the environment alone proves
none of the live upgrade, rollback or browser acceptance cases.

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

### Image Identity

All three Dockerfiles declare final-image OCI source, revision and version
labels, including direct builds that do not use the release workflow. The
source is exactly `https://github.com/ChronoAIProject/cma`; supply the complete
reviewed source commit as `CHRONO_BUILD_SHA` and the chosen deployment build
name as `CHRONO_BUILD`. Default development values are not deployable identity.

Inspect every built platform's image configuration before publishing or
deploying. Require those exact three labels and the runtime contract label on
the Sandbox image. An inherited base-image source label, a build log, or a
Dockerfile source check does not establish the installed image's identity.
Labels themselves are assertions, not proof of origin: retain the immutable
source snapshot, build inputs, build result and scan evidence bound to each
digest. Rebuild after source changes; do not relabel older executable content
as a newer commit.

The Rust builders use locked, component- and architecture-scoped target caches
to reuse compilation during verification cycles. Each build still runs Cargo
with `--locked` and copies its selected executable out of the cache into the
build layer; final images contain neither compiler nor cache. Cached artifacts
are not acceptance evidence. Build, identity, runtime and vulnerability gates
apply to the resulting immutable image on every delivery.

### Lifecycle Scheduling

Sandbox lifecycle scheduling permits at most four concurrent sweeps, with
one new sweep offered per second while capacity remains. Durable per-Sandbox
claims and revision fences, not the scheduler's task list, grant authority.
A slow restore therefore does not monopolize all lifecycle work, and a new
pause-cancellation fence can be claimed while its superseded capture unwinds.
Four simultaneous long operations can still consume all slots; this is a
bounded concurrency improvement, not an unbounded latency guarantee.

Workspace, periodic checkpoint maintenance, and checkpoint-object cleanup
sweeps remain serial. Shutdown stops admitting sweeps, permits a 25-second
drain, and cancels remaining child tasks without clearing their durable leases.
The API requests 256 MiB and is limited to 1 GiB to accommodate the existing
bounded multipart buffers across concurrent lifecycle transfers.

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

## Browser Authority And Recovery

A valid CMA browser cookie identifies the account; it does not by itself grant
resource permissions or prove renewable credentials are available. In provider
mode, requests with usable durable access tokens verify live userinfo and,
when advertised, introspection. Stored permission snapshots are not request
authority. Discovery, userinfo and introspection share a 15-second verification
deadline; a durable reread then requires the same owner, login incarnation,
token generation and exact still-usable access token. This is a request-time
check, not an atomic binding of a later browser request or resource mutation.

Permission evidence must contain a complete string array. Introspection must
contain a boolean `active`; an optional `sub` must exactly match userinfo.
Malformed evidence or provider failure fails closed without using old grants.
Missing, expired or blank durable access, or explicit inactive evidence, leaves
browser identity only: no capabilities and no delegated credential. An active
token with `permissions: []` grants no capabilities but remains a verified
delegated credential. Bearer and login verification reject inactive evidence.

On-demand renewal is coordinated in MongoDB, not a process-local mutex. Each
subject has a login `incarnation`, monotonic `revision` and `token_generation`,
and a nullable `refresh_lease`. The lease lasts 45 seconds; another caller waits
at most 20 seconds, polling every 50 milliseconds. A cancelled worker leaves a
bounded lease for takeover. Completion checks the exact incarnation, generation,
holder and unexpired lease, then replaces by revision without upsert. New login
invalidates old claims; stale success, rejection or abandonment cannot overwrite
it or release another worker's claim. Completion preserves the latest profile
and grants; terminal token loss clears stored grants in the same replacement.

Only a refresh response with HTTP `400` and `error: "invalid_grant"` discards
renewable custody. HTTP `401`, `429`, provider outages, malformed responses and
transport errors retain it and release only the caller's claim. Nonrenewable
access remains usable until actual expiry, when tokens and stored grants are
cleared. Browser-cookie expiry and logout remain separate from token custody.

The exact `/auth/login`, `/auth/callback` and `/auth/logout` routes do not resolve
ambient credentials before running. A stale cookie therefore cannot prevent
explicit sign-in or logout. Callback state, expiry, one-time consumption, PKCE
and initiating-browser binding still apply; navigation or callback never starts
a Sandbox or Launch. Other authenticated routes still verify current authority.

Successful explicit login can repair an exact current-schema `auth_subjects`
row only when **all four** fencing fields are absent. It installs new verified
tokens and a fresh incarnation. Existing cookies and background refresh cannot
infer missing generations or repair such a row. Partial or malformed fencing,
unknown document schemas and counter exhaustion fail closed, including during
login. Do not add fields by hand or erase authentication collections as a
recovery shortcut; those cases require a separately reviewed data repair.

New Sandbox admission additionally requires the owner's renewable credential
prerequisite. A typed `409 sandbox_credential_setup_required` requires explicit
same-account sign-in and continuation. Retain an ambiguous command's original
key and body; use a fresh key only after a definitive refusal. A successful
`GET /auth/session` alone does not prove that the next admission will succeed.

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
| `CHRONO_BRIDGE_MANAGEMENT_ENABLED` | Fresh Bridge setup/configuration/enable/grant/import/retry authority; false by default; retained reads, exact receipts and safe reductions stay available |
| `CHRONO_BRIDGE_INGRESS_ENABLED` | Fresh signed callback acceptance; false by default; accepted input can drain through independently enabled stages |
| `CHRONO_BRIDGE_CONTROL_ADMISSION_ENABLED` | Fresh signed cancel/steer admission selector; false by default; existing Message/refusal/control receipts recover first. Production runtime eligibility remains unavailable, so this selector alone cannot enable controls or advertise operations. |
| `CHRONO_BRIDGE_ALLOCATION_ENABLED` | New Channel allocation/provisioning/child/start effects; false by default; preserve current phases and in-flight outcomes |
| `CHRONO_BRIDGE_EXECUTION_ENABLED` | New Channel command admission and held-command release; false by default; already Released commands drain in shared FIFO order |
| `CHRONO_BRIDGE_OUTBOUND_ENABLED` | Fresh explicit reply-tool admission and provider send attempts; false by default; recover retained receipts/outcomes and never auto-retry Unknown |
| `CHRONO_BRIDGE_TELEGRAM_ENABLED` | Telegram fresh-effect gate, independent of other platforms and stages; false by default |
| `CHRONO_BRIDGE_TELEGRAM_NEW_ENABLED` | Telegram-new fresh-effect gate, independent of Telegram and other stages; false by default |
| `CHRONO_BRIDGE_DISCORD_ENABLED` | Discord fresh-effect gate; false by default |
| `CHRONO_BRIDGE_LARK_ENABLED` | Lark fresh-effect gate, independent of Feishu; false by default |
| `CHRONO_BRIDGE_FEISHU_ENABLED` | Feishu fresh-effect gate, independent of Lark; false by default |
| `CHRONO_BRIDGE_V2_UI_TRANSPORT_ENABLED` | Eligibility for the separately integrated v2 browser transport; false by default; does not disable existing public v2 API routes or change an accepted request protocol |
| `CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED` | New authenticated Launch admission selects common first-message commands under retained personal ownership; false by default. Proven pre-write failure is terminal for new common prompts; ambiguous writes retain the same command/response and refuse legacy recovery. Accepted protocol is immutable; disabling the selector retains accepted common commands and legacy launches retain their original delivery/recovery behavior. Keep false until every worker and permitted rollback version recognizes and transactionally excludes common/legacy dispatch; older workers ignore additive sidecars. |
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
| `CHRONO_SANDBOX_HOME_STORAGE_CLASS` | Durable home StorageClass; configure all four home fields together, or omit all for legacy ephemeral operation |
| `CHRONO_SANDBOX_HOME_CAPACITY_GIB` | Home disk capacity, integer 1-1024 GiB; production 64 |
| `CHRONO_SANDBOX_HOME_TEMPLATE` | Ready, credential-free seed VolumeSnapshot in the Sandbox namespace; initialized with `infra/storage/home-seed.yaml` and `home-template.yaml` |
| `CHRONO_SANDBOX_HOME_TEMPLATE_UID` | Exact immutable Kubernetes UID of the seed snapshot; startup rejects a missing, replaced or unready seed |
| `CHRONO_UPSTREAM_BASE_URL` | Required canonical HTTPS model API base without trailing slash |
| `CHRONO_UPSTREAM_WIRE_API` | Exactly `responses` |
| `CHRONO_PLATFORM_BASE_URL` | Required canonical HTTPS platform proxy origin; also the upstream for the runtime's explicitly granted GitHub API proxy |
| `CHRONO_BASELINE_SERVICES` | Default `chrono-llm-public,ornn-api`; unioned into newly admitted Sandbox scope |
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

## Bridge rollout switches

The registered feature switches are independent process-start booleans. Missing values
mean false; only the exact strings `true` and `false` are accepted. A stage also
requires its pinned platform switch. Enabling management does not enable ingress
or workers. Disabling ingress lets previously accepted work drain when its other
stages are enabled. Switches supply availability only; current caller, Bridge,
grant, custody, resource and original-source checks still apply.

Execution disable stops new Channel admission and held-command release. Commands
already Released into an Agent's shared queue drain in original FIFO order;
dispatch-bound commands reconcile their existing effect. Do not skip a retained
Channel head to prioritize another caller, and do not pause the common command
or lifecycle workers. Held reservations can temporarily consume shared capacity
until their original deadline or a proven revocation. Immediate clearance needs
an explicit audited cancellation operation, not a flag-induced silent reorder.

Outbound disable preserves exact tool receipts, pending order, original
custody/destination, deadlines and attempts. Recover the original receipt before
refusing a new tool intent. In-flight sends may finish recording; expired Sending
attempts become Unknown through local reconciliation. Reenabling never supplies
retry consent or resets an attempt, deadline, source identity or reply token.
Unknown delivery still requires the explicit recovery command and its duplicate
risk acknowledgement. Local expiry and outcome bookkeeping must stay available.

Retain accepted-input disposition evidence independently of payload TTL. An
input paused past its original admission deadline expires truthfully; it must not
silently disappear or obtain a fresh deadline on reenabling. An already accepted
or uncertain browser submission retains its protocol, exact body and idempotency
key even when v2 UI transport eligibility changes. The existing public v2 API
continues serving SDK callers.

A rollout is globally disabled only after every old replica has stopped starting
new work and drained its in-flight effects. A process environment change is not
an instantaneous global cancellation. No runtime image rotation, live Agent pause,
provider-role expansion, database reset or retained-resource rewrite is required
to change these control-plane switches.

Channel-created active Sandboxes renew their runtime credential through a
separate bounded scheduler. Each step verifies the original retained human
control-plane grant, current selected-key custody, original allocation and
current resource identity. Renewal replaces the key on the same runtime and
revokes the previous key after replacement; it does not create another Agent or
conversation. Renewal audit records retain the actual human actor and hashed
grant identity without credentials. An unknown mint result stays held and must
not cause a second mint request.

This maintenance remains available when new-admission switches are disabled.
The renewal lease prevents concurrent Sandbox lifecycle changes while accepted
Agent commands continue through their shared queue. A retained Ready allocation
can renew after reset only under the original resource identity and the current
`DrainOriginal` policy. `SuspendOriginal`, removed or replaced human consent,
and retired resources block renewal. Ordinary owner-token issuance never
substitutes for missing Channel authority.

Dedicated Channel revival and retirement use their retained original-human,
allocation, resource and runtime authority. Renewal alone does not authorize
restore or teardown. Retirement retains home, checkpoint and history data and
holds final Retired publication and physical cleanup. Image rotation remains
held. See [Channel lifecycle recovery](CMA_Channel_Lifecycle_Recovery.md) for the
implemented paths and their remaining verification boundaries.

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

## Deployment continuity and historical reset procedure

The migration-era reset and direct-deployment procedure is revoked for milestone
17 by [#850](https://github.com/ChronoAIProject/cma/issues/850). It provides no
current authority to reset a database, rename live collections, remove owner
Sandboxes, delete provider inventory or adopt a replacement runtime for an
existing Agent. Deployment requires the completed continuity evidence and a
compatible rollback floor described at the start of this document.

Health becoming Ready is not proof of conversation continuity, checkpoint
restoration, baseline visitor access or correct embed behavior. The API pod's
termination grace must exceed its bounded HTTP and worker drains; review it
whenever those timeouts change. Preserve accepted operations, immutable creation
receipts, native conversations, checkpoints and unresolved cleanup authority.
An expired client deadline or stopped worker does not prove an earlier provider
effect can no longer finish. No ordinary rollout may delete owner Sandboxes to
conceal a recovery defect.

### Historical collection evidence

The September 7 inventory and isolated migration rehearsal remain historical
evidence only. They do not establish the current production inventory or permit
repeating collection renames, index removal or resets. The local ignored
`archived_legacy_collections_preserve_custody_and_allow_full_new_store_bootstrap`
test exercises initialization beside archived records and the prior event-index
boundary in an owned disposable database.

Retain historical encrypted custody with its original encryption key and
deployment identity, unresolved revocation debts, session tombstones and exact
legacy object-key ownership until separately verified settlement. A rejected
revocation is not successful cleanup. Historical view definitions and index
metadata remain part of restoration evidence; a generic backup does not prove
that the original namespace, indexes and native conversations can be recovered.

Never start an incompatible historical API against the live database to settle
old debts. Its workers could reopen namespaces or run renewal, checkpoint and
reconciliation against resources they cannot safely interpret. Any legacy
cleanup requires a separately reviewed scope and exact retained authority;
normal CMA-owned key revocation does not authorize NyxID role, account, service
configuration or administrative changes.

### Existing-Owner Verification

The maintainer currently supplies one existing NyxID account. Its local CLI
login can authenticate the real CMA authorization-code flow: preserve CMA's
state, initiating-browser cookie and PKCE challenge, send the CLI access token
only to the exact trusted NyxID authorization origin, and let CMA's callback
establish its own renewable custody. Do not inject CLI refresh tokens into
MongoDB or treat a bearer-only `/auth/session` result as browser sign-in proof.
Keep any browser storage state and credential sentinels in regular `0600` files
outside the repository. Never place credentials in process arguments or reports.

`web/e2e/owner-config.ts` is a separate opt-in entry point with exactly one
`OWNER-resource-lifecycle` scenario. It creates one Sandbox, Workspace and Agent
from reviewed owner-published revisions, observes a completed first answer,
checks browser return and ordinary pause/resume, then submits exactly one public
AG-UI continuation. The second completed answer must recall the first-turn
marker and reported Worktree directory. Exact first-turn history and the
Agent/Workspace/Sandbox/Worktree identities must remain unchanged. Another
browser reopen must retain both durable turns without another create, run,
Pause or Resume command. Cleanup touches only resources recorded in its
owner-bound journal. It does not use operator fault controls.
Ambiguous creation retains cleanup ownership instead of deleting possible parents.

AG-UI `runId` is correlation, not idempotency; the harness refuses every repeat
submission. The finite POST stream is inspected in memory through the SDK frame
decoder, with an 8 MiB total bound, the existing frame limits and a 180-second
deadline. Raw SSE is not persisted. Local transport and guard tests do not prove
that the deployed model continues successfully; that requires executing the
remote scenario against the independently verified deployment.

Supply `CMA_OWNER_E2E_TARGET` and `CMA_OWNER_E2E_MUTATION_TARGET` as the same exact
HTTPS origin, `CMA_OWNER_E2E_SUBJECT`, `CMA_OWNER_E2E_FIXTURE_FILE`, and a fresh
`CMA_OWNER_E2E_OUTPUT_DIR`. The private fixture contains the same origin/subject,
browser storage-state and forbidden-values paths, exact Sandbox/Workspace/Agent
profile IDs, revisions and digests, plus CMA/NyxID commit and image-digest
provenance. Set `CMA_E2E_BROWSER_PATH` when using an installed browser. From
`web/`, discovery requires no remote inputs:

```sh
./node_modules/.bin/playwright test --config e2e/owner-config.ts --list
```

Removing `--list` executes the explicitly configured remote run. Its evidence
is labelled `existing_owner`, one account, and `owner_only_partial`; supplied
provenance remains operator-declared until independently verified. This does
not advance the full J/G campaign. Fresh signup, restricted-visitor behavior,
cross-owner isolation and second-owner forks remain unproved without another
account. The lifecycle baseline also does not prove runtime rotation, restart
fault recovery, external storage cleanup,
provider login, real embeds, or public readiness. Existing full-campaign inputs
and gates remain unchanged.

## Checkpoints And Cleanup

The external storage service is reached through the platform machine-credential
flow and `/api/v1/proxy/s/chrono-storage-service`. The existing bucket is retained
under its new checkpoint configuration name. MongoDB owns checkpoint manifests,
writer fences, publication authority and deletion intents; external objects own
the bytes. GridFS is not an acceptable substitute for the required external
persistence boundary.

Archives above 32 MiB use bounded 32 MiB physical objects through that same
authenticated storage route. The Mongo stage's opaque key records the versioned
part layout and total size before any upload; cleanup can therefore enumerate
every part after an interrupted upload, even if no part manifest was published.
A bounded manifest is uploaded only after every part is readable and both the
part digests and whole-archive digest match. Restore verifies the manifest,
each part and the complete archive. Existing single-object checkpoints remain
readable. Do not roll back to an API that cannot read the part layout after a
multipart checkpoint is published. This avoids the production ingress's HTTP
413 limit without changing runtime archive bytes or writer-fence authority.

New upload stages record a fixed 1,200-second (20-minute) client transfer budget
in `cleanup_after`, increased from 600 seconds after a still-progressing
6.26 GB upload exhausted that shorter budget. This is a CMA-owned bound, not an
expiry sent to the external storage service. Existing stages keep their exact
recorded deadlines across upgrades; never extend a live stage by editing its
record. Transfer cancellation and cleanup both use the stored deadline.

The 180-second per-request timeout, four concurrent 32 MiB parts, three bounded
immutable-part attempts, exact readback checks and Retry-After handling remain
unchanged. Storage credentials are refreshed between requests when needed.
Both periodic and explicit lifecycle captures still have a 1,800-second total
bound, including archive creation and publication, so a 20-minute upload budget
leaves less than ten minutes for the remaining work and does not guarantee every
large home can finish. Periodic capture claims renew every minute; the explicit
lifecycle worker retains its configured 31-minute lease. Timeout preserves
writer-recovery and cleanup authority rather than claiming publication.

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
| Runtime image default changes | Retain existing runtimes, both Agents, dirty Worktree and installed software; new allocations use the new image, and rollback preserves both cohorts |
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

The local credential-setup browser checks also passed 3/3 on 2026-09-07 via
`npx playwright test --config e2e/credential-setup.config.ts` from `web/`.
`CMA_CREDENTIAL_PREVIEW_URL` selects the loopback preview, with the same default
and browser-executable option as the suites above. All three surfaces (Sandbox
creation, Trigger landing, and draft continuation) preserve the current tab and
choices after a typed `sandbox_credential_setup_required` refusal. They open the
existing CMA sign-in URL in a separate window, handle a blocked window, reject
a different account on recheck, and require an explicit fresh-key retry after a
same-account check. Each later dispatch rechecks the live session against the
original account, including ambiguous same-key retries; account changes block
the mutation without discarding its key/body. This check does not atomically
bind a later HTTP request to that session. A later lost response keeps that new
command's original key/body. Trigger recovery retains the original repeat
choice, placement and referrer through reload. Pending connection completion
cannot replace the explicit Continue required after Draft recovery. Reload
restores that per-Launch retry requirement, its original account, and any
unresolved Start key/body; it does not admit another command. Eighteen screenshots cover
both themes at 1440, 390 and 320 pixels in a fresh
`cma-credential-setup-browser-*` OS temporary directory.

This suite intercepts sign-in navigation and identity reads locally. A matching
session confirms only account identity, not renewable credential readiness;
the next public admission may refuse again. It executes no NyxID login,
credential minting, remote Sandbox provisioning or remote credential mutation.

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

### Explicit Sandbox command scheduling

The API process starts the durable Sandbox command scheduler under its existing
shutdown coordinator. It polls once per second, discovers at most 50 queued
records per page, and runs at most eight commands concurrently. A blocked page
retains its cursor progression so later commands are discoverable. Local duplicate
work is suppressed; competing processes are fenced by the exact queued record and
atomic Sandbox revision/authority checks. Shutdown allows 25 seconds for active
work to drain; remaining tasks are cancelled and durable records remain recoverable.
The original five-minute deadline is never renewed. Transient provider/store
failures retry through discovery; terminal rejection does not schedule runtime IO.
Applied receipts describe durable transitions, whose runtime work is completed
by the existing lifecycle recovery loop. Public reads and same-key retries recover
lost acknowledgments without adding another transition.
