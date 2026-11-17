# 12d to 13b reader/writer matrix

> Status: **12d ROLLBACK FLOOR FAIL; full matrix incomplete**. The exact 12d image failed startup after the selected 13b image changed a Lifecycle Sandbox's Always-on policy. Seven separate campaigns passed all four HTTP stages for selected shapes in 27 distinct collections, with twenty-one complete zero-failure startup observations. Those subset passes do not override the failed rollback. The remaining 133 collections and additional nested shapes, writer policies, and native lifecycle/takeover cells are unverified.

The 2026-11-17 static audit produced 160 collections, 480 retained/disabled/enabled writer fixtures, and 1,920 reader/writer cells. These are planning counts, not executed passes. Private audit: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261117-static-v2/COMPLETION-AUDIT.json`, SHA256 `9d6ad1ac0ec6d3da17b0529d818f9d73498d5005b3505475a0ad09248fa889ea`.

## Scope and immutable identities

| Role | Source | Tree | Image | Evidence |
| --- | --- | --- | --- | --- |
| historical / 12d | `cccb86ace3c9c0801e58dfc49c46276006d99f48` | `8e7d5d55190d81959f9c9989c07615197c7f8c8c` | `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:a7334c9181644a99ef87417cdafaecae99c9e027340e401c4c41ec08e353abb3` | `BATCH12D-CURRENT.json` / `FINAL-HANDOFF.json` |
| historical 13b execution target | `9217fbce68cb03b8da633cb98eed90f0ec846b06` | `0789650927f17e2ab3b16fcd82acae9eda7e82cf` | `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:3208839839da2897465019f3bcb54817e25f9df6b737b2f90b14ec98b37bfb01` | `FINAL-IMAGE-STATUS.json` / local artifact `aacd3cd6…` |
| selected combined 13b candidate | `cd91b713d74bb251506d094c3c20d87a329b4791` | `d9e6d8159dfa21cebdae596986f2f9b92497dd77` | `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:260bc820b7c39cba17748dfeaccd291793f35f49c851c9829bc32145e2c2b851` | `CANDIDATE-INTAKE.json`; local artifact, no registry-publication claim |

The historical 9217 artifact is source-labelled and locally available; its build metadata does not imply C275, run17, HPA, or migration qualification. That historical source inventory contains 160 collections and is retained privately at `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/source-inventory-9217.json` (SHA256 `5a60ffb9de06b568316fd2b3e26c719728ce39ef5c4c8eaf9129c29328189186`). The executable plan is `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/plan-9217.json` (SHA256 `510a58bb72d1c818ea31b8ae38bf20c8e8389882d3adfed78d02ead2f95e371c`).

The combined candidate in the table has config
`sha256:e2a1ba9dab16b4e0da660486a84ddf12fd42bdb25a0372db73b0ee41aa17ea2c`.
Its producer receipt says `LOCAL_ONLY`; the rehearsal reference does not prove registry publication.
Private intake: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-candidate-cd91-20261117/CANDIDATE-INTAKE.json`,
SHA256 `84c73a58b1388e4f6b8e0911931feb828276ef34f6a2a536df338b3429f0ff9b`.
All eight intake hashes match; the 160-collection, 480-fixture, 1,920-cell plan validates.
Historical results do not transfer. The delta changes checkpoint retry exhaustion and owner-routing
future allocation; known-abort refresh, unknown-commit reconciliation, and owner-routed admission
through restart remain outside the completed HTTP subset.

Campaign v36 (`/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261117/run-9217-v36/kind-evidence.private.json`, SHA256 `290b02d00c05be04123e63216f67da9713e7cc4335a0e5d826c2f6a0d33023b7`) is a historical corrected run against 9217. It used the exact 12d and 13b image digests above, the local linux/amd64 Mongo digest `docker.io/library/mongo@sha256:494b956596706b19ba44908cb9d03648b585214600987b86cd2a34249358e572`, and the corrected cursor-following activity fixture. Historical, current, and rollback boots all reached Ready with restart count 0; all four directional stages passed with three samples each, eleven collections, and nine declared shape classes. Cleanup recorded `cluster_absent: true`. The Mongo digest differs from the unavailable default dependency only as a local immutable test-runtime substitution; it does not change the application image identities or promote this subset to the M-OPS-02 gate.

Campaign v37 attempted to add Trigger definitions, published revisions, active and retired placements, public previews, and completed operation replay through the real HTTP API (`/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261117/run-9217-v37/kind-evidence.private.json`, SHA256 `257bd88dffa1972199fa79e65a1f0e98b7b9949f9d9873e8b99022e0a6d1b3dd`). The 12d historical boot reached Ready with restart count 0, then the fixture stopped at `old-write`: the API's shared ten-per-minute create budget returned `429` during the first immediate operation replay. The runner removed `cma-matrix-4ecc8c3107` and recorded `cluster_absent: true`; no Trigger receipt is counted. The fixture now paces POST requests at the manifest's documented budget and keeps unexpected 429 responses fatal; its unit tests are on commit `af1d0a895`. This attempt leaves the matrix **UNVERIFIED**.

The browser fixture also prepares a revoked-session shape. Each writer signs in,
logs out through the real HTTP route, then creates a separate live session for
the same subject. Both readers must reject the original cookie with HTTP 401
and an explicit anonymous body, while preserving the live session and pending
PKCE callback. Refreshing the subject's tokens must not revive the revoked
browser session. Loopback HTTP regressions cover all four fixture stages,
session resurrection, malformed denial/logout responses, missing revocation
state, and wrong writer provenance. Combined campaign v6 also passed the
revoked-session checks against both application images. Earlier frozen campaign
packets remain immutable and are not relabelled with this successor result.

The composite fixture combines profile and Trigger exercises with
`migration_usage_fixture.py`, which admits admin usage reports through the public
API. Each writer uses an explicit past range and a unique query, then checks
coalescing within one recorded admission minute. Both pinned sources include that
minute in `query_hash`, so reader stages GET the saved report ID without
re-submitting the query. A pending worker may recompute observation timestamps
and effective range when claimed; immutable identity/query/scope fields must
survive, while an already ready snapshot must match every returned field.
Incomplete ready responses, terminal failures, changed identities, and HTTP
errors are rejected. Empty page/series/owner responses are never counted as
persisted row/chart documents. A crossed coalescing minute is an unmet fixture
precondition with no automatic retry or success receipt.

