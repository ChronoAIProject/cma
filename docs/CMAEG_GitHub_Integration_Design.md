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
and requested repository IDs. The request creates no execution authority.

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

Every negative repository observation advances `authority_revision`, including
one outside the current scope, so it fences an older re-approval intent. Exclusion
lists retain only requested/approved IDs; outside-scope IDs already have no authority.
Approval sets are bounded to 256 repositories so metadata and replay records fit
the existing sealed-record limit. Re-approval prunes exclusions outside the new
approved and original requested scopes without granting any omitted ID. Exact
redelivery does not apply it twice. Positive events cannot undo a negative event, regardless of
arrival order. Re-approval advances the revision and cannot restore old pending
work fenced by a prior revision.

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
Raw retained payloads use separately sealed interaction chunks with seven-day
expiry. Compact ignored outcomes without retained bodies report an immediate
raw-body expiry. No raw issue text or credential appears in management responses.
Compact receipt and approval records are retained; this release adds no destructive
cleanup migration.

Create and mutation keys are scoped to authenticated owner and operation/resource.
The binding request and its replay record commit together. Approval commits the
binding version, immutable audit and replay result together. Changed input under
the same key is 409. Exact replay returns the original result without rewriting
current authority; GET is the current state. Retired integrations refuse earlier
verification/credential/binding mutations even if their old operation exists.

Binding and receipt transactions include a **real integration guard write**
conditioned on the observed integration version. Lifecycle transactions include
the binding version update and receipt. The guard counter is internal and does
not advance public metadata versions. A snapshot read alone would permit a
retirement transaction and a stale approval/receipt transaction to both commit;
writing the shared integration document forces a conflict or version refusal.
Raw chunks can be orphaned by a failed commit and expire normally. They never
constitute an acknowledged receipt.

The `github_integrations` migration advances versions 1/2 to 3, retains old record
kinds, adds binding operation/audit records and optional guard metadata, and
commits its marker last. Startup runs it only with `CMAEG_GITHUB_ENABLED=true`
and existing effective migration permissions. It is repeatable; unknown versions
fail closed. Disabled or failed initialization leaves other sources available.
There are no new deployment keys, App config files, services or database grants.

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
retain their behavior. Tightening that wins before root commitment terminates the
pending dispatch. `dispatched` means durably admitted, not Agent started.

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
