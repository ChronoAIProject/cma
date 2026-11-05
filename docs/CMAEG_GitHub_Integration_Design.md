# CMAEG native GitHub App integration: dynamic control plane

Date: 2026-11-04, Asia/Singapore. Status: slices 1–3 are implemented locally; deployment and online acceptance remain outstanding. This document supersedes deployment-file App registration, static operator attestations, static App route lists, and per-pilot channel-record environment configuration in the earlier GitHub design and onboarding instructions. The revised authorization model uses durable tenant bindings and automatic eligibility reconciliation; it withdraws the earlier one-hour manual verification/renewal requirement. The side-chat revision was design-only; the owner subsequently authorized implementation in the main chat.

## 1. Decision and scope

GitHub App webhooks are a native CMAEG integration feature. An authorized user registers an integration, submits its webhook credential through a protected form, binds verified installations, and activates channel sources at runtime. These operations must not require editing Kubernetes ConfigMaps, Secrets, volumes, or environment variables, and must not restart CMAEG.

Implement one native GitHub App webhook mode within the existing gateway. Do not introduce a new gateway process mode, a forwarding service, a generic plugin framework, or a separate credential service. Keep GitHub protocol details in the GitHub source module. Platform-owned and user-owned Apps use the same resource contract; ownership affects authorization, not deployment shape.

The existing gateway database, creator authentication, organization authority, encryption keyring, channel interfaces, routed admission, recovery, activity, and task queries remain the base. Existing infrastructure credentials remain infrastructure configuration. A new product release and additive database migration may be needed once; adding each App, installation, or route must be a normal product operation thereafter.

The service startup no longer loads `CMAEG_GITHUB_CONFIG_FILE`. It initializes dynamic integration management from existing gateway storage/keyring dependencies when the additive integration migration and database role are ready. Registration/read/list and the capability projection are implemented; all new registrations remain `pending_verification`. The old proof/receiver/dispatch implementation and its historical fixtures remain pending replacement in subsequent slices. Its static configuration is not loaded by service startup. Removing a proof deadline alone is still not a valid implementation of this design.

**Release-contract correction, 2026-11-04:** the owner confirmed that schema and necessary data changes ship with feature code and run automatically during service startup when the feature flag is enabled. The local implementation now follows that workflow through the false-default `CMAEG_GITHUB_ENABLED` startup flag; Shining is not assigned a separate manual migration, and no migration-script directory is introduced. The existing Mongo connection prepares missing/version-1 storage and verifies availability before GitHub admission; version 2 skips DDL, and unknown versions fail closed. This flag takes effect on process startup, not through a new live toggle API. It carries no App, tenant, channel or credential configuration; dynamic onboarding remains unchanged. Startup adaptation is implemented locally, not deployed. Deployed flag state and database authority must still be verified separately.

### Implementation progress: slices 1–3

`POST /v1/github/apps`, owner-scoped `GET /v1/github/apps` / `GET /v1/github/apps/{app_ref}`, and `GET /v1/github/capabilities` are implemented in `github/integrations.rs` and `github/http.rs`. An atomic Mongo transaction stores the integration, separately sealed webhook credential, and owner-scoped idempotency result. Registration accepts a write-only optional webhook secret; it does not yet accept a signing key. Replays after restart return the same App reference, changed private input returns 409 `conflict`, and public responses/logs omit submitted credentials. List pagination uses `limit` (1–100, default 50) and `after`, returning an optional `next_cursor`.

The receiver now resolves the current sealed credential from storage, authenticates the original bytes before parsing or duplicate lookup, and retains a ping without requiring an installation or channel. Receipt identity includes the opaque integration reference and original delivery ID; two unverified drafts claiming the same numeric App ID cannot share receipts. Both receipt lookup paths survive restart and enforce receipt ownership. The historical receiver fixtures now register credentials through the public API; no static credential fallback was added.

`PATCH /v1/github/apps/{app_ref}` updates the display name with CAS and idempotency. App ID and authority/status are not mutable request fields. `PUT /v1/github/apps/{app_ref}/webhook-secret` sets or rotates the write-only credential, with a default 600-second overlap, maximum 3600 seconds, and explicit zero for immediate replacement/retirement. A new overlapping rotation while a prior overlap is active returns 409. Replaying an earlier operation returns its original metadata without restoring retired credentials. `POST /v1/github/apps/{app_ref}/verification` requires independently authenticated `cma-eg:admin` permission, records the verifier and evidence reference, and atomically reserves the `(host, app_id)` identity with the metadata and replay result. Ordinary channel-management authority cannot approve an App. `X-CMAEG-Owner` requires current NyxID organization-admin evidence; the verifier's personal identity remains in the audit metadata.

Public process tests in `tests/management/github_integrations.rs` exercise the compiled gateway, authenticated disposable Mongo replica set, and external NyxID/provider fixtures. New behavior was introduced through failing HTTP tests followed by implementation: dynamic ping returned 404, metadata update returned 405, verification/rotation paths were absent, and selected organization ownership was previously ignored in service authentication. Additional tests cover simultaneous identity reservations, cross-process credential changes, restart/replay, private-input conflicts and migration readiness. They run in the existing Mongo contract jobs. Startup now invokes the additive migration for missing/version-1 storage when the product flag and existing database authority permit it. The `github_integrations` version-2 marker is written last; disabled or failed initialization reports 503 for integration operations and leaves ordinary channels available. The explicit migration command is retained but is not a normal rollout step.

