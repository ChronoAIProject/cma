# Resource authorization foundation

Milestone 17 separates retained resource ownership from the principal executing
an action. The implementation in `layer2-core::resource_access` defines the
policy and trusted adapter contracts. Organization/share persistence, public
resource projections, stream/ticket revalidation, and the complete background
command scheduler are still being connected. This document does not establish
that organization sharing or public queued execution is available.

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
Existing personal Agent read/operation boundaries use the common personal
policy with their retained 404 and capability semantics. General organization
and share access must be connected to fresh ownership/grant stores before it
is exposed on resource routes.

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
