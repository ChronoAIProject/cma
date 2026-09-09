# Services And Connection Assist

Service authority belongs to the **Sandbox**. Every Agent in that Sandbox uses
the same scoped NyxID agent key through the local broker. Agent Profiles select
behavior, not another service grant; Workspace creation and runtime rotation do
not widen an existing Sandbox's reach.

[CMA Model](CMA_Model.md) describes the profile and credential boundaries.
[REST](API_REST.md) and [CMA Trigger](CMA_Trigger.md) define public request shapes.
This document distinguishes service requirements, connection guidance and the
actual scoped credential. None is a substitute for the others.

## Scope And Provisioning

A Sandbox Profile records `nyxid_services`. Admission unions that set with
`CHRONO_BASELINE_SERVICES` and pins the resulting scope on the Sandbox. The
checked-in baseline is `chrono-llm,ornn-api,ornn`. Later profile edits, retirement,
publication or operator-default changes do not rewrite the instance's grant.

```text
Sandbox profile scope + deployment baseline
  -> pinned Sandbox authority
  -> software install without credential or credential-bearing proxy
  -> owner-scoped key issuance and durable encrypted custody
  -> generation-fenced credential activation
  -> Workspace setup and per-Agent conversations through the broker
```

CMA obtains the owner's delegated credential server-side. It mints a separate
agent key for the Sandbox, using the pinned service selection, and retains
its material in encrypted custody rather than in the public aggregate. The
aggregate records nonsecret scope, generation and provider authority metadata.
Duplicate mint races reconcile to one custodied generation and record revocation
of a losing provider key. Retirement and renewal also use recorded custody and
revocation work; a lost HTTP response is not proof of provider revocation.

Software installation completes first, as the nonroot `cma` user. The scoped
credential and credential-bearing loopback proxy become available afterward.
Workspace clone/init and setup then run, followed by per-Agent home preparation.
The global environment contains configuration and an inert broker token, never
the real service credential. The real value is delivered to the runtime's
credential holder over its private transport.

A profile is not a credential store. Environment values, system prompts and
commands are public/disclosed content subject to reserved-name and secret
validation. Service names and ids may appear in owner-visible requirements;
key values, delegated tokens, provider refresh tokens and unredacted rejected
values must not appear in logs, event history or browser storage.

## What Requirements Mean

`GET /api/v1/launches/{id}/requirements` checks as the Launch owner, not its
publisher. Its `services` list retains each required service id and a typed
state. Active usable grants satisfy requirements; inactive, ambiguous, missing
or unknown provider rows do not. A provider outage must not be converted into
an empty successful grant list.

Launch admission and explicit Start both recheck the service list in the
Launch's frozen Trigger Sandbox Profile, including when reusing a live or
paused Sandbox or an existing Agent. An unmet connection keeps the same Draft
and resolved hierarchy available for connection assist. A revoked connection
between admission and Start blocks work until a successful recheck. Replaying
an already admitted command returns its recorded result without starting or
checking again. These checks do not broaden an existing Sandbox's pinned grant.

Current optional guidance appears **inside each service entry**:

| Field | Meaning |
|---|---|
| `display_name` | Validated current catalogue name when available |
| `connection_mode` | `hosted`, `manual` or `unknown` |
| `setup_url` | Validated provider setup destination for a manual connection |

CMA derives hosted/manual mode from explicit active catalogue metadata. An
active HTTP service can use hosted connection assist; an active SSH service is
manual. Missing, inactive or unknown types remain unknown. Do not infer service
type from an id or slug, and do not invent a provider setup URL in the frontend.
Metadata describes a possible next action, not authorization. A missing name
can fall back to the service identifier without changing its state.

The guidance is a live projection and does not rewrite the persisted Launch
requirements snapshot, event history or already issued Sandbox scope. Reading
requirements does not start, retry, cancel or replace any Agent.

## Hosted Connection Handoff

Create with `POST /api/v1/service-connections`, the documented
`CreateServiceConnectionRequest` and `Idempotency-Key`. The request belongs to
one caller-owned Launch and one required service. A successful 201 returns
`handoff_id`, `hosted_url` and `expires_at`.

The adapter requests a 15-minute NyxID connect link as the visitor, not with
operator credentials. It validates the returned provider id, timestamp and
hosted URL: one configured HTTPS browser origin, the supported connect path,
no user information, query or fragment, and no arbitrary redirect. Provider
HTTP requests have a 15-second bound, no redirect following, a 1-MiB decoded
response bound and at most 1,000 catalogue/service rows.

