# Native GitHub integration and ordinary channel execution

This revision supersedes **slices 4–22 of the 2026-11-04 plan**. The owner selected
one-time durable administrator approval and lifecycle tightening for this release,
and ordinary channel turns instead of GitHub execution groups. App private-key
custody, App JWTs, installation-token issuance, automatic reconciliation, batching
and interpretation of Agent replies are not part of this design.

[The protocol](CMAEG_Protocol.md) is normative. App management, installation
approval, lifecycle receipts and retirement are implemented in the first PR.
**Business dispatch is unavailable in that PR:** eligible business deliveries
return 503 `github_dispatch_unavailable` without retaining their delivery ID.
The following ordinary-dispatch section describes the second PR, not a capability
of this intermediate release. Neither merge nor local tests prove deployment.

## Ownership and authority

The integration, binding and channel share one authenticated owner. Management
selects an organization with `X-CMAEG-Owner`; current NyxID organization-admin
membership is required. JSON cannot choose an owner. App verification and binding
approval independently require `cma-eg:admin` and record the actual verifier.
Other owners receive 404 for resource reads and mutations.

Registration creates an unverified App draft with a separately sealed, write-only
webhook credential. Signed ping proves connectivity only. App verification
reserves `(github.com, app_id)` uniquely; it does not approve an installation.
The owner requests a binding for the verified App, installation ID, account ID
and requested repository IDs. This requested set is advisory. The administrator
independently selects the authoritative approved set, which may differ from the
request. The request creates no execution authority.

Both App verification and binding approval require `cma-eg:admin`. For an
organization-owned resource the caller must also select that owning organization
using `X-CMAEG-Owner` and prove current organization-admin authority. Global gateway
admin permission alone does not permit cross-tenant approval.

Administrator verification supplies `expected_version`, `evidence_ref` and an
explicit repository set. The approval records the verifier, approval time,
evidence and resulting authority revision. The same operation explicitly
re-approves a suspended binding or changes its approved repository set. An
uninstalled binding is terminal. Installation/account/App identity is immutable.

**Binding approval is the authority boundary.** Channel `repository_ids` selects
routes inside that boundary; it cannot widen approval. Exclusions override both.
`repository_ids` on the binding reports the effective approved set;
`approved_repository_ids` retains the approval set and `excluded_repository_ids`
records subsequent exclusions. A source must also match installation, event and
action allowlists. Sender identities never grant CMA ownership or tool authority.

Bindings have no time-based expiry. `verified_until` remains a numeric v1
compatibility field with value zero and is never consulted for authority.
`proof_ref` remains a deprecated request field only: static-proof requests are
refused. Approval uses an authenticated decision, never a reference string alone.

## Lifecycle tightening

| Signed observation | Durable result |
| --- | --- |
| `installation.suspend` | Suspended unless already uninstalled |
| `installation.deleted` | Terminal uninstalled binding |
| `installation_repositories.removed` | Exclude each removed repository |
| `repository.transferred` or `repository.deleted` | Exclude that repository |
| A repository owner ID differs from the bound account | Exclude that repository |
| `created`, `unsuspend`, `new_permissions_accepted`, `added` | Preserve authority; never activate or re-include repositories |

`authority_revision` advances only when binding status or effective approved
repository eligibility changes. An unrelated repository notification or a new
GitHub delivery repeating an existing exclusion does not revoke other authority.
Management `version` may still advance to reject a stale request to expand scope.
Re-approving unchanged eligibility records a new audit/version without advancing
the eligibility revision. Exclusion lists retain only requested/approved IDs;
outside-scope IDs already have no authority. Approval sets remain bounded to 256.
Positive events never widen authority, regardless of arrival order.

Lifecycle deliveries remain receivable for pending, suspended and uninstalled
bindings. Unbound installations get compact `ignored` receipts with reason
`installation_unbound`, owned by the integration owner. They never wait for a
future approval. Pending or otherwise ineligible business deliveries likewise
have terminal ignored outcomes. Retirement is the exception: a retired
integration refuses **all** webhook requests, including ping and redelivery,
with 410 `github_integration_retired`, and creates no new receipts.

There is **no automatic reconciliation**. Capabilities report
`automatic_reconciliation: false`; `verification_credential.present` remains
false. Missing lifecycle deliveries are not independently discovered by a
background GitHub API query. The daily board sync reconciles business data; it
does not refresh installation authority. App administration and Agent GitHub
access remain separate permissions.

## Persistence and replay