Capabilities report dynamic registration, native webhook receipt, secret rotation and `app_identity_verification` as implemented. `ownership_verification` still means installation ownership and remains false, as does automatic reconciliation. An App status of `verified` records only the administrator's App-identity decision; it does not activate a source. Signing credentials, permanent installation approval, automatic eligibility checks, dynamic routing/enrollment and UI are still pending. The next implementation slice is 4, durable installation approval separated from execution eligibility. The first online checkpoint after slice 3 is limited to management, signed ping/receipt recovery and isolation; the full goal-agent/PR acceptance remains slice 22. See [the online checklist](CMAEG_GitHub_Online_Verification.md).

Local validation on 2026-11-04 passed: 17 dynamic integration process tests, 20 historical receiver regressions, 12 GitHub module tests, 16 authentication-related tests, 2 Mongo isolation contracts and 6 OpenAPI compatibility tests. Targeted Clippy with warnings denied, Rust formatting, API documentation coverage, local document links, credential scanning and `git diff --check` also passed. No full-workspace test run, production release, live GitHub delivery or agent/PR execution is claimed. The three CMAEG Kubernetes manifests remain identical to HEAD; unrelated shared-workspace edits are preserved. Later rotation/recovery slices still add their deadline/race/failure-injection coverage; the presence of some shared primitives here does not mark those later slices complete.

Startup follow-up validation on the same date passed 70 related tests: 23 integration process tests (including six new startup tests), 20 receiver regressions, 12 GitHub module tests, two Mongo isolation contracts, six OpenAPI compatibility tests and seven media-admission compatibility tests. The disabled-flag test first exposed readiness remaining true; the enabled-startup test first returned 503 instead of accepting registration. Concurrent-startup regression then exposed a duplicate marker insert (Mongo code 11000), fixed by majority-reading the completed version before treating that race as success. Targeted Clippy, formatting, configuration/API documentation checks, relative links, credential scanning and diff whitespace checks passed. This follow-up did not re-run the full workspace or deploy the service.

## 2. Current code and the missing feature

| Existing location | Current behavior | Required change |
| --- | --- | --- |
| `api/crates/cmaeg/src/github/config.rs` | Historical fixture configuration still contains static attestations/routes; production startup no longer loads it, and the receiver no longer reads its secret | Replace remaining legacy binding/routing adapters with durable installation approval and dynamic source projection |
| `api/crates/cmaeg/src/github/config.rs:110`, `github/mod.rs` | Static proofs are bounded by `now + 3600`; binding admission and business ingress reject an elapsed `verified_until` | Replace the manual proof deadline with durable ownership plus separately refreshed execution eligibility; deleting the expiry check alone is insufficient |
| `api/crates/cmaeg/src/github/mod.rs`, `Gateway::from_records` | Dynamic registration initializes from existing storage/keyring, without App environment configuration | Move receiver/dispatch authority to current persisted integration and eligibility state in subsequent slices |
| `api/crates/cmaeg/src/github/http.rs` | Native receive, binding, receipt and task HTTP interfaces | Retain delivery/task paths; add dynamic management and verification interfaces |
| `api/crates/cmaeg/src/github/store.rs` | Sealed, owner-bound records, majority storage and CAS in `eg_automation_records` | Extend typed records and transactions; preserve record identity checks and encrypted custody |
| `api/crates/cmaeg/src/github/startup.rs`, `github/schema.rs`, `config.rs` | False-default product flag, bounded automatic schema preparation, version fencing, effective migration authority and concurrent marker reconciliation | Verify the deployed flag and effective database permissions; future schema changes remain versioned service code |
| `api/crates/cmaeg-core/src/channel/secrets.rs` | Redacted `Secret` type and credentials excluded from public serialization | Reuse this treatment for integration credentials; never return a private storage model as a response |
| `api/crates/cmaeg/src/channels/requests.rs` | Existing native sources accept credentials through management requests | Follow the protected write/public-read pattern, while keeping the shared App credential outside individual channels |
| `api/crates/cmaeg-core/src/channel.rs`, `Source::Github` | App reference, installation/repository IDs, denies, event/action filters | Keep this source shape; make `app_ref` resolve a registered integration and enforce verified binding ownership |
| `api/crates/cmaeg/src/config/routing.rs` | Explicit deployment-selected routed record IDs | Add persisted GitHub source enrollment to recovery eligibility; no per-channel environment edits |
| `api/crates/cmaeg/src/auth/creator.rs`, `auth/permissions.rs` | Authenticated creator/organization selection and gateway admin authority | Reuse creator ownership; independently require gateway admin authority for pilot verification decisions |

The existing implementation cannot be called dynamically managed merely because it accepts dynamic channel definitions. Its App registry, credential, installation-proof and recovery-selection prerequisites must also be dynamic.

## 3. Resource and credential ownership

