# Resource authorization foundation

## Request-bound provider reads

`CHRONO_NYXID_RESOURCE_REQUEST_ENABLED` is default false. Its separate NyxID
issuance flag must remain off until compatible CMA readers are deployed and the
read-only acceptance tests pass. Enabling this path does not require restarting
any user Sandbox, changing its runtime, or touching its persistent home.

When `X-NyxID-Resource-Request` is present, CMA uses only the request-proof path;
disabled, duplicated, invalid, expired and unavailable proofs never fall back to
an ambient bearer or browser cookie. CMA asks the configured authenticated
NyxID introspection endpoint for `token_type_hint=resource_request`, the catalog
service ID and the exact HTTP method, original encoded path/query, content type
and body digest. Provider proof issuance follows normal proxy admission and
checks live key, scope, connection, operation, approval and turn authority again
at introspection. An ordinary identity token cannot substitute for this proof.
The separate `X-NyxID-Identity-Token` header does not activate this proof path.
When `CHRONO_NYXID_PROXY_ENABLED` is enabled, it authenticates the normal CMA API
using the existing signed service identity, as described in
[CMA Operations](CMA_Operations.md#nyxid-proxy-access-to-the-cma-api). That path
supports lifecycle and interaction writes with explicit proxy command custody.
If both headers are present, the request-bound proof path retains precedence.

The first version accepts only GET/HEAD with an empty body. Idempotency keys,
method overrides, duplicate content headers and non-identity content encoding
are refused. The exact descriptor, service audience, token type, finite lifetime
(at most 60 seconds), current subject/RBAC and `independent_commands=false` must
match the provider response. No completed verification is cached. Read retries
must pass another live provider check. Provider unavailability returns 503,
separately from invalid-proof 401 and unsupported-read 403.

The initial route allowlist is finite inventory/detail reads for Agents,
Sandboxes, Workspaces and Worktrees, Sandbox Workspace listings, Workspace
Worktree listings, `/api/v1/agents/{agent_id}/transcript`, and
`/api/v2/agents/{agent_id}/events/snapshot`. The transcript reads and folds
persisted conversation records without contacting the runtime. For the latest
user message, list `/api/v1/agents`, select an Agent, read its `/transcript`, and
select the greatest `sequence` among `turns[].messages[]` with `role=user`.
This includes later owner steering within the same turn. The focused public
OpenAPI also exposes the finite v2 snapshot for paginated event replay.

SSE streams, runtime file/model/skill reads, Bot operations and other routes
are outside this slice. In particular, `/history` invokes the runtime exporter
and is refused; `/settings` and `/resources` are also outside the allowlist.
Existing owner, ancestry and RBAC checks remain authoritative; the proof does
not supply organization membership or another user's sign-in.

The internal `RequestBoundRead` credential source expires with the proof and
grants only resource detail/history reads. It cannot admit mutations, command
custody or Bot authority even when the owner's RBAC includes write privileges.
Neither the proof nor an ambient credential is attached as a delegated
credential or persisted to the API-bearer store. Durable NyxBot actions use the separate signed service identity integration;
they are not supported by this read-only feature.

Milestone 17 separates retained resource ownership from the principal executing
an action. The implementation in `layer2-core::resource_access` defines the
policy, transactional ownership/share stores, and authenticated share-management
and effective-access routes. Shared Agent and hierarchy REST reads are composed. Interaction, stream/ticket
revalidation, and the complete background command scheduler are still being
connected; shared access is not yet available across every existing interface.

## Ownership and attribution

`ResourceOwner` is explicitly `person(subject)` or `organization(subject)`.
Existing aggregate owner fields retain their old serialized form and personal
meaning. Moving the command ledger's owner/actor types into the common module
preserves their exact tagged JSON shape through the `AgentOwner` and
`AgentActor` aliases. A subject that resembles an organization name or ID does
not establish organization ownership.

`CallerContext` carries the authenticated CMA caller, issuer and credential
context, actual actor, and provider membership observations. It cannot be
constructed by deserializing a public request or an old command receipt.
Ordinary policy requires actor and authenticated subject to agree. Channel and
explicitly delegated actors require the separate bridge policy; owner RBAC in
a callback is insufficient.

NyxID userinfo does not currently expose `user_type`. The organization adapter
therefore records a verified API actor without inventing human identity from
profile fields. It uses the live organization authorization endpoint to prove
membership and canonical organization identity. Browser and bridge adapters
must retain their own verified actor/delegation evidence.

## Retained ownership and share storage

Memory and Mongo stores resolve Sandbox/Workspace/Worktree/Agent ancestry under
one snapshot, checking exact parent IDs, retained owner, revisions and Agent
conversation integrity before returning policy evidence. The additive
`resource_policies` sidecar binds the owner kind and policy revision to the
Sandbox ID, recorded owner and creation timestamp. An absent sidecar means the
recorded personal owner. Organization sidecars require explicit issuer, canonical
organization and admitting actor evidence; no production admission path writes
them yet, and no backfill infers organization ownership.

`resource_shares` holds current grants; `resource_share_commands` holds permanent
immutable mutation receipts. Create, mode change and revoke use both policy and
share revision fences. The owner/selector/idempotency-key scope survives later
revocation, retries recover the original receipt, and changed input conflicts.
Receipt validation checks attribution, root incarnation, revisions, timestamps
and the requested effect. Only the personal owner can issue these store commands;
the caller of these internal ports still must perform current authorization.

Mongo commits policy, share, receipt and additive ancestor write guards in one
transaction. Retained aggregate payloads/revisions, native conversation, profiles
and logs are unchanged. An unproved commit returns unavailable; the same key
recovers the exact original receipt. Active grants are bounded to 128 per Sandbox.
The current stores always report isolation as unproved, so narrow interactive
sharing needs a compatible explicit whole-Sandbox grant. Revoking that grant
removes its interaction support on the next policy evaluation.

## Policy

| Authority | Read details/history | Read resource files | Interact/resolve approvals | Manage Sandbox | Manage shares/bridge |
| --- | --- | --- | --- | --- | --- |
| Personal owner with CMA resource authority | Yes | Yes | Yes | Yes | Yes |
| Current org admin, org-owned resource | Yes | Yes | Yes | Yes | Yes |
| Current org member/viewer, org-owned resource | Yes | Yes | No | No | No |
| Current member/admin, Agent `view` share | Yes | No implicit file grant | No | No | No |
| Current org admin, Agent `org_roles` share | Yes | No implicit file grant | Only with current isolation or compatible whole-Sandbox share | No | No |
| Current org admin, whole-Sandbox `org_roles` share | Yes | Yes | Yes | Yes | No |
| Unrelated person or former member with no other authority | No | No | No | No | No |
| Channel sender through ordinary policy | No | No | No | No | No |

These capabilities do not grant profile authoring, provider connections,
credential values, or platform configuration. Destructive lifecycle routes
retain their separate capability requirements. File readers must apply their
existing secret/path restrictions and scoped artifact policy.

A share grants access without transferring ownership or changing creation
origin. An Agent selector never covers a sibling or the parent Sandbox.
Worktrees and separate homes do not prove execution isolation. Both grant
creation validation and each interaction decision check current isolation or
an unrevoked compatible whole-Sandbox grant. Revoking that grant removes its
support for a narrow interactive share. Recipient access grants no resharing
capability.

Inaccessible resources use the existing concealed-not-found behavior. Visible
read-only resources return explicit management denial. Independent ownership,
operator capabilities, and valid overlapping shares survive an unrelated
expired/revoked grant. Operator authority remains explicit: resource-wide read
or control capabilities do not imply bridge credential custody or resharing.

## Organization evidence

The `OrganizationAuthority` adapter verifies the supplied credential with the
configured provider, requires the actual verified subject to match the caller,
and calls `GET /api/v1/orgs/{id}/authorization`. The returned canonical ID must
exactly match the requested ID. Only `admin`, `member`, and `viewer` are
accepted. Service-account, delegated-actor and relay-shaped JWTs cannot acquire
ordinary personal membership authority; this local check only refuses tokens,
and never replaces provider verification.

No completed membership response is cached. Each lookup has a 15-second total
deadline and a bounded response body. Positive evidence expires at most 60
seconds from the beginning of the lookup, including provider verification.
Both wall-clock and monotonic deadlines are enforced; clock rollback cannot
extend an observation or a held access decision. Issuer, subject, organization,
and credential identity must match. Inventory is sorted by canonical ID and
contains presentation choices only, never reusable resource authority.

Mutations must start a fresh check, then require membership observed after that
check began. Deferred effects start another check. Streams and artifact tickets
must revalidate by the decision deadline; the future stream integration must
also re-read current shares and ownership. Neither retries nor provider errors
can renew old evidence. Lookup errors remain local to the affected org and do
not remove independent personal access.

## Authenticated service and public routes

The HTTP middleware attaches a nonsecret request context after authentication.
Provider credentials use the same issuer/subject/digest binding as fresh
organization observations. The new extractor requires the context to match the
existing authenticated Caller. No organization membership is inferred there.

`ResourceAccessService` discards caller-supplied membership observations, obtains
fresh evidence with the actual requesting credential, and rereads the policy
after provider IO. A changed policy fence refuses the pending read. The entire
lookup set has a 15-second deadline. Independent personal-owner checks do not
depend on organization lookup.

Share commands accept only selector, requested effect, expected share revision
and idempotency key. The service supplies owner, actual actor, root fence, server
ID and time. New target organizations require current canonical membership
evidence for the owner. Mode changes preserve the target; revocation does not
depend on that organization remaining available. An equivalent retry recovers
its original immutable response after current owner authorization, even after
revocation or organization membership loss.

Production composition includes these authenticated routes:

- `GET /api/v1/resource-access/{kind}/{id}` returns policy capabilities,
  revalidation deadline and policy revision. Kinds are `agent`, `sandbox`,
  `workspace` and `worktree`. Responses omit owner and parent metadata.
- `GET/POST /api/v1/resource-shares/{kind}/{id}` lists active direct grants or
  creates a grant. Share kinds are `agent` and `sandbox`. Lists are ID-sorted
  and bounded by the 128-active-grant capacity.
- `GET/PATCH/DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}` reads a
  management record, changes mode or revokes it. Reads retain revoked records.
  Mutations require `Idempotency-Key`; update bodies and revoke queries require
  `expected_revision`.

Recipients cannot list owner grants or reshare. Visible read-only resources
return 403; inaccessible resources return 404. Closed JSON bodies reject owner,
actor, membership and policy-fence fields using the existing 422 contract.
Effective permissions describe policy; lifecycle, runtime support and each
operation's authorization still apply. Agent REST detail/history/usage readers
now enforce these permissions. Organization Agent inventory and safe source
categories and hierarchy views are also composed. AG-UI replay/follow applies
the same shared history policy; other streams, artifacts and interactions
remain required across the rest of the product.

OpenAPI and repository TypeScript/Rust SDKs expose all six operations. The SDKs
retain explicit keys/revisions and never automatically resend an uncertain
mutation. Shared contract fixtures replay 43 actual-handler cases in both SDKs.
No SDK package publication, UI workflow or production rollout is claimed here.

## Agent REST read projections

Production composes one ResourceAccessService for management and Agent detail,
transcript, event, source, usage and resource reads. The authenticated request context
must match the existing Caller. A missing context supports older compositions;
a present invalid context is a 401 and never falls back to personal authority.

Personal-owner detail retains its established complete hierarchy and JSON.
Every other authorized reader receives `access: "shared"`, Agent ID/status,
optional title/model and timestamps. No parent ID, path, profile configuration,
credential, native runtime identity or creation provenance is included. The
TypeScript `agents.readAccessible` and Rust `agents().read_accessible` methods
model both shapes; existing `read` methods retain the full personal-detail type.

Shared transcripts and event pages preserve conversation messages and outcomes.
The exhaustive shared event projection withholds ancestry, native conversation
and generation identifiers, work-step diagnostics, correlation metadata,
sign-in URLs/codes, first-message custody and runtime configuration. It removes
origin keys and approval locators and sanitizes failure text and stop reasons.
Withheld records become `{ "type": "access_redacted" }` at their exact original
sequence/timestamp. Pages count these records, so every cursor still advances
through the authoritative complete log. Redaction never mutates stored data
or excuses a corrupt or incomplete log.

Agent usage requires its own ViewDetails decision and runtime capability.
Sandbox metrics also require independent Sandbox ViewDetails authorization;
a narrow Agent grant cannot expose counters from its siblings. Buffered
history/usage/metrics check current policy again after IO and refuse release
after a changed fence or expired membership. Checks do not extend evidence or
retry effects. Membership outages retain the actionable
`organization_authority_unavailable` response, and inaccessible Agents remain
404 `agent_not_found`. Existing-user state has not been mutated for verification.

The Agent source route returns a separately defined safe shared projection:
Agent ID, explicit shared marker and immutable creation category only. Its
`manual`, `workspace_profile` and `trigger` variants have no private Launch or
Trigger IDs, profile indexes or publication shortcuts. Shared reads never
consult another owner's Launch or Trigger records. Personal source retains the
established full provenance and current publication behavior. Layer 2 fences
both successful results and dependency errors after IO; membership expiry or a
policy change refuses the buffered result. Legacy source composition also reads
the explicit owner kind and cannot reinterpret an organization as a person.
Channel binding and usage history uses a separate authorized projection described below.

## Organization inventory

`GET /api/v1/organizations` exposes current canonical choices and presentation
roles, not reusable membership evidence. `GET /api/v1/agents?organization=<id>`
selects organization-owned Agents and personal Agents with a covering active
grant to that organization. The default collection remains personal and has no
organization dependency. SDKs expose `resourceAccess.organizations` /
`resource_access().organizations()` and typed `listAccessible` / `list_accessible`.
Personal list summaries retain their established JSON; other readers receive
safe shared Agent-only fields. Organization ownership still requires an explicit
sidecar and admission evidence; raw retained owner strings cannot enter the org
inventory. Production organization admission remains to implement.

The L2 boundary obtains new evidence with the actual requesting credential and
intersects current provider capabilities. It clears historical/supplied membership
observations. Parent filters first require separate current parent authorization,
so matching a narrow Agent cannot reveal its inaccessible Sandbox or Workspace.
Status and cursor predicates never create additional visibility.

Memory filters before pagination. Mongo reads indexed ownership, whole-Sandbox
grant and Agent-grant branches under one read-only transaction. Each branch
joins matching Agent records, deduplicates, sorts and returns a bounded prefix;
merging those prefixes gives the first `limit + 1` unique Agent IDs. Every
candidate, including lookahead, resolves its authoritative policy and full
ancestry in that snapshot. No unfiltered counts or candidate cursors leave L2.
Membership and current policy fences are checked again before release, including
the lookahead that determines `next`. Database/validation IO has bounded deadlines
and at most eight concurrent final policy reads. An unavailable or changed page
returns a typed error without partial results or stale `next` metadata.

## Shared hierarchy projections

The five hierarchy GETs (Sandbox detail and Workspace list, Workspace detail
and Worktree list, Worktree detail) now use the same authenticated policy service.
A narrow Agent grant never authorizes a parent or its counts. Whole-Sandbox
visibility, current organization ownership and explicit global capabilities
supply hierarchy access under the established policy. Owners retain the exact
personal JSON; other readers receive safe `access: "shared"` shapes with IDs of
the authorized hierarchy, display names, statuses, timestamps and child counts.
No owner identity, profiles, source configuration, repository paths or branches,
runtime bindings, credentials, lifecycle internals or failure diagnostics leave
these shared projections. Worktree assignment checks resolve the Agent in the
same policy snapshot and refuse foreign, missing or inconsistent assignments.

Mongo reads each complete hierarchy under one read-only snapshot transaction,
then aborts it to release the session. L2 validates root identity/incarnation,
all child ownership/ancestry, duplicate IDs, Agent conversation integrity and
bidirectional Worktree assignments before projecting counts. After IO, both
successful results and stored dependency failures pass current policy and held
membership checks. Authorization, hierarchy IO and final policy IO each have a
15-second bound; an expired held decision is never silently renewed.

The personal Sandbox collection excludes explicit organization ownership before
pagination and checks the hidden lookahead's policy. Composed detail reads
supply validated counts, with at most eight concurrent roots and a 30-second
page deadline. Organization Sandbox discovery uses the explicit collection filter below;
organization ownership admission remains to implement.
Snapshot archive export remains personal-only because it contains a whole home.
SDK `readAccessible` / `read_accessible` and subordinate `listAccessible` /
`list_accessible` methods expose the typed personal/shared unions; older personal
methods retain their full types. Seventy actual-handler fixtures replay through
both SDKs, alongside read-only retained-state and disposable Mongo regressions.

## Organization and Shared Sandbox inventory

`GET /api/v1/sandboxes?organization=<canonical ID>` uses fresh evidence from the
actual request credential and the same role/capability intersection as Agent
inventory. The default personal collection remains independent of organization
lookup availability. A selected organization contributes only explicit valid
organization ownership or current whole-Sandbox grants. A narrow Agent share
cannot reveal its parent, count or pagination metadata.

Mongo reads two indexed candidate branches in one read-only snapshot transaction,
deduplicates before the limit and returns at most `limit + 1` exact root snapshots.
Memory applies the same scope before pagination. L2 checks the candidate policy,
query, identity and ordering, then obtains validated hierarchy counts for visible
rows with at most eight concurrent reads. The entire count phase has a 15-second
bound. After this IO, success, dependency errors and timeouts pass current policy
checks for all candidates, including hidden lookahead, plus the original held
membership deadline. An empty page also needs unexpired membership. Caller-name
lookup finishes before acquiring organization evidence; no directory IO occurs
between the final authorization check and the HTTP projection.

Personal owners retain the existing full summary JSON, while others receive
safe `SharedSandboxSummary` metadata/counts with an explicit shared marker. Both
SDKs expose `listAccessible` / `list_accessible` with organization and shared
selectors. Legacy personal `list` types remain intact and refuse reduced shared
responses. This adds discovery only; organization ownership admission and
organization credential custody remain separate requirements.

`GET /api/v1/sandboxes?shared=true` aggregates explicit incoming whole-Sandbox
grants across current organizations. It excludes the caller's own personal roots
before pagination and deduplicates overlapping organization grants. Organization
ownership alone and narrow Agent grants cannot create a shared root. Its Mongo
candidate branch scans indexed active Sandbox shares, then validates complete
snapshots in the same read transaction. It never derives parents from Agent rows.

Shared Agent and Sandbox inventories use the same bounded membership discovery:
at most 128 choices and eight verified lookups at a time. Only confirmed removal
prunes stale discovery choices; provider failures reject the entire result. The
complete Sandbox request, including discovery, counts and final fences, is bounded
to 15 seconds. Original evidence deadlines, candidate policies and the hidden
lookahead remain authoritative through final projection. A changed status also
invalidates a status-filtered response. Combining `shared=true` with an explicit
organization is invalid. Omitting both selectors preserves personal discovery.

## Agent Replay And Live Follow

AG-UI GET events composes `AgentReadService::authorize_event_stream`, an opaque
nonrenewing authority with no bearer credential, and a safe stream snapshot of
Agent ID, runtime generation, active turn, durable head and approved history.
It never projects raw parent configuration. Full log integrity is checked before
redaction. Withheld entries become empty state patches carrying their durable
cursors, so reconnect does not get stuck behind private records.

All pending frames pass a current policy/lifetime check at release. Counted
membership lifetime is monotonic and cannot be extended by reads; every IO phase
has a 15-second bound shortened by the original evidence deadline. Idle follow
wakes by that deadline. Snapshot success, dependency failure and timeout pass a
final policy fence. A changed policy, expired authority or unavailable store ends
observation. A reconnect obtains fresh exact-credential evidence. Personal owners
reauthorize against their actual captured caller and current owner kind, while
retaining exact root/Agent incarnations and ancestry; recipient grant revision
changes therefore do not break the independent owner stream. No read path sends
input, changes the Agent, renews a credential or adopts another target.

For buffered v2 `events/snapshot` and `events/head`, the default-off
`CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED` path performs entry authorization,
one current Agent snapshot, and exactly one final `validate_event_stream` call.
The current header precedes transport cursor selection. The complete page,
predecessor, committed-tail proof and durable bindings stay inside one
`authorization.bounded` operation. Successful results, selector errors, dependency
failures and timeouts all pass the final fence before returning to the transport.
Original nonrenewing membership deadlines, owner/scope/incarnation comparisons
and the 15-second IO bound remain authoritative.

The removed intermediate checks previously released no data outside the process.
Private reads performed before a revocation becomes visible do not authorize
release: both their results and errors are discarded if the final fence fails.
Page projection/serialization follows that fence synchronously, without another
private dependency read. Public stepwise binding/read methods and SSE fill/frame
release retain their existing bounded operations and fences. Independent security
review of revocation and error paths is required before production activation.

AG-UI run, Responses creation/streams, companions, files and attachment tickets
still require their separately documented scopes. Shared event access alone
cannot authorize any of them.

## Deferred human commands

`AgentCommandAdmissionAuthority` checks the actual request issuer, subject,
credential identity and Human/API attribution, then binds the exact selected
credential to durable encrypted custody. A trusted request-source discriminator
keeps existing browser sign-in custody separate from independently presented API
credentials. A failed browser custody check never falls back to API retention.
Resource policy is checked
independently by `AgentCommandService` before reservation. A current human admin
can therefore act on a shared or organization-owned Agent without borrowing its
owner's login. View grants and member/viewer roles do not grant interaction.
Personal owner behavior and the existing `personal:` custody hash are preserved.
The hash binds configured issuer, actual human subject and sign-in incarnation;
it does not encode resource ownership or resource-policy revision.

Independent API custody uses an `api:` authority bound to configured issuer,
actual subject, exact credential identity and the provider-verified expiry.
Authenticated NyxID introspection must confirm `active=true`, `token_type=access`,
the exact subject, current permissions and a future `exp`. Locally decoded claims
only exclude restricted token types; they never authenticate the credential or
establish expiry. Opaque Agent keys, service accounts, delegation, relay,
resource restrictions and sender constraints are not eligible. Admission stores
the exact access token independently and immutably with encrypted metadata
binding and an expiry index. It does not modify `auth_subjects`, capture refresh
tokens or use an owner's saved credential. Repeated admission retains the same
authority and command idempotency scope.

Overlapping verification calls for the exact same bearer share one in-flight
provider request sequence within an `AuthState`. Completed authorization is not
cached: a later call performs fresh provider verification. Independent custody,
resource policy, membership and dispatch fences remain separate checks. Provider
rate limits, transport failures and temporary server failures are unavailable
authority, not evidence that a credential was revoked or a user must sign in
again. Ambient requests expose these failures as sanitized `503` responses.

The production `ResourceCommandExecution` adapter obtains fresh resource authority
from `AuthState` using the exact retained human credential. Refresh stays within
the original sign-in incarnation. Current provider permissions and organization
roles are verified before each effect, followed by an exact subject, sign-in,
token-generation, token-identity and expiry recheck after resource IO. A different
sign-in cannot inherit admitted commands. Relay, service-account, delegated and
channel credentials cannot use this human path.

For independent API commands, execution and revival re-read the same custody,
verify the exact token with NyxID and recheck current resource/membership policy.
The resulting transaction proof expires no later than the credential. Provider
revocation, lost permissions, changed identity, missing custody or expiry prevents
dispatch. Reads of existing history retain their independent authorization.
Sandbox runtime credential renewal continues to use its separate owner delegation.

Before an execution claim, unavailable authority leaves the durable command
queued. The worker paces transient retries and pending revival checks by up to
15 seconds, bounded by the original queue deadline, and holds no execution lease
during that wait. The delay is per worker process; a process restart can shorten
it, while the durable command, deadline and claim fences remain authoritative.
Every later attempt checks current authority again. Definitive refusal and
expiry retain their terminal behavior, and a bound dispatch continues through
its existing reconciliation path rather than being submitted again.

After admitting `api:` commands, an application rollback must retain support for
this authority and its additive custody collection. Older authorizers cannot
execute these queued commands. Browser `personal:` records and their existing
sign-in-incarnation protections remain compatible.

Execution proof binds the exact command, actual actor, independent resource owner,
current policy, ancestry, pinned profile and a nonrenewing lifetime of at most
60 seconds. The worker rereads its renewed claim after provider IO. Stores validate
the current resource snapshot and claim atomically with dispatch binding; the
ordered writer checks authority again before runtime handler invocation. An
existing binding always selects reconciliation, including after revocation.

Pending message preparation obtains independent fresh Agent interaction and
Sandbox management decisions with the same retained credential. It captures the
resource owner's profile, credential configuration and required connections,
rechecks exact pending command and policy, and schedules one durable lifecycle
step outside the dispatch lease. The store reruns the existing revival planner;
proof cannot authorize arbitrary parent edits. Accepted revival survives caller
cancellation. Mongo's real command-row write and parent CAS serialize policy,
claim and refusal changes, and expiry is checked again before commit.

New admissions, resource dispatch bindings and command-authorized revival append
`CommandAuthorizationAudit` records atomically in a separate store. Each records
actor, owner, credential custody reference and resource decisions independently.
Per-observation capabilities identify personal ownership, operator access, owning
organization roles or exact sharing grant revisions. Organization observations
record configured issuer, canonical organization, role and observation/expiry
times, without retaining a credential. Revival records two independent resource
decisions. Historical audit can be deserialized for internal inspection but cannot
construct a live membership or opaque execution proof. It is excluded from public
command status and recipient history responses.

Idempotent receipt recovery retains original audit facts. Expired/refused
transactions do not append evidence; accepted evidence remains readable after
revocation. Previous serialized receipts and aggregate formats are unchanged.
Organization allocation, runtime/connector credential provisioning and channel
service delegation remain separate unfinished foundations. Existing legacy
personal read/operation boundaries retain their explicit personal owner checks.

## Verification

Tests cover the owner/org-role/operator matrix, changed actor/issuer/credential
contexts, fixed expiry, mutation freshness, wall-clock rollback, held-decision
expiry, unsafe narrow grants, compatible whole-Sandbox grants, overlapping
revocation, and unchanged command attribution JSON. Mock NyxID tests exercise
role downgrade/removal, canonical-ID mismatch, unknown roles, provider outages,
credential restrictions, bounded lookup timing, and separate issuers/orgs.
Deferred execution tests cover missing custody, current permission removal,
credential replacement during verification, rotation within a sign-in,
attribution/revision tampering, denied dispatch, lease loss during provider IO,
and refusal to resend a bound command.

Resource-store tests cover concurrent mutation/replay, unchanged retained
ancestry, stale revisions, attribution/effect tampering, explicit organization
evidence and root incarnation binding. Six real disposable Mongo cases cover
replica/reopen recovery, CAS races, audit-insertion rollback, unsafe interaction,
corrupt ownership projections and actual lost commit acknowledgment. These are
store contracts, not evidence of recipient history, interaction or stream revocation.

## Explicit Sandbox control ledger

`layer2-core::sandbox::control` adds durable `pause`, `revive`, and `retire`
intents. The service, memory/Mongo stores, worker, and production authentication
adapter are implemented and composed with the public receipt routes, both SDKs,
a bounded scheduler, and shared UI controls. Public DTOs explicitly exclude
internal authorization references and audits. Existing personal lifecycle
endpoints retain their contracts.

Admission requires an idempotency key and the expected Sandbox revision. The
service captures the actual authenticated human/API actor, exact sign-in
incarnation, explicit resource owner, Sandbox creation timestamp, and current
resource authorization audit. The additive `sandbox_controls` collection keeps
these records out of retained Sandbox/Agent serialization. One queued command
per Sandbox is allowed, and commands expire after five minutes. The key namespace
includes actor, stable credential-authority ID, Sandbox ID, and command kind;
credential-policy revisions do not create a new retry namespace. A replay checks
current authority and returns the original receipt and deadline, including after
its transition or an independent Sandbox revision change.

Pause and revive require `ManageSandbox`. Retirement uses the owner/admin-only
`ManageSharing` boundary, with the existing `DeleteOwnSandbox` capability also
required for a personal owner. A whole-Sandbox `org_roles` recipient can pause or
revive but cannot retire somebody else's Sandbox. Narrow Agent shares do not
authorize parent lifecycle controls. Global Sandbox control does not imply the
owner/admin retirement privilege.

Before applying an intent, the worker verifies the admitting actor's retained
sign-in and obtains new resource/membership authority. Revival checks renewable
connections for the resource owner and then revalidates the actual requesting
actor again. The second check cannot extend the first observation's lifetime or
change the owner/profile whose connections were checked. A new sign-in cannot
inherit an old command, even when its subject and token text match; token refresh
within the original incarnation can continue it. Proofs carry no raw credential
and are bounded by both membership lifetime and the original queue deadline.

The store rechecks its current resource snapshot and the exact queued record,
then commits the model-planned lifecycle transition, Sandbox event, execution
audit, and terminal receipt in one transaction. It performs no runtime IO.
`Applied` means the transition was committed or already satisfied/scheduled;
existing lifecycle recovery completes any scheduled runtime work. Competing
workers cannot apply the same queued record twice. Mongo uses real ancestor
write guards to serialize sharing changes with the transaction. An ambiguous
commit returns unavailable; reading or replaying the same command recovers its
original outcome without a second transition.

Known revocation, stale targets, queue expiry, ineligible lifecycle state, and
missing connection/setup requirements have typed rejection states. Transient
provider or store outages leave the command queued for a bounded retry. Receipt
reads require the original actor plus current target visibility and the same
resource incarnation. Internal records and their authorization references must
not be serialized directly into public responses.

## Approved Agent artifact registry

Explicitly approved artifacts are immutable content associated with one retained
Agent, its full ancestry and owner kind. Publication requires current
`ManageSharing`, actual human/API actor attribution and a non-reconstructible
live publication fence; a persisted audit cannot authorize another write.
Memory checks under its write lock. Mongo checks current resource policy and
ancestry, capacity and publication identity in one transaction, then atomically
inserts the record and fences the policy. A retry after uncertain commit returns
the same artifact, while a changed body or actor under that key refuses conflict.
Existing native history, first input, profiles and runtime files are not changed.

`ViewArtifacts` covers only this approved registry. Narrow history readers gain
this capability while `ViewFiles` retains whole-parent scope. No arbitrary URL,
archive extraction or runtime filesystem export can populate it. Every list,
metadata and download request holds original authorization, validates immutable
binding/content, and rechecks current policy after IO. Hidden lookahead receives
the same validation. Buffered downloads validate SHA-256 and byte count before
release. No reusable signed URL bypasses later membership or grant checks.

REST and SDK content limits are 1 MiB per artifact, 256 retained artifacts per
Agent and 50 records per page. Publication and read operations have bounded
lifetimes; current/future readers may access published content, and revocation
cannot recall copies already downloaded. UI publication explicitly acknowledges
this exposure and retains the original key/body through an uncertain retry.

## Explicit channel usage

Channel usage is an additive sidecar bound to retained Agent and parent lineage.
It never rewrites creation origin or grants resource rights. Authorized readers
receive safe active/history flags and first/last actual accepted-command times.
Attachment records a binding event without inventing command use. Only current
`manage_bridge` authority can read binding references and paginated immutable
history, including actor, timestamp, reason and before/after generations.

Memory and Mongo apply sidecar state and immutable events atomically. Active
binding references are bounded to 16; history remains paginated without eviction.
Service reads recheck current resource policy and the original membership deadline
after storage IO. Inventory joins the validated channel state with creation facts
inside its bounded storage snapshot, applies filters before returning a page, and
retains the existing final resource and membership checks. Broad rows never carry
binding IDs, owner credentials or raw external actor/partition identifiers.

These read contracts are implemented; bridge dispatch must still connect actual
accepted commands and lifecycle changes to the durable mutation path. Public
callers cannot assign or clear channel-use attribution.