Open the hosted URL only from the visitor's explicit connection action. It is
short-lived handoff authority, not a destination to place in logs, analytics,
profile content, persistent storage or error messages. A browser may use a
popup or an explicitly initiated same-tab fallback. In the latter case,
persist only the bounded handoff identity and correlation/expiry metadata,
never the hosted URL or a service credential.

Poll `GET /api/v1/service-connections/{handoff_id}`. Only the owner can read it.
A completed NyxID link is insufficient on its own: CMA re-reads UserService
state and verifies the exact returned id and slug describe a unique usable
service. Only then is the handoff satisfied. A closed popup, a browser return,
a successful sign-in or a local timeout is not proof of a grant.

Preserve the handoff's key/request until creation settles. A transport failure
must not mint another link automatically. Stop polling at expiry, cancellation,
route change or a terminal outcome; bound in-flight reads and let the visitor
initiate a supported new attempt when necessary. On return, refresh requirements
and continue the same Draft through its explicit Start operation.

Pending hosted setup keeps Connect and Verify available. Connect reopens the
current unexpired in-memory link without minting another handoff. Verify reads
the exact handoff immediately and reports when access is still pending; popup
closure alone is never success. After reload, the hosted URL is intentionally
absent from storage, so another explicit Connect can request a new link. Starting
that replacement aborts the previous poll, and late results cannot satisfy the
new attempt. Simultaneous satisfaction of several services continues the same
Draft once; the backend still rechecks every service before admitting work.

## Connection Outcomes

| Observation | Required client behavior |
|---|---|
| Pending handoff | Poll with a bounded client interval until expiry; do not start automatically |
| Satisfied handoff | Refresh requirements; only a still-valid explicit consent can admit Start |
| Failed, cancelled or expired | Keep the Launch; present its recorded result and supported next action |
| Manual service | Offer only the server-provided setup destination and refresh requirements on return |
| Unknown/unsupported type | Show uncertainty; never claim it is connected or silently choose a protocol |
| Provider or identity outage | Preserve state and report unavailable; no optimistic satisfaction |
| Missing owner credential | Reauthenticate through CMA's existing identity flow |
| Missing/foreign handoff | Concealed 404; do not attach another owner's handoff |

The adapter does not invent a custom OAuth callback, device-code exchange or
API-key entry form. NyxID's hosted UI owns those protocol details. CMA only
creates the link, validates its origin and polls the owner-scoped result.
Registration and visitor role administration likewise belong to NyxID operators,
not a CMA callback. See [Operations](CMA_Operations.md).

## Failure And Recovery Boundaries

Credential refusal, dependency unavailability and transient runtime failure are
different facts. A missing grant requires a connection or permission change;
repeating the same provision is not a grant. A provider/network outage can be
retried within recorded operation identity. A stale generation or contradictory
custody record fails closed rather than installing guessed credentials.

The baseline service set must be usable for the intended visitor account.
A role that grants `cma:trigger:launch` but cannot obtain required services does
not prove a working public journey. The real baseline-only fresh-visitor gate
must exercise key issuance, installation order and Agent output.

A runtime rotation preserves Sandbox authority. It does not re-read a changed
profile to broaden scope or replace the Agent with an unrelated conversation.
Credential renewal and checkpoint recovery must be observed against the final
runtime digest, including a working Agent and a rejected unauthorized call.

## Evidence Register

Deterministic tests cover owner isolation, malformed provider responses, bounded
transport, explicit service-type metadata, short-lived handoffs, exact active
service rechecks, duplicate credential custody and generation/revocation fences.
These establish implementation invariants, not the production identity-provider
configuration or real user consent experience.

Final evidence must record OAuth, API-key and device-code hosted journeys;
manual/unknown guidance; missing/inactive/revoked grants; expired/cancelled links;
popup blocking and same-tab return; NyxID unavailability; fresh visitor baseline
access; credential renewal while working; and pause/revive/rotation with the
same Agent identity. Historical provider experiments are retained in
[Sandbox Provider](Sandbox_Provider.md) with their version boundaries.

Do not claim these production journeys passed until the [Operations](CMA_Operations.md)
register names their tested commit/image, redacted result and cleanup. There is
no legacy GitHub contribution-policy, pull-request writer or session-wide reach
contract in this release.
