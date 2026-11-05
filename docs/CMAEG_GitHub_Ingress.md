# Native GitHub receipt and routed admission

**Superseded onboarding architecture, 2026-11-04.** The owner requires GitHub App registration, webhook secret custody/rotation, installation verification and source activation to be native dynamic CMAEG features. See [the replacement design](CMAEG_GitHub_Integration_Design.md). No GitHub-specific ConfigMap, Kubernetes Secret, volume or per-channel environment edits are allowed. The earlier manifest additions have been withdrawn. Main-chat slices 1–3 implement persisted App management, administrator App-identity verification, webhook rotation and signed ping/receipt recovery; no deployment has been performed. Durable installation approval, automatic reconciliation and dynamic routing remain pending. The static configuration and proof flow below are historical, not a production fallback; use [the current acceptance checklist](CMAEG_GitHub_Online_Verification.md).

This document records the earlier local implementation for code investigation, not an approved deployment recipe. The [online verification plan](CMAEG_GitHub_Online_Verification.md) must wait for the dynamic management contract. Statements about static App routes and attestations below describe code that still needs replacement.

The owner clarified the release workflow on 2026-11-04: schema/data changes belong to feature code and execute at service startup when the feature flag is enabled. The explicit migration command mentioned below is not a manual task for Shining's normal rollout. The local implementation now uses the false-default `CMAEG_GITHUB_ENABLED` startup flag to initialize and verify GitHub storage automatically, retaining data when disabled and refusing unknown versions. The startup adaptation is not yet deployed; see the current operations and acceptance documents for its authority and recovery requirements.

The one-hour attestation bound below is historical code behavior, not the revised integration contract. Tenant bindings persist after initial approval; automatic GitHub eligibility checks and lifecycle handling maintain current access. The replacement requires a separately protected App verification credential and automatic recovery, not recurring manual approval or Kubernetes configuration.

Status: native receipt, persistent execution group, 30-second coalescing,
background routed admission, and machine-readable task result projection are
implemented locally, default off and not deployed. This is the current B/C
slice in [the approved design](CMAEG_GitHub_Source_Tech_Design.md).
It starts existing routed channel execution only when an explicit App route and
typed GitHub channel source match. `accepted` means the delivery and its group
membership are durable; `task_ids` contains a stable execution-group ID rather
than an immediate provider admission ID. The task query reports `queued` while
the group window is open, `running` after admission, `recovering` when a batch
launch or query is uncertain, and the existing terminal activity states when
available. A completed chrono-goals response can additionally project
`succeeded` or `no_change` with its branch, commit and pull request fields. If
the provider snapshot is unavailable or does not contain the result contract,
CMAEG keeps the provider terminal state and leaves `result` absent. The routed
scheduler serializes work sharing the same
`record_id` and consumer target while retaining parallelism across other
targets. A route may explicitly exclude pull request head ref prefixes such as
`automation/`; matching deliveries remain queryable as `ignored` and do not
resolve or launch a channel.

## Public contract

Paths are relative to the Event Gateway public base URL.

| Operation | Behavior |
| --- | --- |
| `POST /v1/github/installation-bindings` | Authenticated owner presents `app_ref`, numeric `installation_id`, and `proof_ref`. A matching operator attestation must exist in server configuration. Returns 201 once, 200 for the owner's existing binding. Replay never refreshes or restores authority. |
| `GET /v1/github/installation-bindings/{binding_id}` | Returns the caller's binding, verified account/repository IDs, status, authority revision, expiry and operator audit reference. Other owners receive 404. |
| `POST /v1/github/apps/{app_ref}/webhooks` | Verifies `X-Hub-Signature-256` over exact bytes. Requires single delivery/event/signature headers. Returns 202 for newly retained supported work, 200 for ping, control/unsupported events or an exact duplicate. |
| `GET /v1/github/deliveries/{receipt_id}` | Owner reads identity, event/action, repository ID, body digest, authority revision and raw-body expiry; never the body or secrets. |
| `GET /v1/github/apps/{app_ref}/deliveries/{delivery_id}` | Owner recovers the receipt using the original delivery ID when the first HTTP response was lost. |
| `GET /v1/github/tasks/{task_id}` | Owner reads a durable execution-group task's channel, current batch event, execution status, the CMA `response_id` when admission has produced one, an optional machine-readable `result`, and the retention boundary. It does not expose provider credentials or model output. |

The URL App reference selects a configured secret. Payload fields cannot select
a secret, CMA owner, Agent, target repository, or credential. Installation identity
is `(github.com, configured app_id, installation_id)`; one installation has one
owner. Fork PRs use the top-level installed repository, not `head.repo`.

Only uncompressed UTF-8 `application/json` is accepted. Body size is limited to 256 KiB. Reading and durable admission share a five-second
budget; execution is never awaited. Duplicate identity is
`(github.com, app_id, X-GitHub-Delivery)`; raw digest and event header must match.
GitHub's HMAC does not authenticate delivery/event headers, so supported schemas
also reject conflicting issue/PR/comment/review shapes. This is delivery
deduplication, not end-to-end exactly-once processing.