| Resource | Owner and authority | Public projection |
| --- | --- | --- |
| GitHub App integration | Authenticated individual or organization administrator who registers and verifies control of the App | Server-generated `app_ref`, host, numeric App ID, display name, status, revision, webhook URL, verification status |
| App webhook credential | Integration administrator; one credential lifecycle per App registration | Presence, revision and rotation deadline only; no plaintext, digest, prefix or suffix |
| App verification credential | Integration administrator; dedicated server-side App signing authority for automatic checks | Presence, version and health only; never the private key, JWT or installation token |
| Installation binding | Verified CMA owner for that GitHub installation, independently of the App administrator | Durable ownership/approval, installation/account IDs, binding status and verification audit reference; no periodic ownership expiry |
| Installation eligibility | CMAEG-maintained observation of current GitHub access | Repository IDs, eligibility state, authority revision, last check, freshness deadline, retry time and actionable reason |
| GitHub channel source | Binding owner with existing channel management authority | Integration reference, permitted installations/repositories, exclusions, event/action filters and activation state |
| Agent GitHub connection | Agent owner and NyxID authorization | Existing connection/grant identifiers; never the App webhook credential |
| Gateway bootstrap | Platform operator | Existing storage, authentication and encryption infrastructure; no App-specific data |

GitHub configures its webhook URL and signing secret on the App registration [G1]. Every installation of that App uses its integration endpoint. Installing a shared App grants no permission to read or rotate the App secret. App administration grants no permission to read another installation owner's retained events or change its agent bindings.

Use `app_ref` as the opaque integration identifier already present in the source and receive path. Generate it server-side; do not embed the goal-board repository name, owner credential, or installation ID in it. Retain `POST /v1/github/apps/{app_ref}/webhooks`, with the public URL returned by registration using the existing trusted public-origin mechanism. Do not derive origin from an arbitrary request header. Previously suggested URLs containing `chrono-goals` were placeholders, not registered endpoints.

## 4. Secret custody and rotation

For the first implementation, store the webhook secret in a private integration record sealed with the gateway's existing `MasterKeyRing`/`seal_blob` machinery. This avoids a new secret-store dependency and reuses existing key rotation and encrypted-record custody. Bind the sealed value to integration identity, owner, kind and credential version, and validate those fields on read. Follow the existing public/private credential separation; using a database does not make returning secrets acceptable.

The human enters a locally generated secret into both GitHub App settings and the CMAEG integration form. The form sends it over the authenticated HTTPS management interface; do not put it in a URL, command argument, chat, agent prompt, browser persistent storage, audit body or telemetry. The response contains metadata only. Redact validation errors and request diagnostics before ingestion. A normal agent/channel read must never expose the secret.

The webhook handler resolves the integration from the path, decrypts its credential only server-side, and verifies the exact body bytes before parsing or routing [G2]. A payload cannot choose its credential, owner, agent or destination. The App private key remains separate from the webhook secret. Automatic control-plane checks now require protected App signing capability, described in section 5.2; agent execution continues through its separately approved NyxID connection.

Rotation is an authenticated integration operation using `expected_version` and `Idempotency-Key`. Stage a replacement before changing GitHub; allow at most two credential versions during a bounded overlap, default ten minutes, and persist the previous credential's retirement time. Return the rotation operation/version, not secret material. After the GitHub-side update, confirm a delivery verified with the new credential and finish retirement. An administrator can retire the old version immediately for suspected compromise. Cleanup never extends a retirement deadline after restart.

Credential revision and authorization revision are separate: normal rotation does not cancel valid queued work or change delivery identity. Pausing/revoking an integration does fence new business admission and not-yet-dispatched work. An already running agent remains subject to its separate execution grants; stopping an inbound integration does not revoke the agent's NyxID connection.

## 5. Durable ownership and automatic eligibility checks

### 5.1 Initial ownership verification

Integration registration is initially `pending_verification`, with an optional usable credential for ping testing. A correctly signed ping proves endpoint/secret configuration, not ownership of a GitHub App, an installation or an organization.

Keep independent platform verification for initial pilot ownership approval, expressed as a durable control-plane operation. This is a one-time delegation to the CMA tenant, not an hourly approval cycle:

1. The creator registers a draft App integration. Derive its CMA owner from authenticated creator/organization selection, never a body-supplied owner.
2. A gateway administrator independently checks the actual App registration and the requester's App administration authority in an authorized GitHub session. Record verifier identity, evidence reference, App ID and verified time. The applicant cannot approve this merely by owning the draft.
3. Commit verified App identity and reserve `(github.com, app_id)` uniquely. Unverified drafts cannot squat on a global App ID or replace an already verified integration.
4. A CMA owner requests an installation binding. A gateway administrator verifies installation → App, account/repository IDs, and the applicant's relevant GitHub organization administration authority. Store the decision bound to that request and owner. The installation page URL or a successful HMAC is not sufficient proof [G3].
5. Atomically record the approved binding under one CMA owner, then request an automatic GitHub eligibility check. Approval alone does not activate business routing. A different owner cannot claim the installation through a new route or App alias. Ordinary App updates cannot change verified identity or transfer installation ownership.

Use the existing `cma-eg:admin` authority for the pilot verifier; explicitly check it in the verification handler instead of treating the general channel-creator permission as verification authority. Existing NyxID org-admin selection supplies tenant scope. Administrative membership changes are authorization management, not App configuration in gateway Kubernetes resources.

Versioned, replay-safe approval records replace file-loaded `attestations[]` and `proof_ref`. The tenant binding has no time-based expiry. It persists until explicit revocation, verified transfer or installation removal, with historical ownership retained for receipts and audit. Permission to execute is separate and depends on current eligibility. The original requester's later membership is not inferred from App API responses; the approved automation belongs to the tenant and remains subject to its current CMA administration/delegation policy. Reassigning a tenant or account requires fresh ownership approval.