The existing sealed `eg_automation_records` collection holds integrations,
bindings, approval audits, operation replay records and compact receipts.
Receipt roots use the gateway's expiring-record pattern: an `expires_at` field,
Mongo TTL index and logical expiry checks before reads/replay. `retained_until`
reports a fixed seven-day receipt/deduplication horizon. Ignored business/ping raw
bodies expire within 24 hours; unbound/ineligible receipts retain no body, and
lifecycle evidence expires after seven days. Duplicate reads never renew expiry.
An expired retained row returns 410; the TTL monitor eventually removes it, after
which lookup is 404. Expiry does not enqueue or retry work. Recovery and duplicate
suppression are defined within this horizon; older IDs are not a recovery path.
GitHub.com supports redelivery for three days ([official contract](https://docs.github.com/en/webhooks/testing-and-troubleshooting-webhooks/redelivering-webhooks)).
The dispatch PR must pin admission/task retention no later than the receipt's
horizon and refuse expired pending work instead of reconstructing it.
Configuration, binding approvals and operation audits do not receive TTLs.

Create and mutation keys are scoped to authenticated owner and operation/resource.
The binding request and its replay record commit together. Approval commits the
binding version, immutable audit and replay result together. Changed input under
the same key is 409. Exact replay returns the original result without rewriting
current authority; GET is the current state. Retired integrations refuse earlier
verification/credential/binding mutations even if their old operation exists.

Inert receipt inserts take no integration/binding write guard: they grant no
execution authority. Current integration reads refuse retired endpoints; already
authenticated in-flight inert requests can finish without affecting authority.
Binding creation/approval, lifecycle mutations and owner-mismatch exclusions keep
transactional integration guards because they change durable authority or fence a
pending management decision. Their binding/receipt/audit writes commit together.
Snapshot reads alone cannot fence retirement against an authoritative mutation.
Transient conflicts retry with jitter (up to 132 ms between attempts) within a
3.5-second budget, inside the webhook's five-second total budget. Management
retries retain the original body/key/CAS; a changed version reports the current
version as 409. No retry silently updates the caller's expected version.

Retiring a verified integration atomically marks its App identity reservation
released. Another owner's pending integration can then be independently verified;
concurrent claims still have one winner. Replaying old retirement/verification
cannot release or reclaim the new reservation. Retirement terminates the old
integration's installation authority through the same integration guard used by
binding approval and lifecycle writes. New bindings derive their identity from
`(app_ref, installation_id)`, allowing the replacement integration to request and
independently approve the same installation. Its approval starts empty. Historical
bindings and receipts retain their original IDs, owner and recorded state; their
retired parent prevents any authority use or reactivation. Lookup also supports
existing `(app_id, installation_id)` IDs, but only when their owner and integration
match. It never reuses a retired integration's approval for its replacement.

The feature-gated migration advances versions 1–3 to version 4, adds receipt TTL
metadata/indexes, and stamps legacy receipts with one fixed seven-day migration
deadline without rewriting their sealed bodies. Restarts never extend that stamp.
Only delivery rows can carry expiry. The marker commits last; unknown versions
fail closed. No new service, deployment key, App config file or database grant is
required. Disabling GitHub preserves configuration; existing receipt TTLs continue.

Legacy bindings are not inferred to be approved. The new version/audit shape is
required; unreadable records fail closed. Execution-group records remain an
allowed historical storage kind but have no executor. The reported absence of
legacy binding/group records in production is not a migration safety assumption.

## Ordinary dispatch: second PR

One verified delivery produces one Event Activity per matching channel. Candidate
channels come only from that owner's persisted definitions. Re-read current
channel authority before admission; neither a static route list nor an in-memory
authority cache authorizes work. The channel requires durable routing, explicit
non-human acceptance and provider replay support.

Commit the receipt with pending dispatch before launching work. Attempt bounded
immediate dispatch and recover pending receipts after restart. Stable identities
for `(integration, delivery, channel)` replay the same routed admission. Pin the
compact projection, timestamps and candidate identities so retries cannot add
new routes or change input. `receipt.task_ids` contains admission IDs only.

The guarded admission-root commit is an **opt-in GitHub-only parameter**. It must
commit the root with actual integration/binding guard writes and the channel
version fence, using the existing gateway collections. All other admission paths
retain their behavior. At commit, recheck this delivery’s repository: binding
active, repository approved and not excluded, integration not retired. A global
`authority_revision` mismatch is not a refusal: unrelated tightening retries the
current version fence and leaves eligible pending work admissible. `dispatched` means durably admitted, not Agent started.

The Activity uses actor kind System and a compact CloudEvent: delivery/event/action,
installation, repository ID/full name, issue/PR number, HTML URL, sender login/type,
state and PR merged/draft flags. No title, body, comment or arbitrary payload field
enters the Activity or rendered prompt. Instructions come from the owner's source
`prompt_template`. The gateway knows no board-sync workflow.

Task query projects routed admission and execution/activity state. `Task.result`
and its schemas/enums remain available for v1 compatibility but are never populated.
A completed turn does not attest a PR, merge or Pages deployment. Those facts come
from the chrono-goals publisher and GitHub evidence.

## Serialization and release gates

CMA's durable command lane serializes messages for the same Agent: a live claim
excludes another claim, ordinary messages retain FIFO order, and message dispatch
requires a Ready Agent with no active turn. See `agent_io/lane.rs` and the Mongo
command-lane implementation. Existing-Agent channel turns use Responses v2 with
verified binding authority; their stored actor/source can be Api.

Ordinary channel management also needs the generic organization-owner selection
fix: its current service-auth creator path ignores `X-CMAEG-Owner`, while the
GitHub management extractors honor it. This blocks organization channel setup and
must be fixed before the pilot; do not substitute personal resources.

The current five-minute queue deadline is insufficient for this automation.
A separate CMA queue-policy fix must ship with ordinary GitHub dispatch. Queue
expiry is a definite pre-dispatch failure (`agent_command_queue_timeout`), and
CMAEG currently settles it as failed rather than resubmitting. Custom events also
have a thirty-minute reference lifetime, and their terminal delivery currently
opens that reference even without a reply. Queue policy and safe terminal
settlement must be addressed together. Do not enable this
pilot until the approved longer bounded policy and two-event online proof exist.

The first consumer runs the complete chrono-goals sync through managed `gh api`,
checks/builds, publishes its managed status PR and merges only after CI succeeds
for the exact head. The external daily GitHub Actions trigger sends an authenticated
Custom event to a second durable channel bound to that **same Agent ID**.

Tests use management/webhook/receipt and approval/lifecycle/routing/task HTTP
seams, real Mongo replica sets for transaction/restart claims, and external
NyxID/GitHub/provider fixtures. chrono-goals tests exercise its CLI against a GitHub
API fixture. [The online runbook](CMAEG_GitHub_Online_Verification.md) is the final
acceptance contract. Release only after dispatch and the queue fix are merged;
verify live discovery before configuring production.
