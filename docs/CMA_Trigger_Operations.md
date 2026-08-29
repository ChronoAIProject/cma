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

Anonymous contributor-cost preview also remains readable during pricing-store
or rate-card unavailability. It returns `quote_unavailable` without an amount;
authenticated review remains fail-closed until it can calculate and pin a valid
current quote. An immutable zero-cost policy continues to return exact zero.

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

| Key                                             | Initial value | Operational meaning                                                                                                                                                                                                                       |
| ----------------------------------------------- | ------------: | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CHRONO_TRIGGER_ENABLED`                        |       `false` | Environment admission. `false` refuses new publisher and Launch admission and pauses existing orchestration at its next durable boundary.                                                                                                 |
| `CHRONO_TRIGGER_ORCHESTRATION_PAUSED`           |        `true` | Emergency freeze for new Launch effects and not-yet-dispatched prompts. It does not interrupt a session.                                                                                                                                  |
| `CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH`         |        `true` | Requests capability revocation and prevents renewal. It remains independent from orchestration admission.                                                                                                                                 |
| `CHRONO_TRIGGER_RUNTIME_CAPABILITIES`           |         empty | Exact comma-separated capabilities provided by the pinned runtime image. Empty fails Trigger admission closed while ordinary sessions remain available.                                                                                   |
| `CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG`     |         empty | Exact NyxID public-proxy service used only for credential-free repository metadata. Empty or unavailable makes a published preview return `repository_policy_unavailable`; it never selects the publisher's or contributor's UserService. |
| `CHRONO_TRIGGER_SANDBOX_BASE_URL`               |         empty | Absolute HTTP(S) origin of the CMA-exclusive native OpenSandbox server. Paths, credentials, queries, and fragments are rejected. Empty keeps the versioned Trigger adapter unregistered.                                                  |
| `CHRONO_TRIGGER_SANDBOX_NAMESPACE`              |         empty | Exact DNS-1123 workload namespace in which that authority creates `BatchSandbox` objects. It is also the boundary for discovery and fenced deletion and must be exclusive to CMA Trigger.                                                 |
| `CHRONO_TRIGGER_SANDBOX_API_KEY_FILE`           |         empty | Read-only mounted file containing only the dedicated provider key. `CHRONO_TRIGGER_SANDBOX_API_KEY` is rejected, including when the file is also configured; local development uses a temporary mounted/file path too.                    |
| `CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS`          |         empty | Comma-separated exact NyxID subject IDs allowed to publish or launch. Empty denies all.                                                                                                                                                   |
| `CHRONO_TRIGGER_REPOSITORY_IDS`                 |         empty | Comma-separated numeric upstream GitHub repository IDs. Names are not accepted. Empty denies all.                                                                                                                                         |
| `CHRONO_TRIGGER_USER_COHORT_PERCENT`            |           `0` | Stable SHA-256 subject cohort from 0 through 100. Zero admits no contributor.                                                                                                                                                             |
| `CHRONO_TRIGGER_CHANGED_FILE_LIMIT`             |        `1000` | Maximum files in one reviewed publication, bounded from 1 through the complete-review schema ceiling of 1,000. Retain pinned-provider measurement evidence for the selected deployment value.                                             |
| `CHRONO_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS` |         `300` | Persisted asynchronous fork-readiness deadline, bounded from 1 through 3,600 seconds. The five-minute default is GitHub's support-escalation threshold, not a provider completion guarantee.                                              |
| `CHRONO_TRIGGER_CLEANUP_MODE`                   |     `dry_run` | `dry_run` persists an exact plan but has no deletion port. `execute` sends eligible cleanup through the normal fenced effect worker.                                                                                                      |
| `CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS`        |           `5` | Cadence of each independent lane, 1 through 300 seconds. Missed ticks are skipped.                                                                                                                                                        |
| `CHRONO_TRIGGER_WORKER_PAGE_SIZE`               |          `50` | Candidate rows per page, bounded by the public core maximum.                                                                                                                                                                              |
| `CHRONO_TRIGGER_WORKER_MAX_PAGES`               |          `20` | Hard page ceiling per lane and pass, 1 through 100.                                                                                                                                                                                       |
| `CHRONO_TRIGGER_WORKER_CONCURRENCY`             |           `4` | Hard concurrent candidate ceiling per lane, 1 through 32.                                                                                                                                                                                 |

### Dormant sponsorship boundary (#671)

Native Layer 2 sponsorship models, encrypted Mongo persistence, REST routes,
and four secret-free TypeScript SDK page reads exist. Sponsorship remains
post-MVP because the checked-in binary wires `sponsorships: None` and composes
no provider, billing, pricing, scheduler, worker, or runtime adapter. Keep the
checked-in deployment state exactly as follows:

```text
CHRONO_SPONSORSHIP_ENABLED=false
CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED=true
all other CHRONO_SPONSORSHIP_* ConfigMap values are empty
no sponsorship secret file is mounted
```

An absent tuple and the exact empty ConfigMap declarations both resolve to no
sponsorship runtime. Once any non-boolean value is supplied, CMA requires every
field below, even when the complete tuple is staged with sponsorship disabled.
`enabled=false` requires the kill switch to remain engaged. This build rejects
every exact `CHRONO_SPONSORSHIP_ENABLED=true` value immediately, before reading
the kill switch, mounted secrets, or the remaining tuple, with this fixed error:

```text
CHRONO_SPONSORSHIP_ENABLED=true is not available in this build because sponsorship persistence and provider, billing, and pricing adapters are not composed; keep CHRONO_SPONSORSHIP_ENABLED=false and CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED=true
```

The refusal applies even to an otherwise complete domain-valid gate. Empty,
partial, malformed, mismatched, or above-ceiling disabled staging tuples also
fail process startup; ordinary Trigger configuration is not substituted.

| Configuration group | Required keys once staged |
| --- | --- |
| Fence and evidence | `CHRONO_SPONSORSHIP_EPOCH`, `CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST`, `CHRONO_SPONSORSHIP_EXPIRY_EVIDENCE_ID`, `CHRONO_SPONSORSHIP_REVOCATION_EVIDENCE_ID` |
| Provider policy | `CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY`, `CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS` |
| Domain gate ceilings | `CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS`, `CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS`, `CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES` |
| Expiry and public-abuse ceilings | `CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS`, `CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS`, `CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR`, `CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS`, `CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW` |
| Per-Launch and policy quotas | `CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS`, `CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE`, `CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS`, `CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS`, `CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS`, `CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS`, `CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS` |
| Adapter identities and origins | `CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID`, `CHRONO_SPONSORSHIP_PROVIDER_BASE_URL`, `CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID`, `CHRONO_SPONSORSHIP_BILLING_BASE_URL`, `CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID`, `CHRONO_SPONSORSHIP_PRICING_BASE_URL` |
| Reconciliation work | `CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS`, `CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE`, `CHRONO_SPONSORSHIP_WORKER_MAX_PAGES`, `CHRONO_SPONSORSHIP_WORKER_CONCURRENCY` |
| File-only secrets | `CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE`, `CHRONO_SPONSORSHIP_OPERATIONAL_KEYRING_FILE`, `CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE` |

The parser enforces these absolute deployment bounds. A selected policy may be
narrower. It may never widen them:

| Boundary | Accepted deployment range |
| --- | ---: |
| Epoch | 1 through signed 64-bit maximum |
| Provider-policy allowlist | 1 through 32 distinct canonical tokens; the trusted key must be a member |
| Default and maximum policy expiry | 60 through 86,400 seconds; default no greater than maximum |
| Active policies per sponsor | 1 through 32 |
| Launch-rate window | 60 through 3,600 seconds |
| Launches per sponsor per window | 1 through 100 |
| Concurrent Launches per policy | 1 through 32 |
| Requests per policy / per Launch | 1 through 10,000 / 1 through 1,000; per Launch no greater than policy |
| Requests per minute | 1 through 600 |
| Input or output tokens per policy / per Launch | 1 through 10,000,000 / 1 through 1,000,000; per Launch no greater than its policy dimension |
| Budget per policy / per Launch | 1 through 100,000,000 / 1 through 10,000,000 micros; per Launch no greater than policy |
| Worker interval/page/max-pages/concurrency | 1-300 seconds / 1-200 rows / 1-100 pages / 1-32 candidates |

Evidence identifiers are 8-255 byte opaque canonical identifiers. Adapter IDs
are 1-128 byte canonical policy tokens. Adapter addresses are exact HTTPS
origins with no credential, path, query, or fragment. The encryption,
operational-HMAC, and audit-HMAC keyrings must resolve from different mounted
files. Each file must be nonempty and no larger than 65,536 bytes, and the
three complete file payloads must be pairwise non-identical. Inline
`CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING`,
`CHRONO_SPONSORSHIP_OPERATIONAL_KEYRING`, and
`CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY` values are rejected.

Encryption-key retirement and operational-pseudonym retirement are separate
procedures. An AEAD predecessor may be removed only after every ciphertext
candidate has been rewrapped under the current encryption key. An operational
predecessor must remain configured while any credential, registration,
operation, acceptance, activation, grant, cleanup, or usage document contains
an `sp1` identity bearing that key id. CMA inventories those dependencies at
startup and refuses to open if a required operational predecessor is absent;
ciphertext rewrap does not authorize operational-key removal. Encryption and
operational key ids and bytes must never be reused across the two key classes.

`CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST` is the lowercase SHA-256 digest of
the domain's canonical JSON tuple, in this exact order:

```text
[
  "cma-sponsorship-gate-v1",
  enabled,
  expiry_evidence_id,
  revocation_evidence_id,
  sorted_allowed_provider_policy_keys,
  maximum_policy_budget_micros,
  maximum_policy_requests,
  maximum_concurrent_launches
]
```

The kill switch is deliberately fenced separately and is not part of this
digest. Changing `enabled` does change the digest. The configured epoch must
also match the future durable gate fence; never lower, reuse, or guess it.

Do not attempt activation from the current manifests. The implementation above
does not provide usable sponsorship, release or deployment evidence, or the
aggregate expiry/revocation and restart-recovery evidence required from
#445/#524/#528 and #675; it does not satisfy #671. The manifests name no reviewed
sponsorship Secret Manager resources and adding an unprovisioned object to the
mandatory CSI class would prevent a dormant CMA pod from starting. A future
activation change must first provide the reviewed evidence and compose the
sponsorship service with provider, billing, pricing, scheduler, worker, and
runtime adapters, replace the explicit enabled-state startup refusal, then add
three separately reviewed read-only secret projections and IAM grants, stage
the complete disabled/kill-switched tuple, and pass the post-MVP readiness ledger.

Provision the public GitHub observer as a dedicated credential-free NyxID
service, not as the ordinary credential-backed `api-github` service. Its closed
configuration has `identity_propagation_mode=none`,
`forward_access_token=false`, `inject_delegation_token=false`, no resale
billing, the smallest NyxID anonymous rule `GET /repositories/**`, and a
deployment-reviewed bounded daily quota. CMA narrows that wildcard locally to
exactly one nonzero decimal repository-ID segment and emits no query or body.
Before setting the slug, retain a live trace proving that neither a subject nor
any credential reaches the downstream service. Also exercise route absence,
quota exhaustion, malformed response, and transient failure; each must produce
`repository_policy_unavailable` without offering sign-in or invoking publisher
OAuth. Quota exhaustion therefore degrades published preview availability and
must be sized and alerted as a release dependency. Retired previews do not call
the observer.

### NyxID smoke bootstrap

The repository bootstrap is only for the reviewed non-production Trigger smoke
resources. It pins NyxID `0.10.0` and source revision
`245600cb15110c43aafed77c8a8ee73f655ac060`, rejects production CMA resources,
and performs no application or service-account secret rotation during ordinary
planning or apply. The only rotation path is the explicit, fenced recovery for
one exact ambiguous create described below. Start from
`.github/cma-trigger-nyxid-smoke-bootstrap.example.json`, choose unique staging
bucket names, and keep the state and generated handoff outside the repository in
an owner-only directory. The directory must have mode `0700`; existing state and
handoff files must be regular, singly linked, owner-owned files with mode
`0600`. Every path ancestor must resolve without a symbolic-link component and
must not be group- or world-writable, except for a root- or current-user-owned
sticky boundary such as the system temporary directory.

Run validation and the read-only remote plan before allowing any mutation. The
plan emits the pinned source/API identities, exact administrator subject,
existing or target resource owners and IDs, storage role/service IDs, GitHub
catalog ID, and bucket targets; it emits no credential:

```bash
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --validate-config .github/cma-trigger-nyxid-smoke-bootstrap.example.json

export CMA_TRIGGER_NYXID_ADMIN_TOKEN='<short-lived NyxID administrator bearer>'
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --plan .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json
```

After a human reviews that plan and confirms every target is staging-only, the
same idempotent tool may provision the exact observer, OIDC application, storage
service account, role binding, and two owner-bound buckets. The confirmation is
intentionally explicit:

```bash
export CMA_TRIGGER_NYXID_SMOKE_BOOTSTRAP_CONFIRM=provision-reviewed-nonproduction-resources
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --apply .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json

unset CMA_TRIGGER_NYXID_SMOKE_BOOTSTRAP_CONFIRM
```

Creating an OIDC application or storage service account returns its secret only
once. Immediately before either create POST, the bootstrap durably records a
closed `resource_creation` marker containing the resource kind, exact configured
name, administrator subject, start time, random attempt nonce, and canonical
request digest. A lost response, invalid response, or failure to persist returned
custody retains that marker. Ordinary apply, caller authorization, minting,
environment setup, and dispatch then fail with the resource-specific
`*_secret_creation_recovery_required` code and never resend the create.

First run `--plan` with the retained state. The marker-aware plan is read-only and
shows the full incident marker plus the one exact owner/name/config-matching live
resource when it is observable. Independently review the marker's
`attempt_nonce` and that provider resource's exact UUID. Then, and only then, run
the bounded recovery command with those two reviewed values:

```bash
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --plan .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json

export CMA_TRIGGER_NYXID_SMOKE_RESOURCE_RECOVERY_CONFIRM=rotate-reviewed-ambiguous-nonproduction-resource-secret
export CMA_TRIGGER_NYXID_SMOKE_RECOVERY_RESOURCE_ID='<reviewed exact provider UUID>'
export CMA_TRIGGER_NYXID_SMOKE_RECOVERY_ATTEMPT_NONCE='<reviewed 64-hex create-attempt nonce>'
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --recover-resource-creation .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json

unset CMA_TRIGGER_NYXID_SMOKE_RESOURCE_RECOVERY_CONFIRM
unset CMA_TRIGGER_NYXID_SMOKE_RECOVERY_RESOURCE_ID
unset CMA_TRIGGER_NYXID_SMOKE_RECOVERY_ATTEMPT_NONCE
```

Recovery rechecks the pinned contracts and exact administrator, recomputes the
create-request digest, requires exactly one matching listed resource, and reads
the independently supplied exact ID directly. Before its single rotation POST,
it durably records that ID, a rotation timestamp, and a fresh rotation nonce.
After the response it reads the exact ID again and requires the resource to
remain active, owner-bound, and configuration-matching. Only then may the new
secret be atomically adopted into state and the incident marker cleared. An
ambiguous rotation, failed post-rotation proof, or custody-persistence failure
retains a non-null `recovery_rotation`; that state categorically forbids a
second automatic rotation.

If the resource is absent, not yet observable, duplicated, mismatched, or not
readable through the exact owner authority, retain the marker and stop. Repeated
absence reads and elapsed local time cannot prove that an earlier timed-out
create will never commit. Neither condition permits editing or clearing the
marker, recreating the resource, or rotating some other ID. Resolution then
requires provider evidence that proves the exact request's final outcome or a
provider idempotency/final-outcome primitive.

If apply returns `bucket_provider_provisioning_required`, the live NyxID
administrator observation proved that an exact configured non-production
bucket is absent but the backing storage provider refused NyxID's create. Stop
there. The backing provider owner must create only the two exact configured
bucket names in the pinned account and region and attach evidence that both are
new and empty. The provider owner must not create an ownership marker, reuse an
existing production bucket, or broaden the runtime role. The bootstrap does not
retry the denied provider create, delete a conflicting bucket, or transfer a
bucket that already carries another owner marker.

After independent review of that provider evidence, the NyxID administrator can
adopt only those exact unowned buckets into the recorded storage service-account
identity. This explicit mode never calls provider bucket creation. It requires
the pinned storage OpenAPI ownership operation, refuses a visible foreign owner,
writes the exact owner UUID through the administrator route, then proves each
bucket is readable by the service account and contains no visible metadata or
objects:

```bash
export CMA_TRIGGER_NYXID_SMOKE_BUCKET_ADOPTION_CONFIRM=adopt-reviewed-empty-nonproduction-provider-buckets
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --adopt-provider-buckets .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json

unset CMA_TRIGGER_NYXID_SMOKE_BUCKET_ADOPTION_CONFIRM
```

`bucket_adoption_not_empty` means the now-owned target must be quarantined and
returned to the provider owner for exact recovery; do not use or clean it from
CMA. An ambiguous ownership response is reconciled through a service-account
read and is never blindly replayed. Re-run the read-only plan and ordinary
`--apply` after both adoption receipts pass.

Caller consent and storage provisioning are independent authority boundaries.
When the exact provider bucket path is awaiting its owner, establish only the
reviewed caller's refresh custody with `--authorize-caller`. This mode performs
the same PKCE, signed-token, exact-subject, exact-application, and exact-selected
UserService checks as final minting. Its contract inspection still reads the
storage service's public OpenAPI shape, but the mode neither mints storage
authority nor reads either bucket, writes a handoff, or claims the smoke is
dispatchable. It also clears any prior final-mint handoff binding, so an older
access-token file cannot remain dispatchable after authorization-only work:

```bash
export CMA_TRIGGER_SMOKE_SERVICE_ID='<exact personal api-github UserService UUID>'
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --authorize-caller .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json
```

The final `--mint-caller-token` remains mandatory. It invalidates the configured
output first, proves both buckets are readable by the exact storage service
account, rotates the retained refresh token, re-verifies caller authority, and
publishes one closed handoff. Authoritative state records the SHA-256 digest of
the exact serialized handoff bytes only after every preflight succeeds. The
operator hashes the bytes it loaded, requires the digest match, and uploads
those same bytes, so alternate whitespace, duplicate JSON keys,
authorization-only work, a failed storage check, a crash during publication, or
a legacy state file with no binding cannot reuse an older handoff. A confirmed
`--apply` also clears the binding before remote provisioning; run final mint
again even when apply reports no resource change.
NyxID rotates a refresh token before downstream signature and introspection
checks can finish. The tool therefore persists the replacement as `pending`
custody before those checks and promotes it to `verified` only after they pass.
A failed verification publishes no usable handoff; rerunning the same command refreshes
the retained replacement and retries transient verification without another
consent. A proven wrong subject or selected UserService is not treated as
transient: the tool requests revocation, then uses the refresh endpoint as the
pinned positive denial probe. Only the exact closed inactive response clears
local custody. A `200` means the probe rotated still-active custody; its
replacement is persisted as `pending` and fresh consent is refused. Ambiguous
refresh or probe delivery retries the predecessor once through NyxID's
replacement-chain grace; `caller_refresh_recovery_required` or
`caller_reauthorization_recovery_required` after that bounded retry requires an
immediate reviewed rerun and still publishes no usable handoff.

An authorization-code response can be lost after NyxID issued a refresh token
but before local custody exists. The code must not be replayed: pinned NyxID
treats replay as a broad client/user refresh-revocation event. Immediately
before the exchange, the bootstrap rechecks the pinned live `/health` build and
the exact administrator, then admits only a fresh first-party administrator JWT
whose subject and audience match that observation. Its `iat` and deployment TTL
are point-in-time evidence only while NyxID configuration changes are frozen;
they are not a source-pinned global ceiling. Before sending the exchange POST,
the bootstrap durably writes a closed `caller_exchange` incident marker. A
successful response is moved atomically from that marker into pending refresh
custody. Any lost or unretainable response leaves the marker in place and every
provisioning, authorization, mint, environment setup, and dispatch action fails
closed. Read-only environment planning and exact ephemeral-handoff cleanup stay
available under the shared lock solely to prevent an exchange incident from
stranding credential material.

On `caller_authorization_exchange_recovery_required`, stop. Retire only the
exact dedicated non-production app recorded by the marker:

```bash
export CMA_TRIGGER_NYXID_SMOKE_OIDC_RETIRE_CONFIRM=retire-reviewed-ambiguous-nonproduction-oidc-app
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --retire-ambiguous-caller-app .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json

unset CMA_TRIGGER_NYXID_SMOKE_OIDC_RETIRE_CONFIRM
```

This operation reconciles an ambiguous DELETE through an exact owner-authority
read and requires an inactive-app acknowledgement, but deliberately retains the
incident marker and reports `issuance_fence_proven: false`. App retirement
blocks later refreshes but is not an issuance fence: the pinned token endpoint
can finish an already admitted exchange after retirement and mint an access JWT
or new refresh custody. Do not clear the marker, clear local app custody,
reprovision the same name, open another consent, or claim revocation complete
until NyxID supplies an online inactive-client issuance/use fence or provider
logs prove the exact request completed with a bounded final outcome. Elapsed
local time, including the witnessed JWT TTL, is insufficient by itself.

`--apply`, `--adopt-provider-buckets`, `--authorize-caller`,
`--mint-caller-token`, and every stateful remote environment-operator mode
serialize state reads and authority use through the same adjacent owner-only
caller-authority lock. A concurrent owner fails closed. A complete same-host
lock whose PID is proven dead returns `caller_state_lock_stale`; it is never
removed automatically. Lock publication is atomic only after its payload is
synced. Inspect the exact adjacent file and host process, retain the evidence,
and remove only that one lock under an approved recovery. A malformed lock is
never guessed dead. The handoff output path may equal neither state nor this
adjacent lock path. Lexical aliases are rejected before lock acquisition, and
the same real-path and device/inode identity check runs again after the lock
exists to cover filesystem-specific Unicode and case folding. These fences
prevent a slower verification from promoting older custody and prevent stage
dispatch from racing a pending rotation.

Mint the caller handoff locally immediately before dispatching the protected
smoke. The first caller authorization, whether performed by `--authorize-caller`
or final minting, opens NyxID for unavoidable interactive login and consent as
`chronoai-shining`; `--no-browser` prints the authorization URL instead. Later
authorizations and mints refresh automatically. Refresh-token rotation is
persisted only in the authoritative local state file, so do not run this
command in a hosted runner or copy that state into CI. Set the separately
reviewed UserService ID before minting; the tool proves the signed access token
has restricted service authority equal to the exact singleton selected ID, with
no second service admitted. The OIDC app
preselects the `api-github` catalog entry, but consent remains visible and
user-controlled, so removing all selected GitHub connections makes minting fail
closed. Refresh custody is bound to the selected ID in local state; changing
the ID requires explicit revocation and a new consent rather than silently
retargeting an existing refresh token:

```bash
export CMA_TRIGGER_SMOKE_SERVICE_ID='<exact personal api-github UserService UUID>'
node .github/scripts/cma-trigger-nyxid-smoke-bootstrap.mjs \
  --mint-caller-token .github/cma-trigger-nyxid-smoke-bootstrap.example.json \
  --state /absolute/private/cma-trigger-smoke/state.json \
  --output /absolute/private/cma-trigger-smoke/handoff.json

unset CMA_TRIGGER_NYXID_ADMIN_TOKEN
```

The handoff schema is closed at these 13 fields; unknown or missing fields are
rejected:

```json
{
  "schema_version": 1,
  "nyxid_base_url": "https://nyx-api.chrono-ai.fun",
  "nyxid_client_id": "<OIDC application UUID>",
  "nyxid_client_secret": "<OIDC application secret>",
  "storage_client_id": "<sa_ client ID>",
  "storage_client_secret": "<storage service-account secret>",
  "storage_owner_service_account_id": "<storage service-account UUID>",
  "context_bucket": "<staging context bucket>",
  "trigger_workspace_bucket": "<staging workspace bucket>",
  "public_github_service_slug": "<credential-free observer slug>",
  "subject_id": "<chronoai-shining NyxID UUID>",
  "access_token": "<fresh restricted bearer>",
  "access_token_expires_at": 1780000000
}
```

Do not upload the handoff or UserService ID with ad hoc `gh secret set`
commands. The reviewed environment operator at
`.github/tools/cma-trigger-smoke-environment/operator.mjs` is the only writer
for the protected smoke environment. It is pinned to the live CMA repository,
environment, workflow, `develop` protection, NyxID source/API version, GitHub
catalog service, `chronoai-shining` account identity, and the closed managed
variable/secret inventory. Its provisioning bearer comes only from
`CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN` or an owner-only file named by
`CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE`.

First validate the environment config and generate a ten-minute, read-only
plan. The bootstrap state is the authoritative state produced above; the
operator neither discovers nor auto-selects a runtime UserService. Use absolute
owner-only paths, keep the custody directory at mode `0700`, and review the
entire plan plus its approval fingerprint before applying it:

```bash
operator=.github/tools/cma-trigger-smoke-environment/operator.mjs
environment_config=.github/tools/cma-trigger-smoke-environment/config.example.json
bootstrap_state=/absolute/private/cma-trigger-smoke/state.json
custody=/absolute/private/cma-trigger-smoke/environment-custody
setup_plan=/absolute/private/cma-trigger-smoke/environment-setup-plan.json

mkdir -m 0700 -p "$custody"
export CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE=/absolute/private/nyxid-admin-token
node "$operator" --validate-config "$environment_config"
node "$operator" --plan "$environment_config" \
  --state "$bootstrap_state" --output "$setup_plan"
```

After explicit review, the same approved plan may configure the closed static
variables and stable Kind observer tuple. A changed or expired plan is refused;
generate and review a new plan rather than editing the old file.

```bash
export CMA_TRIGGER_SMOKE_ENV_CONFIRM=setup-reviewed-cma-trigger-staging
node "$operator" --setup "$environment_config" \
  --state "$bootstrap_state" --approved-plan "$setup_plan" \
  --custody "$custody"
unset CMA_TRIGGER_SMOKE_ENV_CONFIRM
```

Mint the 13-field handoff immediately before the run, then generate a fresh
operator plan because setup and caller minting changed the observed inventory.
`--stage-run` proves the caller token belongs to the configured subject and
exact personal UserService, has only `public_repo`, `read:user`, and
`user:email`, differs from the provisioning route, and has the bounded lifetime
required by the workflow. It uploads the exact UserService ID separately,
encrypts the closed handoff for the protected environment, dispatches the exact
`develop` workflow inputs including the full lowercase reviewed head SHA,
identifies one fenced run, watches it to completion, and deletes and
absence-verifies only the ephemeral handoff in `finally`. The workflow's
unprotected plan job refuses the run unless that required SHA is well formed
and exactly equals the dispatch event's `github.sha`; this happens before the
protected provider job can be queued, so a moved `develop` branch cannot
silently retarget an approved run.

The environment operator accepts legacy caller state without an explicit
verification marker as pre-migration `verified`, but legacy state has no current
handoff binding and therefore cannot dispatch until a new final mint. It refuses
`pending` custody or a missing/mismatched digest before validating or uploading
any handoff. A previously minted, still-fresh handoff cannot bypass a later
authorization, apply, failed bucket check, caller rotation, or publication
failure.

```bash
run_plan=/absolute/private/cma-trigger-smoke/environment-run-plan.json
handoff=/absolute/private/cma-trigger-smoke/handoff.json
node "$operator" --plan "$environment_config" \
  --state "$bootstrap_state" --output "$run_plan"

export CMA_TRIGGER_SMOKE_ENV_CONFIRM=dispatch-reviewed-cma-trigger-staging
node "$operator" --stage-run "$environment_config" \
  --state "$bootstrap_state" --approved-plan "$run_plan" \
  --custody "$custody" --handoff "$handoff"
unset CMA_TRIGGER_SMOKE_ENV_CONFIRM
```

`SIGHUP`, `SIGINT`, and `SIGTERM` close dispatch and handoff cleanup through the
same fence. A hard process or host loss can leave the encrypted handoff present;
that is recovery work, never permission to dispatch again. Generate and review
a fresh plan, then run the bounded cleanup command. It deletes only
the single exact
`CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_<UPPERCASE_DISPATCH_NONCE>` secret; the two
stable observer secrets and the separately validated runtime UserService ID
remain unchanged. Planning and this bounded cleanup remain available when a
`caller_exchange` or `resource_creation` incident marker is present; setup and
dispatch do not. Cleanup cannot clear or modify either bootstrap incident.

```bash
cleanup_plan=/absolute/private/cma-trigger-smoke/environment-cleanup-plan.json
node "$operator" --plan "$environment_config" \
  --state "$bootstrap_state" --output "$cleanup_plan"
export CMA_TRIGGER_SMOKE_ENV_CONFIRM=cleanup-reviewed-cma-trigger-staging
node "$operator" --cleanup "$environment_config" \
  --state "$bootstrap_state" --approved-plan "$cleanup_plan"
unset CMA_TRIGGER_SMOKE_ENV_CONFIRM
```

Only after independent environment approval may the credential-bearing job
decode the handoff. It materializes it under mode-`0700` runner custody with a
mode-`0600` file, validates the closed schema and exact protected UserService,
and masks extracted values. The integer `access_token_expires_at` is mandatory;
both minting and post-approval acceptance require at least 7,200 seconds of
remaining life and reject a bound over 28,800 seconds. Runner cleanup separately
destroys Kind, settles the current run's exact live logical provider objects,
removes the observer kubeconfig, and purges all local staging custody. Failure of
the operator's remote handoff cleanup, provider-object settlement, or runner
local cleanup is an incident.

The observer, OIDC app, storage service account, and two buckets are stable,
reusable staging resources, not per-run resources. This is deliberate: rolling
them back after a later step fails can destroy one-time-secret custody, invalidate
refresh rotation, or hide an ambiguous provider write. State is persisted after
each proven creation. On an ordinary failure, retain state, run `--plan` again,
and resume `--apply`; never delete or recreate a resource just to make a retry
look clean. For `*_secret_creation_recovery_required`, stop automatic retries,
retain the state verbatim, and use only the exact-ID
`--recover-resource-creation` procedure above. Never rotate through an ad hoc
administrator action or edit secret custody directly into the state file.

`oidc_secret_rotation_required` or `storage_secret_rotation_required` without a
`resource_creation` marker is a distinct, unproven-custody incident. The exact
resource existed before this bootstrap could durably bind its create attempt,
so the marker-bound recovery command is intentionally unavailable. Quarantine
the resource and escalate for provider final-outcome and ownership evidence plus
a separately approved retirement or recovery design. Do not rotate it ad hoc,
invent a marker, edit custody into state, or delete and recreate it merely to
resume this smoke.

The dedicated context and Trigger-workspace buckets outlive each Kind run. They
must not use a blanket provider lifecycle rule. At pinned `chrono-bucket` source
revision `4087662e2a65e3747b3203ba619eca7342089272`, fenced live objects and
durable deletion tombstones occupy reserved mutation-authority keys; tombstones
remain for the bucket lifetime, and an S3 lifecycle rule must never expire or
rewrite that reserved namespace. The protected smoke instead settles its own
live logical objects through the authenticated storage API after Kind
destruction has proved that every writer is gone. Hidden tombstones are retained
as zero-byte security authority, not misreported as provider deletion debt.

Retirement is a deliberate administrator procedure, not smoke cleanup. First
disable dispatch and delete the protected handoff secret. Using IDs from the
state and a fresh read-only plan, prove both exact buckets are owned by the exact
staging service account and enumerate them completely. Preserve or explicitly
dispose of every retained smoke object; delete a bucket only after an empty
inventory. Then revoke the storage account's tokens and remove the exact account,
revoke/delete the exact OIDC app and its consents, and remove the observer only
after its owner, slug, and sole anonymous rule still match the state and config.
Confirm all five remote resources are absent before removing local handoff and
state custody. A partially completed retirement is resumed in that order from
fresh observations; no name-only bulk deletion is allowed. None of these
commands, artifacts, or retirement steps is approved for a production rollout.

### Ephemeral Kind provider smoke

The protected provider workflow tests the current `develop` checkout in a fresh
single-node Kind cluster on `ubuntu-latest`; it does not call a deployed CMA or
reuse a production OpenSandbox lane. It builds the API and runtime images from
the exact workflow commit, loads the runtime image into Kind, creates separate
ordinary and Trigger OpenSandbox servers and namespaces, initializes a MongoDB
replica set, and exposes CMA only at the locally trusted fixed origin
`https://127.0.0.1:8443`. The Trigger fixture is published after that deployment
is ready, so its public ID, revision digest, source SHA, build SHA, runtime image
ID, runtime version, and configuration revision all come from the same run.

The workflow itself has read-only GitHub permissions. Every referenced action
is pinned to one reviewed 40-character commit; tags such as `@v4`, `@v5`, or
`@v6` are not accepted by the source contract. The credential-bearing job is
behind the non-bypassable `cma-trigger-smoke` environment, requires an
independent reviewer who is not the triggering operator, and rechecks the live
environment rules after approval before reading any protected value. Both
checks require the one exact `AI Application` team identity (team ID
`15748251`, node ID `T_kwDODyHfns4A8Eyb`, slug `ai-application`) and exactly one
branch-policy rule; a merely nonempty reviewer list is not sufficient. Its
required `expected_head_sha` dispatch input must be a full lowercase commit ID
equal to the dispatch event's `github.sha`; a branch-name-only dispatch never
reaches the protected job if `develop` moved away from the reviewed snapshot.

The environment operator provisions these protected variables from the closed
`static_variables` object in its reviewed config:

- `CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE`: one dedicated DNS-1123 label, normally
  `cma-trigger-sandboxes`.
- `CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID`, `_OWNER`, `_REPOSITORY`, and
  `_DEFAULT_BRANCH`: the exact persistent public fixture repository.
- `CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN`: `chronoai-shining` for the initial
  owner-authorized campaign.
- `CMA_TRIGGER_SMOKE_FORK_REPOSITORY`, `_MARKER_INPUT`,
  `_EXPECTED_PATH_TEMPLATE`, and `_EXPECTED_CONTENT_TEMPLATE`: the closed
  preserved-contribution fixture values.

The setup command also generates and retains one closed observer-authority
directory under local custody. It contains the 30-day CA, matching CA key,
client-only `cma-trigger-smoke-observer` certificate and key, canonical
certificate-only kubeconfig for `https://127.0.0.1:6443`, a digest manifest,
and the acknowledged environment receipt. The operator uploads the kubeconfig
as `CMA_TRIGGER_SMOKE_KUBECONFIG_B64`, the matching CA key as
`CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64`, and the lowercase kubeconfig SHA-256 as
`CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256`. No manual reconstruction or partial
upload is permitted.

These stable values are reusable only for this bounded non-production evidence
campaign and never for a real cluster. Keep the complete local authority and
receipt at mode `0700`/`0600`; every later setup and run revalidates their bytes,
digests, certificate purposes, fixed subject/endpoint/namespace, environment
public-key identity, and acknowledged secret timestamps. Losing local custody,
editing one file, nearing the operator's three-day certificate floor, or seeing
a stable secret change is a fail-closed rotation/incident condition. Do not
delete one member or generate replacement bytes by hand; use a separately
reviewed whole-tuple retirement and replacement procedure before another
dispatch.

After approval, the workflow executes this fixed phase order:

1. Validate the observer kubeconfig's exact endpoint, digest, closed schema,
   certificate chain, subject, private-key match, and matching Kind CA key.
2. Build images, create Kind, install pinned OpenSandbox CRDs and images, create
   separate provider authorities, initialize MongoDB, and load the exact runtime
   image ID.
3. Only after provider preparation succeeds, decode and validate the closed
   13-field NyxID handoff, bind the exact UserService ID, mask extracted
   protected values, and retain the bearer only under runner custody.
4. Deploy CMA, then detach and remove the Kind CA key from the node and shred
   the host CA key and administrator kubeconfig before publishing the Trigger.
5. Prove the observer is list-only in the exact Trigger namespace, publish the
   run-owned fixture, execute the real GitHub/NyxID path, and audit closed
   evidence.
6. Under `always()`, destroy Kind and prove no labeled node remains. With every
   writer then quiesced, use the state-bound storage owner to enumerate both
   dedicated buckets and select only this evidence record's exact session keys
   and owner-hashed Launch prefix. Delete generation-zero objects through the
   legacy logical route and positive-generation objects at generation plus one;
   validate each closed acknowledgement, re-enumerate, and require zero current
   run live objects. Preserve and count any unrelated live object, which blocks
   a passing run. Then remove the observer kubeconfig, purge runner
   credential/TLS custody, and validate the local cleanup receipt. Redacted
   evidence is uploaded only after all of this succeeds.

Any phase failure stops forward progress. Cleanup still runs from a validated
ownership marker. If cluster destruction or observer removal cannot be proven,
credential purge refuses to proceed and the workflow fails rather than claiming
terminal local cleanup. GitHub resources created by the smoke remain preserved by
design for inspection; this workflow never deletes the contributor fork,
branch, commit, or pull request. The harness first proves the CMA session is
idle and its runtime is gone, then terminal Kind destruction removes the
ephemeral MongoDB volume and therefore the local CMA Launch and session records.
The fixture policy states this local-state split explicitly as
`github_cleanup=preserve`,
`cma_state_cleanup=destroy_with_terminal_ephemeral_environment`, and
`session_cleanup=stop_runtime_before_terminal_ephemeral_teardown`; none is a
claim that provider objects were deleted.

The staging CMA itself remains in `CHRONO_TRIGGER_CLEANUP_MODE=dry_run`; its
in-process planner therefore has no object-store deletion port. The
generation-aware settlement above is owned by the terminal ephemeral-environment
harness, runs only after Kind destruction proves writer quiescence, and is
restricted to the current run's exact logical namespaces. It is not the CMA
retention worker, does not broaden a production deletion target, never touches
GitHub, and never enumerates a sandbox.

Before teardown, evidence may state
`retained_until_terminal_ephemeral_teardown`,
`terminal_ephemeral_teardown_pending`, or
`idle_awaiting_terminal_ephemeral_teardown`; each describes only local state.
After Kind destruction, observer removal, and custody purge produce the closed
local cleanup receipt, the workflow finalizer changes every created local CMA
Launch/session state to
`destroyed_with_terminal_ephemeral_environment` and revalidates the evidence.
Upload is gated on that finalization and on a closed provider-object settlement
receipt. Publishable evidence records hashes of both stable bucket identities,
the exact storage-owner binding, current-run object counts and observable bytes,
legacy and fenced deletion acknowledgements, unrelated live-object counts, and
the final zero-live-object observation. It also states that fenced tombstones are
not logically enumerable and remain for the bucket lifetime. A missing or
ambiguous delete, residual current-run object, unrelated live object, owner
drift, or schema drift leaves cleanup evidence open and fails the run.

The terminal runner cleanup receipt is closed at `schema_version=2`. Alongside
the run/attempt, destroyed-cluster, and purged-custody proofs, it contains
exactly one `provider_object_settlement` value: the redacted receipt for a
passing smoke, or `null` when the provider path did not pass and no remote
settlement was attempted. A passing evidence document cannot finalize from the
`null` form.

The Trigger provider origin, namespace, and key are one authority tuple. They
must be either all absent or all present; a partial tuple fails startup. When
`CHRONO_TRIGGER_ENABLED=true`, absence also fails startup. A complete tuple does
not reuse the ordinary provider, an Aevatar tenant, another product's workload
namespace, or another product's key. The namespace must be CMA-exclusive because
Kubernetes RBAC cannot constrain `delete` by object label. A production API key
never belongs in a ConfigMap, environment variable, or ordinary Kubernetes
`Secret`. The ephemeral Kind smoke generates unrelated per-run provider keys,
places them only in its disposable local cluster, and purges their custody after
cluster destruction.

CMA reads both provider key files only during process startup. Rotating the
Secret Manager source may update the CSI projection, but CMA does not reread it;
verify the projected version and perform a controlled pod restart before
claiming the new key is active.

### Dedicated provider activation

Adopt the native lane in this order. Do not run an unordered
`kubectl apply -f infra/api/`:

1. Provision and independently verify a CMA-owned OpenSandbox tenant/server,
   dedicated API key, native runtime template, and CMA-exclusive workload
   namespace. Record the external infrastructure owner and reviewed revision.
2. Prove the final runtime image and template satisfy D-09 without privileged
   mode, added capabilities, host networking, disabled Codex sandboxing, or a
   global AppArmor/user-namespace relaxation.
3. Apply a `Role` in the exact workload namespace granting only `get`, `list`,
   and `delete` on `sandbox.opensandbox.io/batchsandboxes`, then bind it locally
   to `chrono-bot/chrono-bot-ksa`. Do not substitute a cluster-wide grant.
4. Provision the dedicated Secret Manager resource and grant the workload
   identity access to only that secret.
5. Apply the dedicated `SecretProviderClass` and non-overlapping read-only CSI
   mount, verify the projected file, then configure the complete provider tuple
   while Trigger remains disabled, orchestration remains paused, the capability
   kill switch remains enabled, and runtime capabilities remain empty.
6. Restart CMA deliberately and verify that `codex-trigger-native-v1` registers,
   ordinary `codex` sessions still use their original lane, and provider-side
   inventory contains zero unexpected Trigger objects.
7. Run the provider-route, D-09, fenced-teardown, restart/recovery, and
   post-stop inventory campaigns against the exact deployment revision.
8. Declare demonstrated runtime capabilities, then advance the environment
   flag, orchestration pause, kill switch, repository/publisher allowlists, and
   user cohort one control at a time. Roll back the last control on any failure.

### Provider-smoke inventory authority

Do not give the real-provider smoke CMA's production reclaimer identity. The
production Role in activation step 3 legitimately grants CMA `get`, `list`, and
`delete` so the session lifecycle can discover and tear down its own
`BatchSandbox`. The smoke observer is a separate identity. Its namespace-local,
list-only Role contains exactly this one rule:

```yaml
apiGroups: ["sandbox.opensandbox.io"]
resources: ["batchsandboxes"]
verbs: ["list"]
```

Create both that Role and its RoleBinding in the exact Trigger workload
namespace, and bind only the observer identity represented by the smoke
kubeconfig. Do not use a `ClusterRole` or a cluster-wide binding. Before the
smoke runs, its authorization check must prove that the identity can list
`batchsandboxes.sandbox.opensandbox.io` only in that namespace; it must reject
`get`, `watch`, `create`, `update`, `patch`, `delete`, and `deletecollection`, a
cluster-wide `BatchSandbox` list, and namespace-local listing of `secrets`,
`configmaps`, or `pods`.

Store the exact namespace as the protected environment variable
`CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE`. It must equal the ephemeral deployment's
`CHRONO_TRIGGER_SANDBOX_NAMESPACE` and the closed fixture field
`trigger_sandbox_namespace`; it is a reviewed deployment binding, never a
dispatch input. The workflow accepts the observer kubeconfig only with its
separately protected SHA-256 and matching Kind CA key. It rejects any endpoint
other than `https://127.0.0.1:6443`, any subject other than the exact observer
CN, and token or `exec` authentication. The pinned `kubectl` v1.34.1 uses the
mode-`0600` file at `${{ runner.temp }}/cma-trigger-smoke-kubeconfig` only after
the fresh cluster consumes the matching CA. Under `always()`, terminal cleanup
removes the observer after evidence audit and before the staging credential tree
is purged. Kubeconfig bytes, client keys, and client certificates must not enter
fixture configuration, logs, or retained evidence.

The smoke takes a complete, unpaginated namespace inventory and rejects any
continuation token. A pass requires this authoritative sequence:

| Observation     | Total | Entitled | Unexpected |
| --------------- | ----: | -------: | ---------: |
| `before_launch` |     0 |        0 |          0 |
| `live_session`  |     1 |        1 |          0 |
| `after_stop`    |     0 |        0 |          0 |

The live object must match `opensandbox.io/id`, `cma/kernel`,
`chrono-bot/kernel`, `cma/session`, `chrono-bot/session`,
`cma/runtime-generation`, and `cma/runtime-reservation`. The evidence retains
only the allowlisted API version and kind, namespace and name, UID, object and
list `resourceVersion` values, creation and deletion timestamps, and those
allowlisted label values. It replaces the raw runtime reservation with its
SHA-256 fingerprint; the raw value must never be serialized. `counts.sandbox`
is derived from `live_session.entitled_objects.length`, not from CMA Session
state. After session stop, the observer polls until the namespace is empty and
fails with `sandbox_leaked_after_stop` at the reconciliation deadline. Normal
session stop is the sole sandbox teardown authority; the observer never mutates
the object.

The public MVP's Trigger kernel is not a contributor or publisher control. The
composition root authoritatively pins it to Codex, every Launch review discloses
`kernel: codex-trigger-native-v1`, and Start/provisioning fail closed if that
selection drifts. The versioned name also pins the dedicated native provider
lane; ordinary sessions retain `kernel: codex`. Runtime version strings and
capability lists are never parsed to infer either identity.

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

For every public-MVP preflight and irreversible GitHub boundary, require the
fresh canonical `api-github` catalog service ID, the exact contributor
UserService ID, its current NyxID-assigned slug and resource URI, and personal
OAuth custody to match the retained authority tuple. GitHub requests must have
the wire shape
`/api/v1/proxy/{catalog_service_id}/{github_path}?_nyxid_via={user_service_id}`.
The per-user slug is routing evidence only; a matching custom/PAT slug, a slug
route, or a UserService-ID direct route is never acceptable authority. Also
require `public_repo`, permit only ancillary `read:user` and `user:email`, and
stop on `trigger_scope_unapproved` for `repo`, `workflow`, any organization,
admin, write, delete, package or gist scope, or any unknown scope.

### Trigger runtime listener boundary

Codex permission-profile host rules do not scope ports. The Trigger profile
therefore allowlists exactly `127.0.0.2` and the runtime reserves every
privileged port on that address that could otherwise expose a credential or
control endpoint:

| Listener                      | Purpose                                    | Sandbox result                                        |
| ----------------------------- | ------------------------------------------ | ----------------------------------------------------- |
| non-loopback pod IP on `9090` | Actual runtime supervisor ingress          | Denied by the managed network allowlist               |
| `127.0.0.1:9091`              | Actual model credential proxy              | Denied by the managed network allowlist               |
| `127.0.0.2:9090`              | Closed supervisor-denial front             | `403` with `x-cma-trigger-loopback-isolation: denied` |
| `127.0.0.2:9091`              | Closed model-proxy-denial front            | `403` with `x-cma-trigger-loopback-isolation: denied` |
| `127.0.0.2:9092`              | Signed Layer 4 platform and GitHub gateway | Permitted only after exact signed-policy evaluation   |

A Trigger runtime must resolve a non-loopback ingress address and must have
both model and platform upstreams. Wildcard or loopback supervisor ingress,
missing upstream configuration, or a collision on any reserved listener exits
startup. The supervisor does not bind and expose `/readyz` until both denial
fronts and both credential proxies have bound successfully, so a partial
topology is never Ready.

Every Reach activation runs the pinned `runtime_gateway_probe_v2` through the
same Codex command sandbox that will receive the first prompt. The probe must
positively observe both closed denial fronts, a `blocked-by-allowlist` denial
for an unlisted private loopback address and a public destination, and an
allowed numeric-resource read through `127.0.0.2:9092` using the complete signed
catalog ID, UserService ID, per-user slug, and repository-ID tuple. A wrong
catalog, UserService, slug, repository, or policy generation is denied before
the helper is spawned. Connection refusal, missing or different headers,
malformed receipts, or any broader reach is a typed failure and no prompt is
sent. Release evidence must additionally show that raw `CONNECT` cannot tunnel
to the real supervisor or model proxy.

The built-image campaign also sends raw sandboxed requests through
`127.0.0.2:9092` using `/proxy/s/` with the canonical `api-github` slug, a
NyxID-suffixed GitHub slug, a PAT-looking slug, the exact UserService ID, and the
catalog service ID. Every form must return `403`/`policy_denied` with zero
provider calls. The only GitHub transport admitted is the signed catalog-ID
route with the exact `_nyxid_via` selector.

The MVP runtime permits non-GitHub slug passthrough only for `chrono-llm`,
`ornn-api`, and `ornn`. That list is deliberately frozen above credential
injection. Treat any baseline-service change as a runtime protocol change: keep
Trigger rollout closed, update the Layer 4 closed set and its tests, and attach
new D-09 evidence before admitting a Launch.

The pinned image carries the distribution `bubblewrap` at `/usr/bin/bwrap` so
Codex does not fall back to its anonymous bundled helper. Following the
[official Codex Ubuntu 24.04 sandbox setup](https://developers.openai.com/codex/sandboxing),
the D-09 runners must install `apparmor-profiles` and `apparmor-utils`, copy
`/usr/share/apparmor/extra-profiles/bwrap-userns-restrict` to
`/etc/apparmor.d/bwrap-userns-restrict`, and load it with `apparmor_parser -r`
before starting the campaign.

Only the disposable D-09 runtime container receives the reviewed outer
`seccomp=unconfined`, `apparmor=unconfined`, and `systempaths=unconfined`
options. They allow `/usr/bin/bwrap` to transition from Docker's outer policy
and establish its nested namespace and `/proc`; they do not grant capabilities,
privileged mode, host networking, or permission to bypass the mandatory inner
`cma-trigger` Codex sandbox. A command-sandbox initialization refusal is a
provisioning failure. Disabling the global AppArmor user-namespace restriction
or running Codex unsandboxed is prohibited.

Mount writable workspace storage at `/home/chrono/workspace` and keep the
Trigger repository root at `/home/chrono/workspace/repository`, as supplied by
the dedicated adapter. Do not mount a volume directly on the repository root:
Layer 4 verifies a restore in a sibling staging directory and atomically
renames it into place, and Linux cannot replace an active mount point.

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
   by the pinned runtime image only after linked, independently reviewed `D-09`
   evidence passes for that exact deployed image digest. Then verify one
   supported and one deliberately omitted-capability review. Add one approved
   publisher subject and one disposable public fixture's numeric repository ID.
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

A historical Trigger record missing the v2 catalog/UserService route tuple is
readable evidence only. It must not be restarted, readopted, rotated, recovered,
or renewed. Replace or explicitly revalidate a pre-release v1 canary before
opening rollout; never infer the missing binding from a slug.

## 5. Pause and kill-switch matrix

An external call already in progress may finish or become ambiguous and enter
reconciliation. A flag cannot recall bytes already written. The next effect is
always claimed only after a durable Launch lease and a fresh rollout decision.

| Launch state                                                                                                           | Orchestration pause                                                                                                                                                                                                                                                                                         | Capability kill switch                                                                                                                                                              | Session and user work                                                                                                                                                                                                                                                                                |
| ---------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `draft`                                                                                                                | No new Start or provider effect. Existing review remains readable.                                                                                                                                                                                                                                          | Normally no reach exists to revoke.                                                                                                                                                 | No session is created or stopped.                                                                                                                                                                                                                                                                    |
| `auth_required`                                                                                                        | Keep the failed effect on its original intent. Successful connection revalidation queues read-only reconciliation of that intent and returns to `resume_state`; it never repeats Start or blindly executes the provider write again.                                                                        | Revoke retained reach if one exists and the failure requires it.                                                                                                                    | Any existing session and workspace remain intact; recovery creates neither.                                                                                                                                                                                                                          |
| `preflight`, `fork_pending`, `fork_ready`, `branch_pending`, `workspace_pending`, `sandbox_pending`, `session_pending` | Finish only the external boundary already in progress, retain its result, then persist `feature_paused` before another claim.                                                                                                                                                                               | Persist `revocation_pending`, then seek tombstone or agent-key acknowledgement.                                                                                                     | A session that already exists stays intact; no inferred rollback deletes earlier resources.                                                                                                                                                                                                          |
| `prompt_pending`                                                                                                       | Do not claim a not-yet-dispatched prompt. A committed dispatch fence follows the seven-state at-most-once recovery contract and is never blindly resent.                                                                                                                                                    | Revoke subsequent GitHub reach without ending the session.                                                                                                                          | Conversation, workspace, and visible history remain.                                                                                                                                                                                                                                                 |
| `ready`                                                                                                                | No new automatic GitHub/checkpoint effect is claimed.                                                                                                                                                                                                                                                       | Stop renewal and converge to effective revocation.                                                                                                                                  | The session remains available for conversation, inspection, export, and recovery subject to the now-revoked GitHub reach.                                                                                                                                                                            |
| `contributing`, `pr_pending`                                                                                           | Freeze the next effect. Reconcile a possibly written checkpoint, branch, or PR before settling.                                                                                                                                                                                                             | Stop subsequent session GitHub use; CMA-owned ambiguous writes still reconcile under their retained intent.                                                                         | Workspace, fork, branch, commits, and PR are preserved.                                                                                                                                                                                                                                              |
| `pr_open`                                                                                                              | No provider status read or write is claimed while rollout is paused. When permitted, fenced read-only status observation and explicit same-Launch Update resume; CMA never merges, reopens, or rewrites the PR automatically. Contributor-confirmed completion is a CMA-only handoff with no provider call. | A kill switch revokes remaining session reach and therefore prevents further agent GitHub work without ending the session. Completion also converges remaining reach to revocation. | Until completion, an open PR remains updateable through the reviewed flow. Contributor-confirmed handoff of an open PR or a positively observed merge advances to `completed`; either path preserves the fork, branch, commit, and PR. Closed-unmerged state remains readable and offers New Launch. |
| `failed_retryable`                                                                                                     | Preserve retry time and durable intent but do not retry while denied.                                                                                                                                                                                                                                       | Revoke reach independently.                                                                                                                                                         | Recovery information and session linkage remain visible.                                                                                                                                                                                                                                             |
| `completed`, `failed_terminal`, `cancelled`                                                                            | No ordinary orchestration. Retention execution is also frozen in `execute` mode until rollout permits it.                                                                                                                                                                                                   | Converge any remaining reach to revocation before cleanup eligibility.                                                                                                              | GitHub work and sandboxes are never selected by Trigger cleanup.                                                                                                                                                                                                                                     |

An existing `owner_credential_unavailable`, `service_inactive`,
`session_rotating`, `session_recovering`, `recovery_required`, or
`revocation_pending` block is not overwritten by a rollout pause. Only a block
that was durably recorded as `feature_paused` is automatically resumed when the
same Launch becomes permitted again. Activity actions
`trigger_orchestration_pause` and `trigger_orchestration_resume` carry the
stable `rollout_reason` values used by support tooling.

## 6. Scheduler and structured telemetry

The process runs four independent interval loops:

| Lane           | Candidate source                                              | Worker                                                                   |
| -------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `effects`      | Active Launch states                                          | Normal fenced effect execute/reconcile worker                            |
| `capabilities` | Launches that may hold short-lived reach                      | Fenced renew/revoke worker                                               |
| `workspaces`   | Trigger Launches with a live runtime and checkpoint authority | Periodic coherent-checkpoint worker; paused orchestration defers capture |
| `cleanup`      | Terminal Launches past `workspace.delete_after`               | Plan-only worker in `dry_run`; normal effect worker in `execute`         |

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

The same event carries `event_name=cma_trigger_scheduler_pass` and
`schema_version=1`. Every candidate emits
`event_name=cma_trigger_scheduler_candidate` with the closed `lane` and
`outcome` values plus `elapsed_milliseconds`. Log-based metric extraction may
use only the closed `lane` and `outcome` dimensions; Launch and correlation IDs
remain exemplars for investigation, never metric labels. This makes pass count,
candidate count, outcome rate, and latency distributions executable without
introducing a second telemetry transport.

A raw threshold observation emits
`event_name=cma_trigger_alert_candidate`, `schema_version=1`, the stable
`alert_id`, severity, lane, and bounded pass counters. The process emits
candidates for scheduler failure/page exhaustion, majority lease contention
without completion, closed provider outage/rate-limit failures, capability
failure, and cleanup failure/debt. Deployment-owned rules apply the sustained
windows in section 7; an emitted candidate is not by itself a delivered page or
release evidence.

The activity recorder emits a separate info-level
`event_name=cma_trigger_activity_counter`, `schema_version=1`, `increment=1`
event after each successful `TriggerLaunchCreate` or
`TriggerLaunchPullRequestOpen` request. Its only differentiating field is the
closed `activity_counter` value `launch_create_succeeded` or
`pull_request_open_succeeded`; it carries no subject, Launch, repository,
provider, request, or correlation identifier. A deployment rule sums each
counter independently over completed five-minute windows and compares it with
the absolute rollout envelope approved in `O-02`. The envelope is an integer,
not a learned unbounded baseline, and must be installed before a nonzero public
cohort is allowed.

These are accepted control-plane request counts, not unique-resource counts.
README re-clicks and idempotent response replays can increment them without
creating another Launch or pull request. They are therefore a safe anomaly
detector and investigation trigger; only durable owner-safe Launch/PR evidence
can establish external resource multiplicity.

Every worker settlement also emits `launch_id`, `correlation_id`, action/effect,
outcome, fence, and safe policy generation or rollout reason.

Effect, capability, and cleanup settlements carry the stable event names
`cma_trigger_effect_settlement`, `cma_trigger_capability_settlement`, and
`cma_trigger_cleanup_plan`, all at `schema_version=1`. Effect settlements also
carry closed effect/mode/outcome/state/failure-class fields and the bounded
public failure code. Provider-degradation candidates are emitted only for the
closed GitHub/broker rate-limit, authentication-unavailable, and temporary
outage codes; validation, user-action, ambiguity, and quota results do not
masquerade as a provider outage.

The durable activity record may additionally contain only these allowlisted
identifiers:

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

| Signal                            | Objective                                                                                                                                                          | Alert                                                                                                                                                                                                      |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Scheduler heartbeat               | Every lane reports within `max(3 * interval, 60s)`                                                                                                                 | Page if any lane is absent for 2 objective windows while the process is Ready.                                                                                                                             |
| Candidate pass                    | No `errors`, `failed`, or repeated cursor in 99.9% of passes                                                                                                       | Warn on 3 consecutive affected passes; page on 12 affected passes or 5 minutes.                                                                                                                            |
| Page budget                       | `page_budget_exhausted=false`                                                                                                                                      | Warn after 3 consecutive passes; page if it persists for 15 minutes because work is arriving faster than the configured bound.                                                                             |
| Lease contention                  | A candidate normally acquires or is stale by the next pass                                                                                                         | Warn when `not_acquired / candidates > 0.5` for 5 minutes with no completion; inspect the support lease before intervention.                                                                               |
| Durable step settlement           | 99% of admitted non-user-blocked steps settle, defer with a retry time, or enter a typed block within 10 minutes                                                   | Page any Launch with no event and an expired lease for 10 minutes.                                                                                                                                         |
| At-most-once dispatch             | Zero automatic repeat after `dispatch_fenced` without positive pre-write failure proof                                                                             | Page on any recovery-required dispatch that is automatically claimed as execute.                                                                                                                           |
| Capability expiry/revocation      | No accepted stale policy beyond 60 seconds plus 5 seconds skew; confirmed revocation may be earlier                                                                | Page on policy acceptance outside the bound, generation rollback, or numeric-resource mismatch. Warn when `revocation_pending` survives beyond the bound plus two scheduler intervals.                     |
| Cleanup eligibility               | 99% of eligible plans appear within 15 minutes; execute completions within 24 hours                                                                                | Warn on a 15-minute plan backlog; page on a 24-hour execute backlog or any widened/tampered target rejection.                                                                                              |
| Session continuity                | 100% across Trigger flags and cleanup                                                                                                                              | Page immediately if a flag or Trigger cleanup stops, rotates, blanks, or deletes a running session.                                                                                                        |
| Credential/content leakage        | Zero                                                                                                                                                               | Page immediately on any credential, prompt, repository path/content, or provider body detected in logs, activity, support, or artifacts. Preserve the evidence under incident policy and revoke authority. |
| Accepted Launch/PR request volume | Each successful Launch-create and PR-open activity counter stays within its deployment-approved absolute five-minute envelope; a disabled/zero cohort expects zero | Warn at or above 80% for 3 completed windows. Fire `TRIGGER-LAUNCH-PR-VOLUME` at an envelope breach in any completed window; missing counter/rule/envelope wiring blocks public exposure under `O-02`.     |

Dashboard panels must include README preview-to-review, review-to-Start, and
Start-to-ready conversion; state duration percentiles; effect execute versus
reconcile; retry/defer/failure class; fork readiness; initial dispatch state;
checkpoint/restore; branch/PR result; capability generation and revocation;
provider status/rate limiting; accepted Launch/PR activity counters and their
approved envelopes; lease contention; scheduler page budget; and cleanup
backlog. Repository/dashboard infrastructure is owned outside this
repository, so its saved view must use the stable fields above and carry a link
to this runbook. The deployment-specific dashboard and alert-rule URLs are
release evidence, not values that this repository can invent. Public exposure
remains blocked by ledger row `O-02` until those URLs and notification targets
are attached and exercised.

### Severity and alert routing

| Severity | Meaning                                                                                                                                                                                                                                                            | Acknowledge and route                                                                                                                                              |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `SEV-0`  | Credential/content disclosure, cross-owner or unauthorized effect, session loss/interruption, destructive cleanup, automatic duplicate task, or accepted policy beyond the security bound                                                                          | Page the platform on-call and runbook owner immediately; appoint an incident commander; engage security plus the affected identity/runtime owner within 15 minutes |
| `SEV-1`  | Systematic inability to Start or recover, unknown external-write outcome without safe evidence, broken revocation lane, missing audit authority, accepted Launch/PR volume outside the approved rollout envelope, or sustained SLO breach affecting multiple users | Page the platform on-call and runbook owner; assign identity, runtime, storage, or provider owner as indicated; update at least every 30 minutes                   |
| `SEV-2`  | Bounded provider degradation, retry/backlog growth inside safety bounds, isolated stuck Launch, or expected legacy-runtime acknowledgement noise                                                                                                                   | Notify the owning team during the staffed window; assign a responder and review before widening rollout                                                            |
| `SEV-3`  | Isolated usability/documentation defect with no authority, data, continuity, or duplicate-effect risk                                                                                                                                                              | File and triage through the ordinary product queue                                                                                                                 |

Provider and user-action blocks keep their typed product state even when an
incident is open. Severity is an operator response classification; it never
rewrites a Launch failure class or makes an unsafe retry permissible.

The following stable alert IDs are the source links that saved alert rules must
point back to. Each deployment maps them to its actual dashboard, rule, and
notification target in release evidence:

| Alert                                                         | Default severity | First response                                                                                                                                       |
| ------------------------------------------------------------- | ---------------: | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`TRIGGER-SECURITY-LEAK`](#trigger-security-leak)             |            SEV-0 | Pause orchestration, engage security, preserve redacted evidence, and request revocation while tracking acknowledgement or expiry                    |
| [`TRIGGER-SESSION-CONTINUITY`](#trigger-session-continuity)   |            SEV-0 | Stop rollout changes, preserve the session ledger/runtime evidence, and apply forward-safe rollback                                                  |
| [`TRIGGER-DUPLICATE-DISPATCH`](#trigger-duplicate-dispatch)   |            SEV-0 | Fence the Launch, pause orchestration, and preserve all delivery generations; never send a compensating prompt                                       |
| [`TRIGGER-UNAUTHORIZED-EFFECT`](#trigger-unauthorized-effect) |            SEV-0 | Enable both pause and capability kill switch and preserve numeric identity/policy evidence                                                           |
| [`TRIGGER-LAUNCH-PR-VOLUME`](#trigger-launch-pr-volume)       |            SEV-1 | Stop cohort widening, compare safe request counters with unique durable Launch/PR records, and never delete or close user GitHub work as containment |
| [`TRIGGER-CAPABILITY-BOUND`](#trigger-capability-bound)       |            SEV-1 | Keep revocation pending, stop renewal, inspect acknowledgement/expiry, and do not claim effective revocation early                                   |
| [`TRIGGER-STUCK-LAUNCH`](#trigger-stuck-launch)               |            SEV-1 | Use the owner-safe support lookup, inspect lease/effect evidence, and schedule only read-only reconciliation                                         |
| [`TRIGGER-SCHEDULER-HEALTH`](#trigger-scheduler-health)       |            SEV-1 | Compare all four lane heartbeats, page budget, cursor, and lease contention before changing concurrency                                              |
| [`TRIGGER-PROVIDER-DEGRADED`](#trigger-provider-degraded)     |            SEV-2 | Honor retry times, preserve public/session reads, and pause new external effects if failures spread                                                  |
| [`TRIGGER-CLEANUP-DEBT`](#trigger-cleanup-debt)               |            SEV-2 | Return cleanup to dry-run on target/audit anomalies and inspect exact retained plans                                                                 |

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

<a id="trigger-launch-pr-volume"></a>
**TRIGGER-LAUNCH-PR-VOLUME.** Stop cohort widening and new Trigger admission
through the section 11 controls without stopping or degrading an existing
session. Preserve the completed alert window, its approved integer envelopes,
and both closed counter totals. Use the authorized activity/support views to
compare accepted requests with unique durable Launches and pull-request records;
do not add identity or repository labels to the metric. Separate README
re-clicks and idempotent replays from genuinely distinct resources. If unique
external work is abnormal, keep new effects paused, follow the duplicate-effect
and provider procedures as applicable, and contact affected owners or upstream
maintainers. Never close, delete, or rewrite a pull request as containment.

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
Launch query, effect, capability, cleanup, and connection-revocation
inventories read bounded raw-BSON batches behind a separately validated string
`_id`/`created_at` cursor envelope. A row whose aggregate cannot be decoded or
whose duplicated query fields disagree is skipped without withholding valid
neighbors and emits the redacted counter event
`event_name=cma_trigger_launch_quarantine`, `schema_version=1`, `increment=1`.
Its only differentiating fields are the closed `inventory` value (`query`,
`worker`, `capability`, `cleanup`, or `connection_revocation`) and the closed
`reason` value (`decode_failed` or `aggregate_validation_failed`); it never
contains Launch, owner, repository, provider, correlation, or error data. CMA
does not repair, reroute, revoke, or delete the quarantined row from fields it
could not validate. Operators retain the event as migration evidence and may
act only from independently verified identifiers.
The one exact historical routing exception is a pre-versioned
`runtime_admission` with no `kernel`: the predecessor schema had only the
ordinary `codex` route, so CMA reads that absence as legacy `codex`, never as
`codex-trigger-native-v1`. An explicitly stored legacy `codex` value remains
readable for inspection, stop, and fenced cleanup. Current review, Start, and
provisioning compare it with the configured native identity and refuse new
effects; operators inventory and retire these rows rather than rerouting them.

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
- The ephemeral staging CMA keeps `CHRONO_TRIGGER_CLEANUP_MODE=dry_run`. After
  terminal Kind destruction quiesces every writer, the staging harness settles
  only that run's exact live logical context/workspace objects through the
  generation-aware storage API and proves their absence. Durable storage
  tombstones remain for the bucket lifetime and must stay outside every provider
  lifecycle expiry rule.
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
redaction applied. Ephemeral staging evidence must additionally carry the closed
provider-object settlement receipt described above. Local Kind destruction is
the required no-writer precondition, not object-absence proof by itself. A
production restart is never performed solely to collect a drill transcript.

### Git Data publication-capacity measurement

`CHRONO_TRIGGER_CHANGED_FILE_LIMIT=1000` is the schema/configuration ceiling,
not provider evidence. Unit tests and local GitHub/NyxID fakes prove request
shape, batching, recovery, and refusal classification; they cannot establish a
safe file-count value against the deployed proxy/provider pair. Until the
following drill passes, select a lower value supported by retained evidence or
keep public branch publication disabled.

The default preparation pass is non-mutating. Against local fakes or retained
redacted artifacts, reconstruct the exact reviewed mix of deletions, mode-only
entries, inline UTF-8 content, and separate binary/large-text blob writes. Verify
the request count, 128 KiB per-file and 4 MiB cumulative inline decisions,
digest-sealed blob/tree/commit recipe, one-at-a-time object cursor, final
`force=false` ref body, 32 MiB response bound, and typed rate-limit/refusal
projections. Label this result **simulation only**; it does not satisfy the
provider measurement gate and must not create, update, or delete a GitHub
resource.

A real measurement requires explicit owner authorization for a disposable
GitHub fork/branch and an approved staging/canary deployment. It must not target
an upstream branch, a contributor's existing fork, or production solely for
evidence collection. For each candidate changed-file value and representative
byte mix:

1. Pin and record the CMA build, NyxID build/service identity, GitHub API
   version, configuration revision, UTC interval, numeric upstream/fork/account
   identities, expected branch head, and cleanup owner.
2. Record exact counts and bytes for deletions, mode-only changes, inline text,
   separate text blobs, binary blobs, executable/symlink modes, and workflow
   paths. Include the resulting blob, tree, commit, ref-read, ref-write, and
   verification-read request counts.
3. Publish only through the normal reviewed confirmation path. Capture redacted
   request/response timings, HTTP statuses, provider request IDs, `Retry-After`
   and rate-limit headers, object cursor transitions, expected/resulting heads,
   and whether every blob, tree, commit, guarded ref, and final verification
   phase completed.
4. Repeat the authorized lost-response and stale-head cases. Prove recovery
   reconstructs only the sealed recipe, performs no blind ref retry, creates no
   divergent logical commit, and preserves a manually changed head.
5. Record the pass/fail threshold and reason, independent review, and rollout
   decision. A pass supports only the measured provider/build/configuration and
   file/byte mix; it is not a mutable GitHub limit baked into product logic.
6. Remove only the disposable resources named in the authorization, using the
   approved cleanup owner and provider procedure. Attach resource-count and
   branch/ref evidence before and after cleanup; CMA cleanup itself must not be
   broadened to delete GitHub resources.

The evidence package must explicitly say **not run** when this provider drill
has not occurred. In particular, the current 1,000-file ceiling must not be
reported as measured or release-ready based on local tests alone.

## 15. Recovery drill procedure

The minimum release drill proves that a possible successful GitHub write can be
recovered after worker/control-plane loss without duplicating a resource,
interrupting the session, or granting an operator write authority. Run it in an
authorized staging/canary environment against the disposable public fixture.
Never restart or deploy production solely for this drill.

### Preconditions

1. Record the evidence fields required by
   [CMA_Trigger_Release_Readiness.md](./CMA_Trigger_Release_Readiness.md#2-evidence-record),
   including final build/runtime digests, configuration revision, and the
   generation-aware provider-object settlement fields above.
2. Limit publisher, repository, and contributor admission to the fixture.
   Verify staging has `CHRONO_TRIGGER_CLEANUP_MODE=dry_run`; verify the capability
   lane and all four scheduler heartbeats (`effects`, `capabilities`,
   `workspaces`, and `cleanup`) before injecting a fault.
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
sequence preserved all user work. The pass proves current-run live logical
objects absent after writer quiescence while retaining any storage tombstone
authority required by the pinned provider contract.

Abort and keep rollout closed on any second provider write, automatic replacement
prompt, live lease break, cross-owner data, credential/content marker, missing
audit event, unexpected cleanup target, policy acceptance beyond its bound, or
session/history/workspace interruption. Preserve the evidence; never delete or
reset the provider resource to make the next run look clean.

The evidence package includes the redacted command/request shapes, support and
activity projections, state/event timeline, resource-count diff, saved
dashboard interval, fired-alert links, rollout revisions, expected versus actual
table, provider-object settlement receipt, redaction review, and
independent reviewer sign-off. Until that package
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