Unbound installation events retain minimal integration-level receipts and do not launch work. Business events without a verified owner binding are `ignored: installation_unbound`; never route them retroactively when someone later binds the installation. Minimal unbound metadata is visible only to the integration administrator. After binding, perform an explicit authorized reconciliation. Existing owner-bound receipts remain with their original owner.

### 5.2 Credential needed for automatic verification

The webhook secret cannot authenticate a GitHub API query. The minimum default is a separate, write-only App private-key upload through the integration interface, sealed with the existing keyring and scoped to that integration. Its private input, storage type and rotation are separate from the webhook credential. No key, JWT or token is returned to the owner, channel, model or agent; no Kubernetes resource is added or edited.

CMAEG uses the key only in its server-side verification module: authenticate the App with `GET /app`, query `GET /app/installations/{installation_id}`, and check the bound account, suspension and permissions [G4]. Repository eligibility uses fully paginated `GET /installation/repositories` with a freshly issued, metadata-only installation token [G5][G6]. Explicitly limit permissions and validate the returned grant; never accept default full App permissions. Inventory may cover the installation's repository metadata, including selected private repositories, but never discovers projects for chrono-goals or changes channel allowlists. An intentionally repository-restricted or incomplete inventory cannot prove that other repositories have been removed.

The App private key itself has broader signing authority; only the reconciler's operations and issued token are restricted. Do not expose a generic signing/token API or permit content/issue/PR writes through this module. Renew credentials server-side using their actual expiry, and invalidate token caches on credential rotation or observed authority change. Token expiry never expires the tenant binding. Revoking a key in CMAEG stops local use; removing the key at GitHub remains a separate App-administrator action.

This is a required addition for unattended operation, not an assertion that NyxID already provides an App signer. A future confirmed NyxID signer/broker can replace key custody through the same narrow verification interface. The pilot does not implement installation tokens for agent execution: the agent still uses its existing approved NyxID connection. Missing verification credentials allow setup/ping diagnostics but block source activation with `verification_credentials_required`.

### 5.3 Reconciliation and freshness

Use a small reconciler in the existing gateway process, with persisted due times, a per-installation lease and existing CAS/recovery storage. Do not create another service or a generic scheduling system. Trigger a check on initial approval, credential replacement, source scope changes, relevant lifecycle events, stale dispatch and restart of overdue work. Coalesce concurrent requests for the same installation.

Recommended initial machine-policy defaults are a five-minute periodic check with jitter and a maximum fifteen-minute age for a successful complete observation. These are bounded, persisted integration policy, not per-App environment settings, GitHub rules or human reapproval deadlines. The source/dispatch check reads `last_checked_at`, `fresh_until`, `next_check_at`, `eligibility_state`, `last_error_code` and `authority_revision`. Count freshness conservatively from the check start; partial pagination, timeout, retry scheduling or restart never extends it. These values bound the stale-access window; they do not promise instantaneous detection of every missed external revocation.

Lifecycle events tighten eligibility immediately. A suspend, removal or repository exclusion advances authority revision and fences unstarted work. Positive events request a current API check instead of granting access directly. A completed check may restore eligibility for the same approved owner and existing allowed source, but cannot undo a local pause/revocation, a deletion tombstone or an ownership conflict. A reinstallation with a different ID needs a new approved binding. Compare the stored revision when committing a check so an in-flight result cannot overwrite a newer negative event. Successful unchanged checks update observation freshness without changing the authority revision or cancelling valid work.

Only complete, consistently scoped GitHub observations can refresh positive eligibility. An unexpected 404 for a known installation marks eligibility `stale` with reason `verification_not_found` and blocks dispatch pending another authenticated check; it is not by itself evidence of uninstall or a tenant transfer. Rate limits and transient 5xx/network failures retry with bounded backoff, jitter and GitHub retry hints. Refresh an expired derived token automatically; a persistently rejected App signing credential requires administrator repair. The periodic check is necessary because webhook delivery alone does not provide recovery from every missed notification [G7][G8].

### 5.4 States, execution and recovery

| Condition | Public state and automatic behavior | Execution / owner involvement |
| --- | --- | --- |
| Initial ownership approved, no complete API check | Binding remains approved; eligibility `pending_check` | No business execution until the automatic check succeeds |
| Complete allowed observation, no negative signal | Eligibility `active`; next check scheduled | Admit and dispatch within the current source and agent grants |
| Transient check failure while observation is fresh | Keep current eligibility with degraded check health and retry time | Existing fresh authority may still be used; no human renewal |
| Freshness deadline passes without success | Eligibility `stale`; affected retained work `recovering` | Stop dispatch; retry automatically; preserve binding and task identity |
| Confirmed suspension or repository access removal | Eligibility `suspended` or affected repository ineligible; advance authority revision | Fence affected work; only current allowed facts can restore future eligibility |
| Confirmed uninstall or explicit local revocation | Binding retained as revoked/tombstone | No automatic resurrection; a new installation requires initial approval |
| Invalid signing credential, account/tenant conflict or authorization requiring repair | Expose a specific `required_action`; affected work `owner_action` | Owner intervenes to repair credentials or reauthorize, never merely because an hour passed |