The correction has two reproduced baseline failures and passing successors, 17
usage unit tests, and three local HTTP contract tests, including all four stages
across minute changes. These validate the harness, not application artifacts.
That historical fixture suite passed 67 tests with zero skipped; standard-library
trace reports 83% statement coverage for the usage fixture. Private evidence is
`/Users/shining/code/work/tech-lead.staff/developer-19/migration-usage-fixture-20261117/`.
Combined campaign v6 passed report admission/control compatibility in both
directions. Empty row/chart responses remain excluded from persisted row/chart
coverage. The historical cluster remains stopped.

The optional `migration_ram_kind.py` adapter preserves the shared Docker disk
reserve by mounting the disposable node's `/var` on a private 6 GiB tmpfs volume.
Pass it through the runner's `--kind` option and set `MATRIX_REAL_KIND` to the
ordinary kind executable. It limits the node to two CPUs and 8 GiB memory with
no swap, verifies the actual filesystem and limits, and checks resource ownership
before removing storage. Mongo survives every application image switch within
the node; this setup makes no claim about node or host failure durability.
Eleven adapter contracts and an actual bootstrap/cleanup check pass. Shared
clusters, volumes, and image caches are outside the adapter's cleanup scope.

The first combined-candidate attempt using this adapter stopped because
kubectl's client-generated `List` omitted the server resource version. The
runner now reads the server's `PodList` directly before replaying startup events;
missing or expired history still fails. The next attempt captured the complete
12d startup with zero failed attempts, then correctly stopped on HTTP 422: the
test's concurrent workspace edit used spaces in a path-component name. Valid
distinct workspace names correct that fixture without relaxing its one-success,
one-conflict, exact-replay, or no-silent-overwrite assertions. Both defects have
reproduced failing regressions and passing corrections. All 123 Python and 15
Node harness tests pass on the isolated integration tree. Both failed attempts
and their successful cleanup receipts remain private under
`migration-resume-cd91-20261117-v4/` and `migration-resume-cd91-20261117-v5/`.
They establish no reader/writer compatibility result.

The Bot extension adds `migration_bot_fixture.py` and the runner's opt-in
`--bot-fixture` dependency configuration. Both application images write four
Bots: active with a retained test result, disabled after credential rotation,
failed after provider refusal and normal cleanup, and deleted with retained
creation replay. The opposite reader must preserve the exact projected state,
pinned profile references, creation fingerprint conflicts, and deleted-record
behavior. Credential/channel effect counters must stay unchanged during reads
and replay. The gateway and key issuer are synthetic; this is not proof of
real bot delivery or every internal BSON variant.

The first attempt correctly failed because its synthetic key used an unsupported
prefix. A regression reproduces that defect and the corrected issuer uses the
credential grammar required by both immutable images. The corrected campaign
then passed old-write, new-read-old-write, new-write, and old-read-new-write:
four Bot records plus three profile samples per stage, 13 collections in this
campaign, and 17 distinct collections across the two preceding successful
campaigns. The Event Worker campaign below brings the selected total to 19. Its
three boots took 4.274422, 2.923593, and 2.903270 seconds, with zero failed
attempts. It exited zero and removed its cluster; 98 raw evidence files are
sealed in `/Users/shining/code/work/tech-lead.staff/developer-19/migration-bot-fixture-20261117-v2/SCOPED-RESULT.json`
(SHA256 `39bb8c62065d5ca972f99f5ecc431e676fb9c9beb2a950459227a374221911de`).
Actual nonsecret ConfigMap readbacks exist for the old and new readers. The
rollback pod's ConfigMap mount is verified from its startup watch, but the
finite data observer arrived after cleanup; its configuration remains source
bound rather than a fresh ConfigMap data readback. The first failed attempt is
retained separately. A later trace-only regression corrects inherited gateway
DELETE labels without changing dependency responses or effect counters; the
successful campaign retains its original frozen tooling hashes.

The Event Worker Profile extension is available as
`migration_event_worker_fixture.py` and has now completed one actual campaign.
It writes through
`/api/v1/event-worker-profiles` and exercises the two persisted collections
`event_worker_profiles` and `event_worker_profile_revisions`: current revision,
immutable revisions 1 through 3, pinned profile references, retirement, active
list exclusion, and creation/update/retirement replay. Its unit regressions
cover both reader directions and reject changed immutable revisions. The
campaign passed all four directions with six samples per stage (three base
profiles plus three Event Worker revisions), three clean boots with zero failed
boots, and preserved all three catalog comparisons. Raw evidence is sealed in
`/Users/shining/code/work/tech-lead.staff/developer-19/migration-event-worker-fixture-20261117-v2/AUDIT.json`
(SHA256 `0d95487e7ba25bea28f4ac293a02a035bfeddcdd669bf8a14e0e0eaa3d101b95`).
The two Event Worker collections are included in the 19-collection selected
result; their additional nested and policy variants remain unverified.

The gateway-record extension (`migration_gateway_fixture.py`, enabled with
`--gateway-fixture`) tests channel and connection namespaces with the same UUIDs
but different opaque bytes, including one-byte and 65,536-byte payloads, owner
transfer, paginated heads, deletion, and rejection of stale updates/deletes.
The synthetic identity grants gateway authority only to its admin principal;
the ordinary owner must still receive 403. Ten offline regressions validate
these assertions; they do not establish application compatibility.

Its first immutable-image campaign stopped on the initial 12d gateway PUT:
HTTP 404 `not_found`, “No REST route matches this request path.” Source checks
on both exact revisions confirm that CMA does not reference either gateway
router constructor, although the retained store adapters and library routes
exist. The 13b reader stage did not run. This is an unavailable application
entry point, not a demonstrated schema incompatibility. Both
`gateway_channel_records` and `gateway_connection_records` remain unverified;
the selected collection total stays 19. The attempt and source hashes are sealed
in `/Users/shining/code/work/tech-lead.staff/developer-19/migration-gateway-fixture-20261117-v1/AVAILABILITY-AUDIT.json`
(SHA256 `18096ec180e8bdfef2d2159c035aa7dc844840d72b20543204a7d64858dbf984`).
The bounded run exited 1 after 102.358 seconds and removed its cluster and
private tmpfs storage. No unchanged retry was made.

The standalone `migration_rate_fixture.py` exercises the shared Mongo input
budget (`--rate-fixture`, one input per minute). It sends input to a nonexistent
Agent and requires a domain 404 before checking exhaustion; a generic missing
route never counts. Each opposite image's first input for that subject must
return 429 before refill, from a different verified pod UID. The 12d writer uses
the ordinary browser subject and the 13b writer uses the separate synthetic
admin subject. No Agent/runtime is allocated and no application row is injected.

