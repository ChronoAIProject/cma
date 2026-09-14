# Resource authorization foundation

Milestone 17 separates retained resource ownership from the principal executing
an action. The implementation in `layer2-core::resource_access` defines the
policy, transactional ownership/share stores, and authenticated share-management
and effective-access routes. General shared reads, interaction, stream/ticket
revalidation, and the complete background command scheduler are still being
connected. These management routes do not yet make shared Agents usable by
recipients across the existing interfaces.

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
now enforce these permissions. Organization Agent inventory is also composed.
Shared hierarchy views, streams,
artifacts and interactions remain required across the rest of the product.

OpenAPI and repository TypeScript/Rust SDKs expose all six operations. The SDKs
retain explicit keys/revisions and never automatically resend an uncertain
mutation. Shared contract fixtures replay 43 actual-handler cases in both SDKs.
No SDK package publication, UI workflow or production rollout is claimed here.

## Agent REST read projections

Production composes one ResourceAccessService for management and Agent detail,
transcript, event, usage and resource reads. The authenticated request context
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

## Deferred personal commands

`PersonalCommandAdmissionAuthority` requires the exact invoking credential to
match CMA's durable encrypted subject custody and verifies current provider
permissions. A bearer with no matching custody receives a typed refusal. The
nonsecret command authority ID binds configured issuer, subject and sign-in
incarnation through a length-delimited hash. An unrelated credential or a later
sign-in cannot adopt admitted commands, even when token text is reused.

`AgentCommandAuthorizer` refreshes within that same incarnation, verifies the
provider's current subject and capabilities, and fences the result against the
current token generation. A refresh that loses to a new sign-in cannot use the
new login. Revocation and verification outage have distinct outcomes. Relay,
service-account and delegated-actor tokens are refused by this personal path.

`AgentOperationService::execute_authorized_claim` obtains that actual verified
caller before attempting runtime dispatch. The claim is checked after provider
IO, and the conversation transaction still requires the exact live durable
lane, fence and command input. Losing the lease during verification produces no
runtime write. A command with an existing dispatch binding is reconciled from
its log and cannot be executed again.

The public admission adapters and scheduler do not yet invoke these ports.
Existing personal Agent read/operation boundaries retain their personal checks.
Configured legacy Agent readers now check the explicit persisted owner kind;
an org owner cannot become a personal owner through its raw subject string.

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