Signed lifecycle deliveries remain receivable while eligibility is stale or suspended. For a previously approved binding and an explicitly allowed source/repository whose eligibility is only stale, durably retain business deliveries as `202 pending_verification` with owner, source and authority context; do not dispatch them. After a successful check, reevaluate the same receipt against current grants and resume or finalize it as ineligible. This path cannot authorize previously unbound/never-allowed events or revive work fenced by a confirmed revocation. Bound pending work by the existing retention/capacity policy, surface expiration, and return 503 if durable receipt commitment is unavailable. Do not depend on GitHub automatically retrying an unretained failure [G8].

At dispatch, recheck freshness and current revisions in addition to existing tenant/source checks. On restart, resume overdue checks and retained tasks from their persisted identities; successful reconciliation clears transient recovery automatically. Inbound changes do not revoke independent running-agent credentials: retain the separate execution-grant fencing and uncertain-side-effect recovery requirements from the original design.

## 6. Dynamic routing and execution enrollment

Resolve eligible channels through persisted native GitHub sources and verified bindings. Remove the separate `apps[].routes` allowlist from startup configuration. Keep installation/account/repository numeric identity, source event/action allowlists and repository exclusions. A source may only refer to installations bound to that channel's owner; an App shared across tenants is not a cross-tenant route grant.

Source activation must persist the channel record's eligibility for durable GitHub admission and recovery using the existing routed store. No user should add a channel ID to `CMAEG_ROUTED_CHANNEL_RECORDS` to complete GitHub onboarding. Preserve existing non-GitHub rollout behavior and platform-wide safety controls; add a narrowly scoped persisted enrollment for verified GitHub sources. Recovery eligibility survives source pause/deletion while retained tasks still need reconciliation. Disabled sources cannot accept new work.

Build a small persisted route lookup projection from the existing channel/source state. It is only a candidate index, not authority: re-read current channel, integration and binding revisions before admission. A stale index may delay a newly added route, but must not authorize a removed route. Creation/activation is complete only after the durable projection/enrollment is committed; index rebuilding must not be necessary for the first accepted delivery. Do not scan all tenants for each webhook or create a second configuration authority.

Initially use majority reads of current integration/binding versions and persisted eligibility freshness on every receive and dispatch; add no in-memory authority cache. Fence acceptance against the versions in the storage transaction. If rotation/pause races with receipt commit, re-read and reverify as appropriate. Transient GitHub verification outages follow the durable pending/recovery behavior in section 5.4; unavailable required local storage returns a retryable error. Successful control-plane writes are visible to every replica without restart. Multiple replicas retain existing CAS, lease and group execution behavior.

Retain durable receipts, bounded batch accumulation, stable task IDs, pending arrivals during a run, the scheduler and Responses lookup. Preserve existing result-verification limitations rather than treating this redesign as proof of end-to-end PR correctness. The webhook secret is never an agent credential. Full sync, allowed outputs, PR idempotency and CI remain the existing chrono-goals workflow.

Keep self-output suppression in owner-controlled source/execution policy, not in App startup configuration. The first pilot routes only the CMA repository, so target-board PR events do not match that route. If a later route includes the target board, identify this automation's output by the configured target repository and recorded PR/branch identity; do not exclude every bot or every branch with a shared `automation/` prefix.

## 7. Proposed public contract

All paths are relative to the gateway public base. These operations are proposals, not claims about the deployed OpenAPI. Management uses existing creator/organization authentication. Reads return only authorized public projections; cross-owner reads return 404. Write-only credentials use separate input types.

| Interface | Behavior and recovery |
| --- | --- |
| `GET /v1/github/capabilities` | Reports `native_app_webhook`, dynamic management/rotation, ownership verification, automatic reconciliation and store readiness; no tenant inventory. Runtime support is separate from each App's credential health. Not ready means onboarding is unavailable, not a prompt to configure per-App environment variables |
| `POST /v1/github/apps` | Register a pending integration with display name, App ID, optional secret and stable idempotency key. Return 201/200, `app_ref`, version, status and webhook URL; no secret |
| `GET /v1/github/apps` and `GET /v1/github/apps/{app_ref}` | Owner-scoped listing/read, separate webhook/signing credential presence/version/health, ownership verification and signed-ping observations; recover an uncertain create with the same key/body |
| `PATCH /v1/github/apps/{app_ref}` | Versioned metadata/pause/activation update. Activation requires verified App identity, webhook credential and a working verification credential; verified App ID is immutable. Individual source activation also requires current installation eligibility |
| `PUT /v1/github/apps/{app_ref}/webhook-secret` | Write-only initial setup or staged rotation; versioned/idempotent. Metadata reports overlap/retirement; never return the submitted value |
| `PUT` / `DELETE /v1/github/apps/{app_ref}/verification-credential` | Protected App private-key setup/rotation or local removal. Validate App identity before promotion, return metadata only, invalidate derived credentials, schedule new checks; removal fences dependent execution and reports credential repair required. No generic signing or token-export interface |
| `POST /v1/github/apps/{app_ref}/verification` | Explicit gateway-admin decision with evidence reference and expected version; update verified App authority, not a client assertion of ownership |
| `POST /v1/github/installation-bindings` | Owner submits installation candidate tied to the verified integration. Returns a pending binding request; the previous static `proof_ref` contract is withdrawn |
| `POST /v1/github/installation-bindings/{binding_id}/verification` | Gateway-admin commits initial ownership approval or explicitly requested reauthorization, then schedules an automatic eligibility check. No periodic renewal or client-supplied expiry/freshness |
| `POST /v1/github/installation-bindings/{binding_id}/reconcile` | Authorized owner/admin requests a coalesced asynchronous check; return 202 and its stable identity, queried through binding GET. This operation cannot approve ownership, extend freshness or reactivate revoked bindings; normal checks run without user requests |
| Existing binding GET, channel create/update/source test | Separate durable binding status from current eligibility; expose last successful check, freshness/retry timestamps, reconciliation status and safe required-action code. Configure source filters and persisted durable enrollment; no credential echo |
| Existing webhook, delivery and task paths | Preserve raw-body HMAC, durable receipt lookup, duplicate/conflict identity, queued/running/recovering/terminal states and PR/result query behavior |