The actual campaign passed all four stages: 13b honored the 12d budget after
20.962716 seconds (`Retry-After: 40`), and restored 12d honored the 13b budget after
23.274952 seconds (`Retry-After: 37`). All three complete startup observations
recorded zero failed boots, and every catalog comparison was preserved. The
138.971-second run exited zero and removed its cluster, private storage, and
browser custody. This adds one selected shape in `request_rate_limits`, bringing
the combined total to 20 collections and 12 clean boots. Other rate tiers,
leased credits, expiry and policy variants remain unverified. Audit:
`/Users/shining/code/work/tech-lead.staff/developer-19/migration-rate-fixture-20261117-v1/AUDIT.json`
(SHA256 `09d42732770c946b5f8f72f83ab38e40e388eef1b264dd3177182ec6d0d04272`).
Twelve fault regressions pass with 81% fixture statement coverage; the complete
frozen harness passes 162 Python and 15 Node tests, with zero skipped.

The opt-in `migration_sandbox_fixture.py` (`--sandbox-admission-fixture`) prepares retained unprovisioned Sandbox admission, event, colour and replay checks. Its manifest restricts runtime RBAC to reads and rejects runtime controllers/pools; it cannot establish a Ready Agent or native assignment result.
After Docker recovered, two attempts exposed fixture errors: admission adds exactly two profile references, and usage rows project a per-read `as_of` clock. Corrections preserve all other fields and all persisted usage semantics exactly. Both failures and cleanup receipts remain under `migration-sandbox-admission-20261117-v{1,2}`. Corrected v3 passed all four stages, two manual admissions per writer, and three zero-failure boots in 328.912 seconds; its cluster, helpers, temporary aliases, browser custody and RAM volume were removed. Audit: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-sandbox-admission-20261117-v3/AUDIT.json`, SHA256 `39e71aa3354102784c977e00cf23bb69a3ed5617686e6468208e4db2e42b3620`. The expanded harness passes 182 Python and 15 Node tests, including reproduced baseline failures. Seven Always-on fixture fault tests have 98% statement coverage; eleven manual-admission fault tests have 81% measured coverage.

The successful selected-shape union is now 26 collections with 18 clean boot observations across six campaigns; the separate Always-on failure remains explicit. Coverage binding: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-sharing-kind-20261117-v1/SELECTED-COVERAGE.json`, SHA256 `06275c5aab266bfbc9cc76cfcd1631dcda1dfd7ce3d27cc3b99baa8a3a53a5e1`. A listed collection does not mean all its variants passed.

The campaign captures the entire Mongo collection/index catalog after
each of the four application fixture stages. This covers a concrete startup
delta: 13b adds `sandbox_usage_events_interval` with ordered keys
`sandbox_id, ended_at, started_at, revision`. The collector records collection
UUIDs, options (including validators), and complete index definitions. Existing
definitions must survive upgrade and rollback; newly added definitions must
survive every subsequent stage. Added definitions are listed for review, never
treated as proof that an old reader accepts new documents.

Catalog collection uses a bounded Job with no retries or service-account token,
and a synthetic Mongo role restricted to `listCollections`, `listIndexes`, and
`killCursors`. It drains both catalog cursors, rejects incomplete index builds,
and has no document-read privilege. The runner binds each receipt to the same
Ready application pod before and after capture. Offline regressions exercise
index removal/reordering, TTL/unique/partial/collation changes, collection
recreation, validator changes, pagination failures, and error redaction. In v6,
all 86 instantiated collection catalogs were preserved across all transitions.
The added interval index survived the 12d rollback. This metadata evidence does
not establish document-reader coverage for those 86 collections.

The runner now records a pod-list resource version before each boot, including
before the initial API Deployment is applied. After rollout, a bounded Kubernetes
watch reads retained startup events from that version. It rejects failed or
restarted attempts and pods replaced before Ready, even when the final surviving
pod is healthy. Expired history, missing creation/readiness events, transport
failure, and malformed records cannot produce a clean-boot receipt. The previous
serving pod's intentional retirement is excluded by its pre-boot UID.

Offline regressions demonstrate that the former final-snapshot check accepts a
healthy replacement after a failed attempt, while the new event check rejects
it. V6 replayed the raw startup history for all three application boots and
observed one successful attempt each. Historical final-snapshot receipts still
do not establish the absence of an earlier failed/replaced pod.

The profile fixture also prepares the concurrent-edit part of TC-025. For each
Agent, Workspace, and Sandbox profile, two independent update intents start
from the same draft version. Recorded HTTP intervals must overlap; exactly one
edit must return 200 and the other must return `profile_version_conflict` (409).
Only the winning draft is published. Both completed commands are replayed after
the image switch, followed by a fresh stale-version refusal; the current profile
and original published revision must remain identical. The fixture retains the
request IDs and timing evidence and stops on transport failures or throttling,
without automatic retry. Local HTTP regressions validate this harness, including
deliberately broken double acceptance and replay mutation. V6 passed these
selected profile races and opposite-reader replays on both application images.
This is the declared TC-025 profile subset, not whole-matrix acceptance.

## Combined-candidate execution, 2026-11-17

Campaign v6 ran from 13:59:21 to 14:13:34 SGT (852.944 seconds) with the exact
12d and combined-candidate identities above. Each of `old-write`,
`new-read-old-write`, `new-write`, and `old-read-new-write` passed with seven
application fixture samples (three profiles, three Trigger records, one usage
report), plus browser/settings checks. The union contains 19 declared check/shape
labels in 16 collections; labels are not a count of every nested BSON variant.
Startup observations for 12d, candidate, and 12d recovery were 4.290, 2.845, and
2.906 seconds, each with one observed pod attempt and zero failed boots. This
three-boot sample does not establish a performance percentile or 90-day scale.

Private audited result:
`/Users/shining/code/work/tech-lead.staff/developer-19/migration-resume-cd91-20261117-v6/SCOPED-RESULT.json`
SHA256 `d4633132b19d1c32510a97e7fdc19895c2ae7f54db2cb8d2f1eec5a865b693c7`.
Raw evidence SHA256
`3d3b6b8b28d5862dd11b4e5fa577462a4e58fcd2d7bb583a75994b8d0737e58f`.
The audit independently replayed raw boot events and catalog comparisons,
verified all frozen inputs, and sealed 96 evidence files. Supervisor exit was 0;
its roots, owned cluster, tmpfs holder/volume, temporary image aliases, and
browser custody file are absent. Linux/amd64 application images ran on the local
arm64 Docker host. No production or native lifecycle calls were made.