Business events are `issues` and `pull_request`, with explicit recognized actions.
`installation`, `installation_repositories` and `repository` lifecycle events
update authority and are retained as `ignored`; they never create business
tasks. Unknown actions and other event families with a verified bound repository
are also retained as `ignored`. Sender type does not suppress bots. There are no
chrono-goals fields or organization-discovery rules in the implementation.

## Historical file-backed installation binding — replacement required

This uses the approved internal-pilot alternative. A platform operator independently
verifies installation/account, organization administrator authority and repository
IDs, then supplies a short-lived attestation through a mounted platform secret.
An authenticated owner subsequently presents its proof reference. An installation
ID alone never establishes ownership. This is not a self-service GitHub OAuth flow.

Configuration is JSON with `apps` (at most 32 entries):

| Field | Meaning |
| --- | --- |
| `app_ref`, `app_id` | Unique URL reference and positive GitHub App ID. |
| `operator_owner` | CMA subject responsible for attestations and App ping receipts. |
| `webhook_secret` | Platform webhook secret, never an Agent credential or API response. |
| `attestations[].proof_ref`, `owner` | Unique proof reference within the App and exact CMA owner subject. |
| `attestations[].installation_id`, `account_id` | Independently verified positive GitHub IDs. |
| `attestations[].repository_ids` | Verified repository snapshot, at most 1,000 IDs; not an Agent execution grant. |
| `attestations[].verified_until` | Unix seconds, at most one hour after configuration load. Expired authority cannot admit new bindings or work. |

For a bound installation, lifecycle authority and its receipt commit in one
MongoDB transaction. `suspend` blocks work; `deleted` stays uninstalled. Positive
notifications require verification and cannot restore authority. Repository
removal, transfer and deletion remove the affected verified ID; renaming
preserves the ID, while a repository-created event does not enroll a new
repository. Adding a repository does not expand/restore removed authority. Other
repositories retain eligibility. Duplicate lifecycle delivery does not change
the revision twice. An uninstalled binding cannot be resurrected by a positive
event.

No JWT signer, current GitHub API reconciliation or verification refresh endpoint
exists yet. Unbound installation events return 404; App ping works before binding.
Resolve these limits before organization-wide production activation; do not edit
the database to bypass verification.

## Storage, errors and recovery

`eg_automation_records` stores sealed owner-bound bindings and compact receipts
using deterministic IDs and majority reads/writes. Existing interaction storage
holds separately sealed raw chunks with its seven-day TTL. Each chunk carries a
typed JSON envelope with receipt identity, owner, digest, index and count, so the
shared routed recovery scanner can safely skip it. All chunks commit before
the receipt. Failed attempts may leave expiring orphan chunks, never an acknowledged
partial receipt. Compact records currently have no expiry; terminal-record retention
remains required before production activation.
Business admission also commits against the binding revision in a transaction;
concurrent revocation cannot be overwritten by an old authorization snapshot.

| Response | Recovery |
| --- | --- |
| 400 `invalid_request` | Correct headers or shape; no business admission. |
| 401 `github_signature_invalid` | Check secret and exact bytes; no receipt created. |
| 403 `github_installation_forbidden` | Owner proof, binding or repository eligibility is missing, expired or revoked. |
| 404 `github_resource_not_found` | No resource visible to this owner; does not expose another tenant. |
| 409 `github_delivery_conflict` | Same delivery with different bytes/event; original receipt remains authoritative. |
| 413 `payload_too_large` | Over 256 KiB. Oversized-payload API reconciliation is not implemented yet. |
| 415 `unsupported_media_type` | Require UTF-8 JSON and no content compression; no receipt is consumed. |
| 503 `github_source_unavailable` | Disabled source, storage failure, or uncertain/timed-out commit. Query/retry the original identity. |

A 404 after disconnect is not proof that an in-flight write cannot still commit.
Concurrent duplicates converge on the create-only root; uncertain inserts/commits
are acknowledged only after exact readback. Receipt lookup does not depend on an
Activity window. Never invent a new key to bypass uncertainty.

## Release prerequisite: implement dynamic integration management

Do not supply a GitHub config file to make the current draft deployable. Replace
`Gateway::from_env`, its in-memory App/secret/proof lookup, static App route list
and GitHub-specific recovery record selection with the control-plane contract in
[the replacement design](CMAEG_GitHub_Integration_Design.md).

Reuse existing storage encryption, creator authentication and durable execution.
The additive automation migration/runtime-role mechanisms remain available for
feature storage; integration onboarding must not run DDL or change cluster
configuration. Register/rotate/verify/activate through authenticated public
interfaces and persist those changes before acknowledging them. Existing retained
tasks still need recovery after their source is disabled. A file-backed fallback
or per-App Kubernetes resource is not an acceptable replacement.

The following route-guard notes describe the earlier implementation only; they do
not reinstate its startup `apps[].routes` contract:

`exclude_pull_request_head_prefixes` is a generic route guard. It is evaluated
after installation and repository authorization but before channel resolution;
matching pull request deliveries are retained as `ignored` and never create a
task. A prefix such as `automation/` can prevent a configured publisher branch
from re-triggering its own goal agent while leaving other bot branches eligible.

The channel source must use the same `app_ref`, installation and repository IDs,
and explicit `issues`/`pull_request` action allowlists. A delivery is only routed
when the owner, installation, repository, source and action all match. The payload
cannot select a channel, Agent or credential. The example timestamp is illustrative;
the attestation must be within the one-hour configuration bound when CMAEG starts.

## TDD evidence and remaining work

Tests exercise real HTTP routes and authenticated MongoDB replica storage. Only
external NyxID is replaced. Internal collaboration is not mocked; assertions use
public responses, never database inspection.

Observed red/green slices: missing ingress → durable receipt after restart;
duplicate 503 → original receipt; ping/PR/lifecycle 400 → their explicit behavior;
large body 503 → expiring chunks and a 413 bound; lookup 404 → owner lookup;
repeated setup 503 → original binding; unsupported event 400 → retained ignore;
repository-removal 400 → scope revocation. Additional regression assertions cover
raw-byte signatures, conflicting redelivery and owner isolation.
Review added regressions for lifecycle/event-header substitution, media type, and
recovery after setup-proof expiry. Recovering an existing binding never extends
its verification expiry or restores its status.

Run `cargo test --locked -p cmaeg --test github_source -- --ignored --test-threads=1`
with `CMA_IM_TEST_MONGODB_URI` pointing at a disposable authenticated replica set.
Both existing Mongo contract runners include this target. Ordinary tests skip it
without the explicit Mongo fixture invocation.

## Direct-Agent admission lookup prerequisite

CMA now exposes `POST /api/v2/responses/lookup` for a caller with direct authority
on an existing Agent. The original creation body and original key recover the
response JSON and command/response IDs without reserving new work. Missing
admission is a non-final 404; changed input under the same key is 409. See
[Responses](API_Responses.md) for the full current subset.

Real-Mongo HTTP tests use the production CMA authentication, command and read
services, replacing only the external NyxID HTTP service. It verifies missing
lookup does not reserve a key, service reconstruction recovers the same command,
and GET response agrees with lookup. They also check foreign-owner concealment,
credential revocation and that lookup does not reapply creation-time model
constraints. These test a B prerequisite; they do not
demonstrate a worker, Runtime execution, OS-process crash recovery or PR publication.
Binding-key event lookup is still unsupported and discovery reports that limit.

Run `cargo test --locked -p cma responses_lookup_tests:: -- --ignored --test-threads=1`
with `CHRONO_TEST_MONGODB_URI` pointing at a disposable authenticated replica set.
Both Mongo contract runners also invoke this test.

## Verification record (2026-11-04)

| Check | Result and boundary |
| --- | --- |
| Native receipt HTTP + real authenticated MongoDB replica set | 16 passed in the working checkout. |
| Direct-Agent lookup HTTP + real MongoDB + production CMA authentication/services | 3 passed; external NyxID fixture only. |
| CMA control-plane binary unit suite | 1,057 passed; 530 explicitly ignored environment-dependent tests. |
| Layer 1 public interface suite, including contract fixtures | 399 passed. |
| Layer 2 core suite | 1,138 passed. |
| CMAEG / cmaeg-core Clippy, all targets, warnings denied | Passed. |
| Routed GitHub source/status increment | `cmaeg` 1,394 passed and 3 ignored library tests; GitHub config, execution-group state, authority-recheck and OpenAPI compatibility tests passed on 2026-11-04. |
| CMA / Layer 1 / Layer 2 Clippy, all targets, warnings denied | Passed on the isolated checkout described below. |
| CMAEG reviewed OpenAPI | Exact binary match; pre-existing path contracts unchanged. |

The working checkout also contains unrelated, pre-existing Agent skill changes.
Those currently cause a Rust `E0308` at `layer1-public/src/rest/instance_problem.rs:207`
(`String` versus `&str`). They were preserved. CMA/Layer 1/Layer 2 validation above
therefore used an isolated checkout of `677c27440aa96e428ee50f9801ea886136ea04c8`
plus this task's changes; it is not a green build claim for the combined working
checkout. The suites use the existing CI stack setting `RUST_MIN_STACK=33554432`.

Standards review found no actionable violation. Spec review found historical lookup
reapplying the current model constraint; a public HTTP test reproduced the 422,
then the minimal lookup-only change passed while new creation still refused the
mismatched model. No changes were deployed, committed, or made to live credentials.

Still required: remaining seam A activation/recovery/retention boundaries;
provider admission lookup and binding-key recovery (B); Runtime/NyxID
enforcement and end-to-end provider-side execution (C);
real App/Agent/CI/Pages acceptance. The chrono-goals Node/gh read and Git Data
API status PR adapter, including machine-readable `succeeded`/`no_change`
results, is implemented locally, but its repository permissions and end-to-end
use remain unverified.
No Agent, NyxID permissions or production deployment changed.