An explicit `expected_version` is required for mutations of existing resources, using the repository's current CAS convention; stale versions return 409 with safe current metadata. Idempotency keys are scoped to authenticated owner plus operation/resource. Hash private request fingerprints with a keyed mechanism instead of storing raw-secret request bodies in an idempotency ledger. Exact replay returns the original result without restoring revoked authority; a changed body under the same key returns 409.

For management: 400 invalid shape; 401 unauthenticated; 403 missing permission; 404 absent/invisible resource; 409 version/idempotency/verified-identity conflict; 422 unsatisfied activation prerequisites; 503 unavailable storage or uncertain commit. Recover uncertain writes using the same operation identity and metadata read/replay; never create a new App reference to bypass uncertainty.

For webhook: retain 401 signature failure, 409 conflicting delivery, 413 over-size and 503 unknown commit/unavailable required local storage. Fresh retained business work returns 202; a retained stale-eligibility receipt returns 202 with `pending_verification` and does not promise agent execution. An exact duplicate or retained ping/ignored event returns 200 with the original status. Authenticated pause/unbound/revoked outcomes may produce compact ignored receipts but never tasks. HMAC verification is still required before duplicate lookup so retired credentials cannot retrieve/replay a receipt. Receipt owner authorization is separate from signature verification.

## 8. User-facing onboarding and event sequence

Add a native GitHub App integration setup entry in the existing CMAEG console, backed by the same public interface. Initial fields are App ID, display name and the write-only webhook secret, with a separate protected App private-key upload for automatic verification. Explain the credential purposes separately. The view shows the returned endpoint, ownership/binding status, source eligibility, last successful automatic check, retry health and last signed ping. Rotation/pause are ordinary integration actions; no Kubernetes fields, configuration-file path, hourly renewal prompt or environment-variable editor appears in this flow.

```mermaid
sequenceDiagram
    participant U as Integration administrator
    participant EG as CMAEG control plane / ingress
    participant DB as Existing encrypted gateway storage
    participant V as Authorized pilot verifier
    participant GH as GitHub App
    participant C as Channel owner
    participant W as Existing durable worker
    participant R as Gateway eligibility reconciler
    U->>EG: Register App; separately submit webhook and signing credentials
    EG->>DB: Persist pending integration and separately sealed credentials
    EG-->>U: app_ref, webhook URL, status (no secret)
    V->>EG: Verify App administration and installation ownership
    EG->>DB: Durable owner approval and pending eligibility check
    R->>GH: Authenticate App; read installation and complete repository inventory
    R->>DB: Commit eligibility, authority revision and next check
    C->>EG: Configure and activate GitHub source / consumer
    EG->>DB: Persist channel routing and durable enrollment
    U->>GH: Configure returned URL, same secret, events; enable webhook
    GH->>EG: Raw signed delivery
    EG->>DB: Read current credential / authority / channel; commit receipt and group
    EG-->>GH: 202 after durable acceptance
    W->>DB: Claim task and recheck current authority
    W->>W: Run existing CMA execution and query recovery
    C->>EG: Query delivery, task and structured PR result
    loop Periodic or lifecycle-triggered check
        R->>GH: Reconcile current eligibility with scoped credentials
        R->>DB: Refresh observation or restrict access; persist retry if unavailable
    end
```

Ping can be tested before a channel or verified installation exists, but never activates either. Subscribe to lifecycle/business events on GitHub; GitHub-side subscription and CMAEG-side routing are separate controls. Registration does not modify the GitHub App settings automatically. A transient verification outage retains permitted-owner deliveries in the pending path described in section 5.4; it never waits for GitHub API reconciliation inside the webhook response deadline.

For the present pilot, App ID `5606750`, supplied installation ID `178446930`, organization ID `253878174` and repository ID `1330597394` are enrollment inputs. They are not deployment constants or proof. Only `ChronoAIProject/cma` triggers the initial route. Keep the full chrono-goals `projects.yaml` sync, use the existing approved NyxID GitHub execution path, publish a status PR, and leave first merge/Pages verification to the established workflow.

## 9. TDD seams and vertical implementation slices

Use the `tdd` and `codebase-design` skills. The owner authorized continuation with these proposed seams in the main chat; slices 1–3 now have public-interface tests. A later material change to a seam still requires agreement before writing its tests. Retain behavior tests at existing public receiver/task interfaces where their contract remains valid.