The covered collections are `activity`, the six Agent/Workspace/Sandbox profile
and revision collections, `auth_browser_logins`, `auth_login_attempts`,
`auth_subjects`, `operation_claims`, `platform_settings`, `sandbox_usage_reports`,
`sandbox_usage_report_control`, `trigger_definitions`, and
`trigger_definition_revisions`. Full collection/shape directions in the table
remain **UNVERIFIED** even for these collections because the fixture does not
exercise every persisted variant.

## Source-derived collection inventory

The inventory is generated from the immutable source registries (`store::isolation::COLLECTIONS` and `store::isolation::replicas::COLLECTIONS`) plus explicitly retained adapter namespaces. It proves collection-name coverage only; nested BSON/document-shape coverage requires execution against each pinned image and therefore remains unverified.

The exact-source reference ledger distinguishes 51 names with only registry/census/test literal references from 109 with implementation candidates. It retains every source location and file hash: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-writer-scope-20261117/SOURCE-REFERENCES.json`, SHA256 `c3ac35ce76ee9e7f0148e50a07a91769923d6d2b31456eb3b1c01b140cac487e`. This is discovery, not a call graph: dynamic names and retained formats remain to be traced. In particular, the retirement census `Empty` policy refuses any rows; it does not prove that a namespace is empty or safe to restore. No collection is removed and no compatibility pass is inferred.

| Collection | Scope | 12d declaration | Combined 13b declaration | 12d writes → 13b reads (all shapes) | 13b writes → 12d reads (all shapes) |
| --- | --- | --- | --- | --- | --- |
| `activity` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_access_requests` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_access_tools` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_artifacts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_capacity_recovery_work` | runtime | `api/crates/cma/src/store/agent_capacity_recovery/records.rs` (`f0a48911ba42…`) | `api/crates/cma/src/store/agent_capacity_recovery/records.rs` (`f0a48911ba42…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_channel_usage` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_command_orders` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_commands` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_creation_provenance` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_lifecycle_operations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_admissions` | runtime | `api/crates/cma/src/store/agent_log_segments/writer_admission.rs` (`e6b7e1f28936…`) | `api/crates/cma/src/store/agent_log_segments/writer_admission.rs` (`e6b7e1f28936…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_audits` | runtime | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_evidence` | runtime | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_gates` | runtime | `api/crates/cma/src/store/agent_log_segments/writer_gate.rs` (`0c4797e35f7d…`) | `api/crates/cma/src/store/agent_log_segments/writer_gate.rs` (`0c4797e35f7d…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_heads` | runtime | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_projections` | runtime | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_publications` | runtime | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_log_segments` | runtime | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | `api/crates/cma/src/store/agent_log_segments/mod.rs` (`7b75408ef800…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_manager_migrations` | separate-or-retained | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_message_feedback` | runtime | `api/crates/cma/src/store/message_feedback.rs` (`a8a6d55825db…`) | `api/crates/cma/src/store/message_feedback.rs` (`a8a6d55825db…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_profile_revisions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_profiles` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_provenance_progress` | separate-or-retained | `api/crates/cma/src/store/provenance_progress/model.rs` (`77cedee3a72f…`) | `api/crates/cma/src/store/provenance_progress/model.rs` (`77cedee3a72f…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_reuse_bindings` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agent_suggestions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `agents` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `auth_actor_links` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `auth_api_bearers` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `auth_browser_logins` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `auth_login_attempts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `auth_nyxid_proxy_grants` | runtime | `api/crates/cma/src/store/auth.rs` (`051acd97bfbe…`) | `api/crates/cma/src/store/auth.rs` (`051acd97bfbe…`) | **UNVERIFIED** | **UNVERIFIED** |
| `auth_subjects` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `binding_conversations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `binding_event_media_uploads` | runtime | `api/crates/cma/src/store/media_uploads.rs` (`ebadecb29aa3…`) | `api/crates/cma/src/store/media_uploads.rs` (`ebadecb29aa3…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bot_setup_contexts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bot_setups` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bots` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_allocation_heads` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_allocations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_callback_dispositions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_callback_inbox` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_callback_platform_messages` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_callback_rates` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_callback_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_callback_replies` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_channel_capabilities` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_channel_control_aliases` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_channel_control_intents` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_channel_control_references` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_credentials` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_definition_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_definitions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_invoke_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_invoke_grants` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_invoke_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_heads` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_invocations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_items` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_recoveries` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_scans` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_outbox_send_rates` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_partition_command_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_refused_control_aliases` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_refused_control_intents` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_source_control_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_source_control_policies` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_source_control_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `bridge_text_attachments` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_child_cleanup_job_admissions_v1` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_child_cleanup_jobs_v1` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_command_revival_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_credential_renewal_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_history_references` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_lifecycle_credential_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_mint_orphan_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_mint_orphans` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_provider_dispatch_journals` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_provider_finality_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_provider_teardown_receipts` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_retirement_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_retirement_finalizations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_reuse_cleanup_admission_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_reuse_cleanup_admissions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_reuse_cleanup_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_reuse_cleanup_removal_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_reuse_cleanup_requests` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_reuse_cleanup_tombstones` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `channel_usage_events` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `checkpoint_archive_gates` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `checkpoint_archive_tombstones` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `checkpoint_objects` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `checkpoints` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `cma_agent_manager_members` | runtime | `api/crates/cma/src/store/agent_ownership/mod.rs` (`c29ed9ea0eae…`) | `api/crates/cma/src/store/agent_ownership/mod.rs` (`46701d22800d…`) | **UNVERIFIED** | **UNVERIFIED** |
| `cma_agent_manager_owners` | runtime | `api/crates/cma/src/store/agent_ownership/mod.rs` (`c29ed9ea0eae…`) | `api/crates/cma/src/store/agent_ownership/mod.rs` (`46701d22800d…`) | **UNVERIFIED** | **UNVERIFIED** |
| `cma_document_artifacts` | runtime | `api/crates/cma/src/document_artifacts/mongo.rs` (`2ce9810c1d61…`) | `api/crates/cma/src/document_artifacts/mongo.rs` (`2ce9810c1d61…`) | **UNVERIFIED** | **UNVERIFIED** |
| `cma_document_quotas` | runtime | `api/crates/cma/src/document_artifacts/quota.rs` (`0af8defbf57b…`) | `api/crates/cma/src/document_artifacts/quota.rs` (`0af8defbf57b…`) | **UNVERIFIED** | **UNVERIFIED** |
| `codex_subscription_connections` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `command_authorization_audits` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `event_worker_profile_revisions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `event_worker_profiles` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `gateway_channel_records` | separate-or-retained | `api/crates/cma/src/store/gateway_records.rs` (`8653a15751b9…`) | `api/crates/cma/src/store/gateway_records.rs` (`8653a15751b9…`) | **UNVERIFIED** | **UNVERIFIED** |
| `gateway_connection_records` | separate-or-retained | `api/crates/cma/src/store/gateway_records.rs` (`8653a15751b9…`) | `api/crates/cma/src/store/gateway_records.rs` (`8653a15751b9…`) | **UNVERIFIED** | **UNVERIFIED** |
| `gateway_interaction_records` | separate-or-retained | `api/crates/cma/src/store/gateway_interactions.rs` (`d09b925c82a0…`) | `api/crates/cma/src/store/gateway_interactions.rs` (`d09b925c82a0…`) | **UNVERIFIED** | **UNVERIFIED** |
| `model_budget_reservations` | runtime | `api/crates/cma/src/store/budget/mod.rs` (`dcc3d8ad19a5…`) | `api/crates/cma/src/store/budget/mod.rs` (`dcc3d8ad19a5…`) | **UNVERIFIED** | **UNVERIFIED** |
| `native_prepared_sandboxes` | runtime | `api/crates/cma/src/store/native_prepared.rs` (`cdbc96c76b47…`) | `api/crates/cma/src/store/native_prepared.rs` (`5c2304036c3e…`) | **UNVERIFIED** | **UNVERIFIED** |
| `native_warm_pool_targets` | runtime | `api/crates/cma/src/store/native_warm_pools.rs` (`62caa0b8dcd0…`) | `api/crates/cma/src/store/native_warm_pools.rs` (`62caa0b8dcd0…`) | **UNVERIFIED** | **UNVERIFIED** |
| `operation_claims` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `owned_workspace_cleanup_audits_v1` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `owned_workspace_cleanup_consents_v1` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `owned_workspace_cleanup_jobs_v1` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `owned_workspace_cleanup_tombstones_v1` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `platform_settings` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `provider_binding_preparations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `provider_bindings` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `provider_tool_opens` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `provider_tool_relays` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `request_rate_limits` | runtime | `api/crates/cma/src/store/request_limits.rs` (`c85d25710e25…`) | `api/crates/cma/src/store/request_limits.rs` (`c85d25710e25…`) | **UNVERIFIED** | **UNVERIFIED** |
| `resource_policies` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `resource_share_commands` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `resource_shares` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `runtime_backend_routes` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_admissions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_billing_state` | runtime | `api/crates/cma/src/store/sandbox_mongo.rs` (`acf5dbc4e951…`) | `api/crates/cma/src/store/sandbox_mongo.rs` (`9fc17d453b5c…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_colors` | runtime | `api/crates/cma/src/store/sandbox_colors.rs` (`917d841d48ea…`) | `api/crates/cma/src/store/sandbox_colors.rs` (`917d841d48ea…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_controls` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_credential_issuance` | runtime | `api/crates/cma/src/store/sandbox_credentials/issuance.rs` (`176a6287311c…`) | `api/crates/cma/src/store/sandbox_credentials/issuance.rs` (`176a6287311c…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_credential_revocations` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_credentials` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_events` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_metric_bindings` | runtime | `api/crates/cma/src/store/sandbox_metric_bindings/mod.rs` (`8707766a488b…`) | `api/crates/cma/src/store/sandbox_metric_bindings/mod.rs` (`8707766a488b…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_metric_rollups` | runtime | `api/crates/cma/src/store/sandbox_metric_rollups/mod.rs` (`1fc44b190219…`) | `api/crates/cma/src/store/sandbox_metric_rollups/mod.rs` (`1fc44b190219…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_profile_revisions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_profiles` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_retained_availability` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_sessions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage` | runtime | `api/crates/cma/src/store/sandbox_usage.rs` (`59e7178ad841…`) | `api/crates/cma/src/store/sandbox_usage.rs` (`e63d187f2183…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_allocation_buckets_v1` | runtime | `api/crates/cma/src/store/allocation_buckets/mod.rs` (`04c7359499b4…`) | `api/crates/cma/src/store/allocation_buckets/mod.rs` (`04c7359499b4…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_events` | runtime | `api/crates/cma/src/store/sandbox_usage.rs` (`59e7178ad841…`) | `api/crates/cma/src/store/sandbox_usage.rs` (`e63d187f2183…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_report_charts` | runtime | `api/crates/cma/src/store/usage_reports/mod.rs` (`fa3bbb5b6ce0…`) | `api/crates/cma/src/store/usage_reports/mod.rs` (`8fa86d266996…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_report_control` | runtime | `api/crates/cma/src/store/usage_reports/mod.rs` (`fa3bbb5b6ce0…`) | `api/crates/cma/src/store/usage_reports/mod.rs` (`8fa86d266996…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_report_rows` | runtime | `api/crates/cma/src/store/usage_reports/mod.rs` (`fa3bbb5b6ce0…`) | `api/crates/cma/src/store/usage_reports/mod.rs` (`8fa86d266996…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_reports` | runtime | `api/crates/cma/src/store/usage_reports/mod.rs` (`fa3bbb5b6ce0…`) | `api/crates/cma/src/store/usage_reports/mod.rs` (`8fa86d266996…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandbox_usage_rollups` | runtime | `api/crates/cma/src/store/sandbox_usage_rollups/mod.rs` (`01423860dc76…`) | `api/crates/cma/src/store/sandbox_usage_rollups/mod.rs` (`9573eda4d091…`) | **UNVERIFIED** | **UNVERIFIED** |
| `sandboxes` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `service_connection_handoffs` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `trigger_definition_revisions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `trigger_definitions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `trigger_first_message_bindings` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `trigger_launch_events` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `trigger_launch_input_protocols` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `trigger_launches` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `users` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `visitor_launch_days` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `warm_sandboxes` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `workspace_profile_revisions` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `workspace_profiles` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `workspaces` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |
| `worktrees` | runtime | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | `api/crates/cma/src/store/isolation.rs` (`2bc6eafb92a1…`) | **UNVERIFIED** | **UNVERIFIED** |

## Source-derived durable shape strata

Collection names do not capture nested document contracts. The following strata are the reviewed source families that must be represented by fixtures. Their status remains **UNVERIFIED** until the exact 12d and 13b executables write and read them in both directions.

| Shape stratum | Collections covered | Source contract evidence | 12d writes → 13b reads | 13b writes → 12d reads |
| --- | --- | --- | --- | --- |
| Agent and environment lifecycle envelopes, including retained admission identity and work authority versions 1/3/4 | `agents`, `sandboxes`, `workspaces`, `worktrees`, `sandbox_admissions`, `sandbox_events`, `sandbox_sessions`, `warm_sandboxes`, `native_prepared_sandboxes`, `sandbox_credentials` | `store/schema.rs`, `store/work_schema.rs`, `layer2-core/src/sandbox/store/sandbox_admission.rs`, `store/sandbox_mongo/collections.rs` | **UNVERIFIED** | **UNVERIFIED** |
| Manual unprovisioned Sandbox identity, admission replay, append-only events and colour assignment | `sandboxes`, `sandbox_events`, `sandbox_colors` | `migration_sandbox_fixture.py`; v3 audit above | **PASS for declared shapes** | **PASS for declared shapes** |
| Zero-capacity warm-pool targets, system default, retained command replay and revision fences | `native_warm_pool_targets` | `migration_warm_pool_fixture.py`; v3 raw HTTP and startup replay audit below | **PASS for declared shapes** | **PASS for declared shapes** |
| Sandbox organization grants, mode revisions, retained revocations and command replay | `resource_policies`, `resource_shares`, `resource_share_commands` | `migration_share_fixture.py`; sharing audit below | **PASS for declared shapes** | **PASS for declared shapes** |
| Lifecycle Sandbox admission, originally unset Always-on policy changed to true by the candidate | `sandboxes`, `agent_lifecycle_operations` | `migration_admission_policy_fixture.py`; exact artifact audit below | **PASS for this original-policy case** | **FAIL: 12d startup rejects retained Sandbox** |
| Agent command, queued-input, cancellation, ownership and operation receipts with revision/idempotency fences | `agent_commands`, `agent_command_orders`, `command_authorization_audits`, `agent_lifecycle_operations`, `operation_claims`, `resource_share_commands` | `store/agent_commands/records.rs`, `store/agent_commands/queued_input.rs`, `store/agent_lifecycle_operations.rs`, `store/resource_access/share_links.rs` | **UNVERIFIED** | **UNVERIFIED** |
| Segmented transcript/log envelopes and publication projections, including writer gates and recovery evidence | `agent_log`, `agent_log_heads`, `agent_log_segments`, `agent_log_publications`, `agent_log_projections`, `agent_log_evidence`, `agent_log_audits`, `agent_log_gates`, `agent_log_admissions`, `agent_capacity_recovery_work` | `store/agent_log_segments/{envelope,codec,schema,publication,projection}.rs`, `store/agent_log.rs` | **UNVERIFIED** | **UNVERIFIED** |
| Usage events, state, rollups, report jobs/rows/charts/control, allocation buckets and metric bindings | `sandbox_usage`, `sandbox_usage_events`, `sandbox_usage_rollups`, `sandbox_usage_allocation_buckets_v1`, `sandbox_usage_reports`, `sandbox_usage_report_rows`, `sandbox_usage_report_charts`, `sandbox_usage_report_control`, `sandbox_metric_bindings`, `sandbox_metric_rollups` | `store/sandbox_usage.rs`, `store/sandbox_usage_rollups/`, `store/usage_reports/{schema,views,on_demand_views}.rs`, `store/allocation_buckets/` | **UNVERIFIED** | **UNVERIFIED** |
| Per-AM membership, ownership leases, epochs and fencing tokens | `cma_agent_manager_members`, `cma_agent_manager_owners` | `store/agent_ownership/{records,acquisition,membership}.rs`, `layer2-core/src/agent_ownership/` | **UNVERIFIED** | **UNVERIFIED** |
| Auth, encrypted custody, provider bindings and subscription records | `auth_api_bearers`, `auth_actor_links`, `auth_browser_logins`, `auth_login_attempts`, `auth_subjects`, `auth_nyxid_proxy_grants`, `provider_bindings`, `provider_binding_preparations`, `codex_subscription_connections`, `sandbox_credentials`, `sandbox_credential_revocations` | `store/auth/`, `auth/`, `store/sandbox_credentials/`, `store/codex_subscriptions.rs`, `store/schema.rs` | **UNVERIFIED** | **UNVERIFIED** |
| Profiles, revisions, users, settings, policies, shares and access requests/tools | `agent_profiles`, `agent_profile_revisions`, `sandbox_profiles`, `sandbox_profile_revisions`, `workspace_profiles`, `workspace_profile_revisions`, `users`, `platform_settings`, `resource_policies`, `resource_shares`, `agent_access_tools`, `agent_access_requests` | `store/agent_profiles.rs`, `store/profiles/`, `store/users.rs`, `store/settings.rs`, `store/resource_access/`, `store/agent_access_*` | **UNVERIFIED** | **UNVERIFIED** |
| Document/artifact blobs, quota and media-upload metadata | `cma_document_artifacts`, `cma_document_quotas`, `agent_artifacts`, `binding_event_media_uploads`, `checkpoint_objects`, `checkpoints`, `checkpoint_archive_gates`, `checkpoint_archive_tombstones` | `document_artifacts/`, `store/agent_artifacts.rs`, `store/media_uploads.rs`, `store/checkpoints.rs`, `store/retirement_archive/` | **UNVERIFIED** | **UNVERIFIED** |
| Activity, cleanup, trigger, bot and bridge/gateway durable receipts | `activity`, `owned_workspace_cleanup_*`, `trigger_*`, `bot_setups`, `bot_setup_contexts`, `bots`, `gateway_channel_records`, `gateway_connection_records`, `gateway_interaction_records`, `bridge_*`, `channel_*` | `store/activity.rs`, `store/owned_workspace_cleanup/`, `store/trigger_definitions/`, `store/bot_*`, `store/gateway_records.rs`, `store/gateway_interactions.rs` | **UNVERIFIED** | **UNVERIFIED** |

The retained Always-on admission cell has an **actual immutable-image rollback failure**. Campaign `migration-admission-policy-20261117-v2` admitted Lifecycle parents through the public Agent API with runtime mutations denied. The first three stages passed: 12d wrote, 13b read, and 13b changed the new parent's policy from unset to true through the admin API and read it back. Restored 12d never became Ready; its 180-second rollout wait failed, four restarts were observed, and the application exited with `invalid retained Sandbox`. Both initial boots succeeded. The rollback fixture could not execute because startup failed. The 388.296-second campaign exited 1 and removed its cluster, helpers, temporary image aliases, browser custody and RAM volume. Audit: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-admission-policy-20261117-v2/AUDIT.json`, SHA256 `d4c224d0885ce744a3fa125727f8eb8fe9d8632756117ec0c579c9002e18450c`.

The startup path is `MongoRuntimeRouting::open` → `read_sandbox` → `read_admission_replay_metadata` → `SandboxAdmissionReplay::new`. The 12d reader recomputes the admission fingerprint from the current policy; 13b preserves the original digest and reads it through `from_retained`. Thus the failure occurs before traffic, not merely on a later get/list call. Historical M5 native evidence additionally covers both Lifecycle and Trigger origins and all three original policy states: six expected baseline failures and six corrected passes, with thirteen relevant files byte-identical to the selected sources. Binding: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-always-on-rollback-20261117/SOURCE-RISK.json`, SHA256 `223186fd7123a8a3ffe3586f7a8a70f6134c4bbd15d764bf94aff64c676bf252`. Only the Lifecycle unset→true case has exact-image failure evidence. A recovery reader must carry the admission-policy fix; no Mongo repair, policy reset, or historical rewrite is proposed.

## Required execution cells

Only the explicitly reported HTTP shapes have passed. The following checks are required for every listed collection and each reviewed document shape before this file can become a gate result:

- write with 12d, then decode/replay/restart/take over with 13b while preserving IDs, revisions, leases, receipts, indexes, and semantic values;
- write with 13b, then decode/replay/restart/take over with 12d;
- exercise retained, disabled, and enabled writer policies, including refusal before traffic and before writes for unsupported shapes;
- boot each reader with the retained database and record zero failed boots;
- after 13b writes, boot and serve with the exact 12d image as the undo target; record any refusal or semantic difference;
- retain one raw receipt per cell with the exact image digest, source SHA, configuration hash, timestamps, sample count, and cleanup result.

## Executed evidence (partial, not a gate result)

Campaign v21 (`/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/run-9217-v21/kind-evidence.private.json`, SHA256 `fd1c24050b9b9e8c6164e4d27f05f87e09931c298ea1aa550dd7c8eb5a6a539b`) repeats the isolated campaign with a ConfigMap-only synthetic provider. It uses the pinned linux/amd64 Python dependency `docker.io/library/python@sha256:48d884813cf44e927cf64c5529dd630dd3b18338c6dc727991eef9b71e934a20`; the provider image is validated before kind load and the identity Deployment is waited to Available. The fixture performs one real PKCE browser login on the 12d writer, then carries the browser session cookie through the 13b switch and 12d rollback, checking `/auth/session` after each switch for the same owner subject. All three boots and all four profile stages passed with zero API restarts; cleanup recorded `cluster_absent: true`. This extends the executable subset to `auth_browser_logins`, `auth_login_attempts`, and `auth_subjects` session continuity. It remains a partial result: it does not certify the remaining collections, encrypted token variants, or all policy/read-write shapes.

Campaign v23 supersedes v21's auth-strength claim (`/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/run-9217-v23/kind-evidence.private.json`, SHA256 `96c08880456d30f2c3539d19df4ebb3ae2bfb5e547403cc8cf57bf9cb6cb58ff`). In each writer stage, the fixture completes a real PKCE callback and creates a durable browser session, then leaves a separate pending login attempt. The opposite reader checks the retained session subject and consumes the pending callback exactly once; the rollback reader does the same after the 13b writer. The four stages passed for the six profile collections plus `auth_browser_logins`, `auth_login_attempts`, and `auth_subjects`; all three boots had zero API restarts and cleanup recorded `cluster_absent: true`. Cookie values remain in a mode-0600 ephemeral file inside the private run directory and are removed by runner cleanup. This is still a subset result and does not certify `auth_api_bearers`, actor links, proxy grants, encrypted token variants, or the remaining application stores.

Campaign `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/run-9217-v16/kind-evidence.private.json` (SHA256 `3e241c3034958a3cc609e77e6787eac7b569516e2d8ecb6e04b95b3dfad4545a`) used one disposable kind cluster and a pinned Mongo image. The runner required an exact Ready API pod for each selected image/config digest, port-forwarded fixtures directly to that pod, checked that the pod identity stayed stable throughout every fixture, and recorded cleanup as `cluster_absent: true`. Historical boot, current boot, and rollback boot all reached Ready with zero restarts. The earlier v14 receipt remains at `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/run-9217-v14/kind-evidence.private.json` (SHA256 `38854b7cac0c63323832ca9d7848d5be3d1b9355834970be583bc02ff868a093`) as historical evidence only.

The application fixture wrote and read six profile collections (`agent_profiles`, `agent_profile_revisions`, `workspace_profiles`, `workspace_profile_revisions`, `sandbox_profiles`, `sandbox_profile_revisions`) with three nested shape classes through the actual AM routes. All four directional stages passed: `old-write`, `new-read-old-write`, `new-write`, and `old-read-new-write`; each stage recorded three application samples. This proves the profile route subset and exact rollback readback. It does not turn collection-name inventory into application-level coverage.

An attempted hierarchy extension is preserved as a separate blocked receipt at `/Users/shining/code/work/tech-lead.staff/developer-19/migration-matrix-evidence-20261116/run-9217-v17/kind-evidence.private.json` (SHA256 `a970c3c96cb68be0ac19aa1434eaea71f83356b399c2341478ead673c590f223`). The disposable trusted-mode manifest has no identity/NyxID credential provider, so the real `POST /api/v1/sandboxes` route returned `503 hierarchy_service_unavailable` with `identity_unavailable` before it could write a hierarchy record. The campaign cleaned its kind cluster (`cluster_absent: true`); no hierarchy receipt is counted.

The campaign does not prove all 160 collections, every nested BSON codec, retained/disabled/enabled policy behavior, lifecycle replay, leases, indexes, or worker takeover. The earlier direct-Mongo fixture was discarded as insufficient application evidence. The overall campaign therefore remains **UNVERIFIED** until the remaining application readers and writer policies have executable receipts.

Native setup attempts v24-v28 are retained as private blocked receipts and are not counted as migration cells: v24 `a992d78e39af87eda0700039189fbf92605ac83e080dc84f76bf4d18e5778c94`, v25 `eb5e3b8f767fdfa34f33f80c5c2326b4c4799ed7b0d93bb970efaabced0fa488`, v26 `895bef96de30fa092131bc0008fc4181d7ef215e958b40884949d410f319d900`, v27 `116646c4b1116e3a6b126844ee2c21c28d5b9f9929ef5fc8f681f70ec0a04595`, and v28 `f0eb9f6da42a9f6b85d31f18b3a6d2708b305efb92737dc90ab8274ad7d36ad4`. Each owned cluster was removed; none produced a valid lifecycle receipt.

## Blocker and next action

The zero-capacity warm-pool fixture first omitted the startup system default; the corrected fixture binds its exact actor/key hash, fingerprint, resources, revision and zero target. The cancelled v2 run remains preserved and contributes no passes: capacity-release receipt SHA256 `a4610876c962720c7e2a446d97042761d438de7751cb5f8fd0b0aaac7b11b8f3` under `migration-capacity-release-20261117/stop-20261117T114536Z/` in developer-19's private directory.
The separate v3 campaign completed all four stages on the pinned images in 294.920 seconds. Each writer created one zero-capacity target and resized it once to revision 2; the opposite reader preserved its plan, original command replay, changed-input/duplicate-plan refusals and stale-revision fence. The system target also remained unchanged. Offline audit replayed all 39 pool HTTP requests, reconstructed the writer receipts, verified the 2-second reconciliation polling interval and replayed all 15 startup watch events across three zero-failure boots. Catalogs were preserved, with only the reviewed interval-index addition. Audit: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-warm-pool-20261117-v3/AUDIT.json`, SHA256 `8b4846331086f9640cabf3e5b4c81819c1cf267ef0e9de0d54ef6a922ced6f45`. This adds selected `native_warm_pool_targets` shapes; positive capacity, native UIDs, retirement, lease takeover and unknown commits remain unverified.
The v3 runner exited zero and removed its cluster, helpers, RAM volume, image aliases and browser custody. Independent finality froze 528 files and found no owned resources or processes: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-capacity-release-20261117/finality-20261117T121415Z/CAPACITY-RELEASE.json`, SHA256 `5098cfdeb742c0f0396f458e4628300f3ead2823dd147845e58341237e59d809`. No migration kind workload is running. The cancellation regressions validate harness behavior only; the completed campaign supplies the artifact evidence.

After the revision-234 rehearsal reached terminal cleanup, the bounded `--share-fixture` campaign passed all four stages on the pinned images in 456.313 seconds. Each writer retained two Sandbox organization grants: an updated active grant and a revoked grant, with four immutable command receipts. The audit replayed all 88 sharing HTTP checks, including stale-revision and changed-input rejection, without querying the deleted database. Three complete boots had zero failures; catalogs were preserved, including the reviewed interval-index addition. Cluster, helpers, RAM volume, image aliases and browser custody were removed. This adds selected shapes in `resource_policies`, `resource_shares`, and `resource_share_commands`; Agent links, recipient access and unknown-commit injection remain unverified. Audit: `/Users/shining/code/work/tech-lead.staff/developer-19/migration-sharing-kind-20261117-v1/AUDIT.json`, SHA256 `39c741efdf6e53041e1d821341df045a02e7d6709de2f83e6b03c1f1c1c30cdf`. Frozen harness: 198 Python tests and 15 Node contracts passed, zero skipped; sharing fixture/provider executable-line coverage was 99.31%/84.62%.
The exact images demonstrate a failed 12d rollback after a candidate Always-on write, alongside partial passes in 27 collections. M-OPS-02 cannot certify 12d as the undo floor. A reader containing the admission-policy correction needs separate qualification; the remaining 133 collections and additional shapes/policies also remain unverified. The provider-auth prerequisite was resolved in v23. Native assignment still calls Kubernetes `pods/exec` in `layer3-adapter/src/agent_sandbox/assignment.rs::deliver_assignment`, which task policy prohibits; no Ready lifecycle is claimed. The admission-only fixtures deny runtime mutations. Retained gateway adapters additionally lack mounted CMA reader/writer routes in both selected sources. Full replay/takeover, index/lease preservation and writer-policy coverage remain required. Collection inventory and build metadata cannot substitute for those cells.

`.github/scripts/migration_plan.py` expands the 160-row inventory into 480 policy fixtures and 1,920 reader cells. `.github/scripts/migration_kind_runner.py` creates the isolated cluster, boots both readers against one synthetic replica-set Mongo, preserves Mongo while switching readers, and records the rollback boot.

## Generator and regression tests

- `.github/scripts/migration_inventory.py` derives the inventory from both immutable source revisions and fails closed on unknown registry syntax.
- `.github/scripts/test_migration_inventory.py` verifies complete collection discovery, immutable identity, and fail-closed behavior.
- `.github/scripts/migration_plan.py` expands each discovered collection into retained, disabled, and enabled writer fixtures in both directions; it never infers a refusal from a policy label.
- `.github/scripts/migration_kind_runner.py` is the image-backed executor. It requires a plan and a fixture command that emits collection/shape coverage receipts, verifies immutable local image labels and digests, uses a synthetic ConfigMap-only Mongo authority, and preserves that database across the 12d → 13b → 12d reader switches.
- `.github/scripts/migration_trigger_fixture.py` exercises Trigger authoring, revisions, placements, public previews, and completed operation replay without launching a runtime; `.github/scripts/test_migration_trigger_fixture.py` covers value preservation, replay mutation guards, stale revisions, writer selection, pacing, and fatal rate-limit handling.
- `.github/scripts/migration_usage_fixture.py` exercises usage-report admission/control through the public admin API; `.github/scripts/migration_composite_fixture.py` composes profile, Trigger, and usage fixtures. Unit and local HTTP tests cover minute-scoped coalescing, saved-ID reads, pending worker transitions, immutable ready snapshots, malformed responses, and failure without retries. Empty endpoint responses do not claim persisted row/chart coverage.
- `.github/scripts/test_migration_plan.py` and `.github/scripts/test_migration_kind_runner.py` cover plan completeness, Artifact Registry identities, receipt contracts, secret-free manifests, and fail-closed preflight behavior.
- Existing `.github/scripts/migration-matrix.mjs` remains the evidence-envelope validator; its contract tests do not execute service images.