| Seam | Captures through public behavior | Does not prove |
| --- | --- | --- |
| A: integration management + webhook + receipt query | Registration/authorization, separate write-only webhook/signing custody, HMAC, rotation, pause, cross-owner isolation, durable duplicate/conflict recovery and cross-replica visibility | Genuine GitHub organization approval, browser rendering, agent/PR execution |
| B: ownership approval + automatic reconciliation + channel activation + task query | Durable owner binding, scoped verification access, freshness, missed/late lifecycle events, no payload-selected route, filters, automatic recovery, revocation fencing and coalescing | Real GitHub organizational authority, live rate-limit behavior or external PR side effects |
| C: actual agent execution + recoverable PR result | NyxID-backed GitHub reads, complete configured sync, bounded status publication, uncertain PR recovery, checks and no-change behavior | App administration proof or delivery network guarantees |

Replace only external systems: GitHub API/delivery sender, NyxID identity authority, clock and CMA Responses provider where outside the seam. Do not mock internal registry, verifier, signer, reconciler, routing, crypto or group functions, and do not assert call order. A GitHub fixture validates the submitted credential/grant and serves controllable installation states, paginated inventories and failures; assertions concern public eligibility, receipt and task outcomes, not internal calls. Use real Mongo replica-set tests for transactional/CAS/restart conclusions, and verify outcomes through public reads. Advance the external clock to test freshness and operation beyond an hour without real waits. For confidentiality tests, use known synthetic sentinel credentials and inspect public response/audit surfaces; never inspect production secrets.

Startup acceptance uses the existing seam A process tests. One red→green behavior at a time introduced flag gating and automatic preparation; subsequent coverage verifies disabled startup refuses GitHub admission while ordinary channels remain available, enabled startup prepares missing/known-old GitHub storage without the migration command, restart preserves the same App and receipt, initialization failure keeps GitHub unavailable until the external fault is corrected, and disabling/re-enabling retains records. Concurrent startup and rejection of unknown future versions are covered. These HTTP observations prove readiness, admission and recovery, not deployed database grants or the deployment operator's flag control. Tests use no mocks of migration functions or assertions about their call order.

Implement each row as one failing behavioral test followed by the minimum implementation, then proceed to the next row:

| Slice | First failing test | Minimum change |
| --- | --- | --- |
| 1 | With the product feature enabled, an authorized owner registers an App and reads redacted metadata without any App-specific file/environment or restart | Persisted typed integration/credential, protected create/read and capability projection |
| 2 | A signed ping to the returned URL is queryable; invalid body/signature is refused | Receiver resolves current integration credential from storage |
| 3 | Another owner cannot read/update/rotate the integration or replace its verified App identity | Existing creator/org authority checks, verifier-only activation and atomic App identity uniqueness |
| 4 | Initial owner approval creates a durable binding but cannot dispatch before an automatic eligibility check | Separate binding status and eligibility; remove startup proofs |
| 5 | Activating a source without verification credentials returns `verification_credentials_required` | Capability/activation prerequisites and protected credential write contract |
| 6 | Uploading a key for another App cannot make the integration ready | Server-side App identity validation before credential promotion |
| 7 | A complete current observation allows the approved source to admit a task without deployment edits | Narrow signer/metadata inventory, route projection and persistent durable enrollment |
| 8 | An incomplete repository inventory cannot refresh eligibility or silently revoke unobserved repositories | Full pagination/coverage check and explicit unknown status |
| 9 | Healthy periodic checks keep the same binding active beyond one hour without an approval request | Persisted due times, lease, freshness policy and unchanged-authority refresh |
| 10 | A repository removed at GitHub without a delivered lifecycle event is fenced by the next periodic check | Compare current inventory and advance authority revision on an effective access change |
| 11 | A delayed positive event or older check result cannot undo a newer suspension | Negative-event fencing and revision-checked reconciliation commit |
| 12 | After freshness expires during a transient outage, a signed eligible-owner delivery is queryable as pending and does not dispatch | Durable pending receipt and automatic retry without owner reapproval |
| 13 | Recovery of GitHub verification automatically resumes that same retained receipt/task identity | Current-grant reevaluation and transition out of transient recovery |
| 14 | Expiry of a derived GitHub token is recovered without expiring the tenant binding | Server-side credential refresh driven by actual token expiry |
| 15 | Persistent rejection of the App signing credential reports credential repair, not an hourly binding renewal | Bounded authentication retry and a specific required-action projection |
| 16 | A local pause racing with a successful check still prevents task dispatch | Transactional integration/source authority fences |
| 17 | A second gateway replica rejects a retired webhook secret without restart | Versioned credential staging/retirement and current-state reads |
| 18 | A lost management response replays to the same integration/rotation/binding outcome | Durable keyed request fingerprint and safe recovery semantics |
| 19 | Reusing a management idempotency key with a changed credential body returns a conflict | Fingerprint conflict handling without retaining plaintext |
| 20 | Restart resumes an overdue check and retained work without duplicate task identity | Existing lease/recovery machinery and persisted reconciliation progress |
| 21 | The console shows transient verification recovery without asking for a deployment edit or manual renewal | Minimal UI over already-tested credential, binding and health projections |
| 22 | A real CMA issue event produces a recoverable PR or verified no-change result | End-to-end fixture/configuration; fix only observed integration gaps |

Keep the prior full-sync, batching, task/PR identity and outcome tests. These slices replace the file-config prerequisites rather than silently duplicating both systems. Do not add a permanent static fallback or a GitHub-to-Custom forwarding workaround.

## 10. Acceptance, deployment and handoff

The feature is ready for the first closed-loop pilot only when an authorized owner can register the App, enter/rotate its distinct webhook/signing credentials, obtain a webhook URL, approve installation ownership and activate the CMA route after automatic verification entirely through CMAEG interfaces. At least two running replicas must observe registration/rotation/pause without rolling pods. Restart must preserve integrations, due checks and queued work. Installation events, unknown/removed repositories and cross-tenant attempts must fail closed or produce the specified ignored receipt. Duplicate deliveries must retain the same task identity; a real status PR must be queryable and attributable to its source delivery.

Unattended operation additionally requires demonstrated automatic checks beyond the former one-hour boundary, recovery from a transient GitHub outage without owner reapproval, removal detection when a lifecycle delivery is deliberately omitted, and rejection of delayed positive events. Incomplete inventories and ambiguous not-found responses must not refresh authority. Credential expiry is automatically recoverable; true credential revocation exposes a specific repair action. Verify these through public observations, with external-clock tests for long intervals and a live rehearsal of recurring checks. There is no hourly human-renewal acceptance path.

Acceptance also requires zero GitHub-specific changes to `infra/cmaeg/configmap.yaml`, `secret.yaml` or `deployment.yaml`. No integration credentials or IDs may appear in those manifests, new per-App Kubernetes resources, Helm values or a replacement environment variable. The existing database keyring is reused without rotating or editing its Kubernetes configuration for this use case. Implement the additive storage migration and required existing database-role updates as a normal product release; ordinary integration onboarding performs no DDL and grants no cluster access.

The implemented startup behavior is feature-controlled initialization using existing gateway dependencies. With the rollout flag disabled, do not perform GitHub schema/data changes or admit new GitHub work; retain stored records. When enabled, apply versioned, repeatable updates in service code, verify schema/indexes and authority, then expose availability. Initialization failures or the 30-second initialization deadline leave GitHub unavailable while unrelated sources continue; a later startup can resume after the underlying fault is repaired. Do not downgrade unknown schemas or delete records when disabling the flag. Existing deployment credentials may already permit initialization; the program never grants itself database privileges, and neither `shared-schema` nor `isolated` authority checks may be bypassed. The explicit migration command remains an operator tool, but is not a normal release prerequisite. A concurrent duplicate marker write is reconciled through a majority read of the expected completed version; other errors remain failures. A running migration/rollout never reports readiness before schema/authority checks pass. Bootstrap constants may bound payload size, batch windows and record limits, but may not carry tenant integration state. Online deployment and effective database authority remain unverified.

| Responsibility | First implementation |
| --- | --- |
| CMAEG | App registration UI/interface, separately sealed webhook/signing credentials, narrow App signer and metadata-only token refresh, one-time ownership records, automatic eligibility reconciliation, native ingress, owner routing and delivery/task recovery |
| CMA | Existing agent lifecycle/Responses and execution permissions; no webhook secret enters agent context |
| NyxID | Existing creator authentication and separately granted agent GitHub access; a future confirmed managed signer may replace CMAEG key custody, but its availability is not assumed |
| chrono-goals | Existing complete sync and previously scoped authentication/PR adapter; no Kubernetes knowledge or integration secret |
| Human administrators | Configure the GitHub App, submit its separate credentials through protected forms, approve initial ownership, and repair actual revocation/credential problems; no periodic manual renewal |

Future self-service ownership approval can replace the initial pilot verifier through the same binding interface. Installation-token brokerage for agent execution remains separate from the metadata-only tokens required here. Automatic eligibility reconciliation is required in this feature, not deferred to a future optional phase; PR result recovery must still be independently verified.

Recommended defaults: one native integration type; app_ref returned by registration; existing encrypted gateway storage; one-time admin-assisted tenant approval; server-side App verification credentials; five-minute checks and fifteen-minute maximum observation age; ten-minute bounded webhook-secret rotation overlap; CMA-only event pilot and full configured goal sync. The freshness numbers are adjustable machine policy, not binding lifetime. Confirm changed test seams before implementing them. The implementation must replace the old proof-expiry checks with durable ownership and automatic reconciliation; withdrawing the loader is only the first step. The side-chat revision changed documentation only; subsequent main-chat implementation is tracked in section 1. The earlier GitHub-specific Kubernetes wiring remains withdrawn.

## Sources

Local code paths in section 2 were inspected on 2026-11-04; deployed behavior must still be verified separately.

- [G1: GitHub App webhook registration and secret scope](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/using-webhooks-with-github-apps)
- [G2: Raw-body webhook signature verification](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries)
- [G3: Installation setup URL is not ownership proof](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/about-the-setup-url)
- [G4: App identity, installation metadata and installation token creation](https://docs.github.com/en/rest/apps/apps)
- [G5: Scoped installation access tokens and expiry](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-an-installation-access-token-for-a-github-app)
- [G6: Installation repository inventory and pagination](https://docs.github.com/en/rest/apps/installations#list-repositories-accessible-to-the-app-installation)
- [G7: Installation and repository lifecycle payloads](https://docs.github.com/en/webhooks/webhook-events-and-payloads)
- [G8: Failed webhook delivery and redelivery behavior](https://docs.github.com/en/webhooks/using-webhooks/handling-failed-webhook-deliveries)
