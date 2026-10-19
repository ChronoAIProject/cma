# CMA Trigger

A CMA Trigger is an external button that brings its visitor to an Agent of
their own, in their own Sandbox. It never grants access to the publisher's
Sandbox or Agent. The publisher selects three profile revisions, reuse policies,
presentation and a first message. CMA owns the durable visitor Launch.

The [CMA Model](CMA_Model.md) defines ownership and trust. The full wire surface
is in [REST API](API_REST.md); operational enablement and evidence belong to
[CMA Operations](CMA_Operations.md).

## Publisher Configuration

A Trigger has a mutable draft with `draft_version`, immutable published
revisions and their content digests, and `draft|published|retired` availability.
First publication assigns a stable `public_id`. Later publications change the
latest preview, not the link or a Launch already admitted from an earlier
revision.

```json
{
  "presentation": {
    "title": "Research with trureturing",
    "description": "A prepared research workspace.",
    "button_label": "open_in_cma",
    "theme": "auto"
  },
  "target": {
    "sandbox_profile": { "id": "sbp_example", "revision": 1 },
    "workspace_profile": { "id": "wkp_example", "revision": 1 },
    "agent_profile": { "id": "agp_example", "revision": 1 }
  },
  "reuse": {
    "sandbox": "reuse_existing",
    "workspace": "reuse_existing",
    "agent": "ask"
  },
  "first_message": "Inspect the research repository and summarize its current state.",
  "consent": { "always_review": false }
}
```

The example ids are placeholders; use real published profile revisions.
Presentation title is 1-120 characters, description at most 2,000,
optional `signup_hint` at most 200, and first message at most 32 KiB.
The four button labels are `open_in_cma`, `chat_with_this`, `run_with_cma`
and `try_it`. Theme is `auto`, `light` or `dark`. All presentation is
plain text, never publisher-provided HTML.

Publication resolves every reference and rejects private/unusable referenced
profiles. A published Trigger prevents retirement of profiles it still needs.
The selected Workspace Profile may be among the Sandbox Profile's initial
Workspaces or an independently selected public Workspace Profile. The same
principle permits selecting the intended Agent Profile without copying or
mutating its content.

## Reuse: Cases a, b And c

| Maintainer case | Sandbox policy | Workspace policy | Agent policy |
|---|---|---|---|
| a: new Sandbox | `always_new` | Applied inside the newly provisioned Sandbox | `always_new`, `ask` or `resume_existing` as configured |
| b: new Workspace in an existing Sandbox | `reuse_existing` | `always_new` | As configured |
| c: Agent in an existing Workspace | `reuse_existing` | `reuse_existing` | `always_new`, `ask` or `resume_existing` |

Reuse is evaluated only against the visitor's eligible resources. Sandbox and
Workspace matching uses the profile **id**, not its revision: an existing
instance retains its old pinned snapshot. Missing resources are created.
A paused reused Sandbox is revived before work proceeds.

When a new Sandbox provisions the Workspace already named by the target
profile, resolution checks that new Sandbox again and reuses the profile-created
Workspace where the policy permits it. It does not blindly create a duplicate.

Agent `resume_existing` selects the visitor's newest eligible live Agent from
the same Trigger, or a matching pinned Agent in the selected Workspace. The
Workspace Profile's automatic Agent and the Trigger share one admission slot
when the Trigger requests reuse. An untouched Agent receives the first message
once; an existing conversation is preserved. Durable reuse receipts retain the
same Agent across retries and concurrent owner activity.
`always_new` creates a new Agent. `ask` with an existing candidate returns
`409 trigger_repeat_choice_required` carrying
`existing: { agent_id, title, created_at, first_message_excerpt }`.
The client presents Continue where you left off or Start fresh and submits
the explicit `choice: "resume" | "new"`. The title may be null or absent.

An eligible resume can short-circuit environment creation. Therefore the wizard
must describe all three policies together; a Sandbox setting alone is not a
promise to replace an Agent that the Agent policy explicitly resumes.
Resumption still enters durable readiness checks: a paused Sandbox is revived,
the Agent must become available, and only then is the Launch ready. Reopening
an existing Trigger conversation preserves its original profile pins even if
the publisher has since changed the Trigger target; it never resends the first
message.

## Publishing Workflow

1. Create and publish the Agent, Workspace and Sandbox Profiles. Inspect the
   complete nested configuration and make every referenced profile publicly
   usable before publishing the Trigger.
2. Open `/triggers/manage`. Select exact revisions in Profiles; choose reuse
   behavior in Reuse; edit title, description, button, theme, first message and
   review preference in Presentation.
3. In Where it lives, add each source URL and label. Placements are HTTPS URLs,
   at most 512 bytes, with no user information; fragments are stripped.
   Labels are at most 80 characters, with at most 32 live placements.
4. Save the draft, then explicitly Publish its current version. Saving never
   publishes. A version conflict retains the editor for deliberate reconciliation.
5. Open the embed panel, select a placement, and use the server-returned snippet.
   Inspect both badge themes. Stats distinguish total Launches and ready Agents
   and include placement attribution.

Publisher routes are `/triggers/manage/{id}` and
`/triggers/manage/{id}/embed`. Refresh and browser history restore those views
without saving, publishing or launching.

### Publisher REST

All mutations require a caller-supplied `Idempotency-Key`. Keep one key and
exact payload until the outcome is known; a transport failure does not authorize
a new write. Publishers need the public publishing capability.

| Method | Path | Body or result |
|---|---|---|
| POST | `/api/v1/triggers` | `{ content }`; 201 Trigger draft. |
| GET | `/api/v1/triggers` | Owner page, `cursor` and `limit`. |
| GET | `/api/v1/triggers/{id}` | Owner Trigger, draft version and availability. |
| PUT | `/api/v1/triggers/{id}` | `{ content, draft_version }`; version-checked save. |
| DELETE | `/api/v1/triggers/{id}` | Remove an owned Trigger from authoring; 204. |
| POST | `/api/v1/triggers/{id}/publish` | `{ draft_version }`; 201 immutable publication. |
| POST | `/api/v1/triggers/{id}/retire` | Retire new use without deleting existing resources. |
| GET | `/api/v1/triggers/{id}/revisions` | Immutable revisions. |
| GET/POST | `/api/v1/triggers/{id}/placements` | List, or create `{ source_url, label }`. |
| POST | `/api/v1/triggers/{id}/placements/{placement_id}/retire` | Retire one placement. |
| GET | `/api/v1/triggers/{id}/embed?placement_id=...` | Copy-ready descriptor. |
| GET | `/api/v1/triggers/{id}/stats` | Total and placement counters. |

DELETE physically removes a never-published draft. For a published or retired
Trigger it archives the definition: owner reads/listing no longer return it,
new Launch admission is refused, and edits/publication are unavailable. Its
public mapping and immutable revisions remain available internally for existing
Launches, including admitted visitor drafts; their resources and history are
unchanged. Exact successful DELETE receipts still replay after removal.

Archived definitions retain `retired` availability and an optional `archived_at`
timestamp. Existing records without that field remain readable. Once any
archive is written, rollback binaries must retain this reader and owner-list
filter; an older strict reader cannot safely interpret archived aggregates.

## Host Snippets

The authoritative descriptor includes `public_url`, `badge_url`,
`label_text`, `javascript_sri`,
`content_security_policy: { img_src, script_src }` and
`snippets: { markdown, html, javascript, react, url }`.
Its origin is the validated `CHRONO_SERVER_PUBLIC_URL`, never a request Host
header or publisher-supplied origin. Clients render these strings verbatim.

The following examples are the checked-in descriptor fixture at
`https://cma.example`, not a live published Trigger. For deployment use the
actual descriptor, including its exact SRI value.

### README Or Markdown

```markdown
[![Open in CMA](https://cma.example/api/v1/triggers/public/pub_example/badge.svg?label=open_in_cma&theme=auto&placement=plc_readme)](https://cma.example/api/v1/triggers/public/pub_example/open/plc_readme)
```

A README renderer must allow linked images. GitHub camo fetches the image without
the visitor's cookies or referrer; the image response therefore contains only
public badge content. Click attribution comes from the destination/placement,
not an assumption that the image proxy forwarded a browser header.

### Static HTML

```html
<a href="https://cma.example/api/v1/triggers/public/pub_example/open/plc_readme"><img src="https://cma.example/api/v1/triggers/public/pub_example/badge.svg?label=open_in_cma&amp;theme=auto&amp;placement=plc_readme" alt="Open in CMA"></a>
```

No JavaScript is required. The image has an accessible label and links to the
same read-only landing as every other host.

### JavaScript

```html
<script src="https://cma.example/embed/v1.js" integrity="sha384-K56ODoEc3ZTiX/9Xfe3xk5PEm0Q+bQw5brGMyIfL3hGTeREM9Xf7zvR/7nn1XGuI" crossorigin="anonymous"></script>
<cma-trigger trigger="pub_example" placement="plc_readme" label="open_in_cma" theme="auto" origin="https://cma.example"></cma-trigger>
```

The script defines `<cma-trigger>` and upgrades `<a data-cma-trigger>`.
Required attributes must be valid before anything renders. It does not call
CMA APIs, read cookies or start a Launch. It uses no eval.

Add the exact descriptor directives to the page's own CSP. A minimal fixture
policy is:

```text
default-src 'none'; img-src https://cma.example; script-src https://cma.example
```

Do not weaken the host's CSP with arbitrary wildcard origins or remove SRI to
make a mismatched script load. Obtain a fresh descriptor after an embed asset
change and verify the supplied hash against the served script.

### React

```tsx
import { CmaTriggerButton } from '@chronoai/cma-sdk/react';

<CmaTriggerButton triggerId="pub_example" placementId="plc_readme" label="open_in_cma" theme="auto" baseUrl="https://cma.example" />
```

This JSX needs no CMA credential. Use the server-returned React snippet with
the repository-only SDK's `react` subpath and the host application's React peer
dependency. The component renders an anchor and image without fetching data or
using browser globals; server rendering and mounting never submit a Launch.

### Just The Link

```text
https://cma.example/api/v1/triggers/public/pub_example/open/plc_readme
```

## Visitor Admission And Consent

`GET /api/v1/triggers/public/{public_id}` returns read-only presentation,
availability, server-composed reuse summaries, public software disclosures,
a technical disclosure and a first-message excerpt. It does not expose
publisher-private data or perform any provisioning.

The external button links to
`/api/v1/triggers/public/{public_id}/open/{placement}`. This route only redirects
to `/t/{public_id}/{placement}`. A browser-confirmed deliberate document click
carries a ten-minute, host-only HttpOnly intent cookie; prefetches, scanners,
and generic page loads do not. The landing consumes it through the public
same-origin `POST /api/v1/triggers/public/{public_id}/entry-intent` endpoint.
For a no-review Trigger that retained click is the explicit start action, so
sign-in returns directly to automatic provisioning without another button.
A signed-out visitor's bounded intent retains the target and command identity
through sign-in. An OAuth callback alone never authorizes work. Direct landing
visits and publisher-reviewed Triggers retain the explicit landing action.

```http
POST /api/v1/triggers/public/pub_example/launches
Content-Type: application/json
Idempotency-Key: visitor-intent-example

{
  "placement_id": "plc_readme",
  "start_if_ready": true
}
```

Use the authenticated visitor's cookie or bearer credential; never a publisher
credential. Optional `referrer_origin` is validated attribution, not access
control. A valid placement identifies a declared host; unknown placement cannot
grant additional authority.

For a no-review Trigger, `start_if_ready: true` explicitly accepts the frozen
configuration, including its exact command/environment trust digest. Connection
requirements still must be satisfied; publisher-required review is not bypassed.
That explicit click is retained as the Launch's single `landing` confirmation
even while its state is `draft` and connections are missing. Provisioning and
the first message remain unstarted. Connection rechecks retain the original
confirmation and timestamp; they must not ask for the same software trust again.
If all requirements are satisfied, the same authenticated POST records consent
and returns 202 with an admitted Launch. Otherwise it returns 201 with the
durable draft and `review.needs`. Review can require service connections,
software trust, publisher-required review or a repeat choice. Show only needs
that actually exist. Trust displays the exact server-provided commands and
environment values, with explicit acknowledgement.

After satisfying needs, continue through the owner-only Start operation:

```http
POST /api/v1/launches/lch_example/start
Content-Type: application/json
Idempotency-Key: visitor-reviewed-start-example

{
  "trust_confirmed_digest": "<exact Launch trust confirmation digest>",
  "acknowledgements": []
}
```

Omit the trust field when no trust is needed, including a no-review draft whose
landing confirmation already accepted the frozen digest. Keep acknowledgements
empty for that continuation; Start preserves, rather than replaces, the recorded
confirmation. Use the Launch trust manifest's
confirmation digest, not a guessed profile digest or a digest computed from
display text. Start rechecks current connections and requirements before
admission. Replaying the original create key returns its original response;
it is not a substitute for Start and does not refresh a recorded draft.

The recorded `checking_requirements` step stays `blocked` while any service is
missing, inactive or unknown. Partial verification never marks it complete.
Successful Start verification records completion and its timestamp; losing a
required connection during preparation records it as blocked again. These step
changes use the same durable Launch events as resource preparation, so reloads,
event streams and other public clients see the same status.

Rollback compatibility: older API validation rejects an unstarted Launch with a
recorded confirmation. Once this release admits such a draft, any rollback API
must retain this validation support, including for cancelled unstarted Launches.
Do not erase confirmations, fabricate a start or delete Launch history to make
an older binary accept the records. No serialized field was added.

## Accounts And Connections

`GET /auth/signup-options` has exactly four fields:
`social_providers`, `email_signup`, `invite_code_required`,
`register_available`. Failed upstream discovery returns a closed response;
a UI must not promise open registration from stale assumptions. Create account
uses `/auth/login?screen=register` with a validated canonical local return.
NyxID public registration and its default visitor role are operator
prerequisites; CMA does not change them.

`GET /api/v1/launches/{id}/requirements` returns
`{ services, checked_at }`. Each entry has `service` and state
`satisfied|inactive|missing|unknown`. Current entries may additionally have
`display_name`, `connection_mode: hosted|manual|unknown` and `setup_url`.
Absent metadata is omitted, not null. Enrichment belongs to this current GET
projection only; stored Launch snapshots and history are unchanged.

The UI labels this step **NyxID Service Connection Verification**, before
Sandbox assignment or revival, Workspace preparation, and Agent provisioning.
Every required service must be satisfied, including launches reusing prepared
warm environments or existing conversations. An active routing row alone is
insufficient: CMA reads the exact user's key connection state, credential
presence, access and node availability. For GitHub, NyxID's readiness projection
also detects an existing credential row with no credential material; assistant
tool grants are independent of this connection check.

Existing bindings link directly to their NyxID `/keys/{id}` page for repair.
Each disconnected service offers Connect and Verify. Users finish setup in
NyxID, return to the same CMA Launch, and explicitly Verify; background polling
does not start a Sandbox. Partial completion keeps that Launch in draft and
retains its consent. Start freshly verifies all services before any launch work.

Do not infer connection modes from a slug or interpret unknown/inactive as
satisfied. Hosted setup creates an owner-scoped handoff:

```http
POST /api/v1/service-connections
Content-Type: application/json
Idempotency-Key: visitor-connect-example

{
  "service_id": "service-example",
  "launch_id": "lch_example",
  "intent_id": "connection-intent-example"
}
```

The 201 response supplies `handoff_id`, `hosted_url` and `expires_at`.
Open a blank popup directly from the visitor's gesture, then navigate only to
the validated hosted HTTPS destination. Never read its cross-origin contents.
Poll `GET /api/v1/service-connections/{handoff_id}`; states are
`pending|satisfied|expired|cancelled|failed`. Satisfied requires an active
visitor service connection, not merely a completed authentication screen.

The handoff has a bounded lifetime. The assistant's initial poll cadence is
1.5 seconds with backoff and a five-minute UI bound. Popup blocking offers a
same-tab destination and an explicit recheck. Manual services use the supplied
safe NyxID setup URL and a recheck, not a fabricated hosted connection flow.
Expiration requires an explicit new attempt. CMA never asks the visitor to
paste a service credential into its own UI.

## Progress And Recovery

```text
draft -> resolving -> sandbox_pending -> workspace_pending
      -> agent_pending -> message_pending -> ready -> completed
```

The model also records `failed_retryable`, `failed_terminal` and
`cancelled`. Blocked reasons distinguish missing connection, orchestration
paused, Sandbox revival and required recovery. Paused orchestration freezes
work at a recorded boundary; it is not a failed Launch.

The seven step keys are `signing_in`, `checking_requirements`, `sandbox`,
`workspace`, `agent`, `first_message`, `ready`. Each has recorded
`pending|running|completed|blocked|retryable|failed` status, safe detail and
optional timestamps. A timer must not advance a step.

Use `GET /api/v1/launches/{id}` for the owner snapshot and
`GET /api/v1/launches/{id}/events?after=0&limit=50` for durable pages.
The read-only SSE companion is
`GET /api/v1/agui/launches/{id}/events?starting_after=0`.
An SSE id is the Launch event sequence, not an Agent conversation cursor.
Advance only after the consumer acknowledges the event. Reconnect or poll
from that cursor and retain the last successful view during an outage.
The companion also supports `follow=false`, a bounded `limit`, and
`Last-Event-ID` precedence; the TypeScript SDK uses the query cursor.

Owner Launch reads and lists may include `first_message_command` with exactly
`agent_id`, `command_id`, and `response_id`. It is the immutable association for a
new common first prompt, derived from the accepted permanent receipt. It is
omitted for legacy Launches, prompts not yet attached, and reuse that attached no
new prompt. Omission does not authorize sending a first message again. Previously
cached Launch admission responses keep their original shape; use the owner read
to refresh the association.

Use `GET /api/v2/responses/{response_id}` and its events extension for the common
response's current status and retained outcomes. These reads independently enforce
current Agent history authority; the Launch association grants no execution or
read capability. It contains no prompt, initiating credentials, grants, internal
binding digest, or runtime identity. Ambiguous delivery retains this same response;
reconnect and recovery observation do not submit another prompt. This projection
does not enable the default-off Trigger command selector or establish old-worker
rollout/rollback compatibility.

Ready supplies the visitor's Agent id. Navigate to `/agents/{id}` and focus
the composer. First-message delivery is separately recorded; ready is not
evidence that a possibly ambiguous dispatch can be sent again.

Before readiness, `POST /api/v1/launches/{id}/cancel` cancels remaining work,
not a Sandbox already created. Failure presents supported remediation and a
copyable support code. A retryable flag is not a retry endpoint: the current
public API offers status reads and a new explicit intent, not a worker-retry
command. Do not silently resubmit a first message or restart a failed operation.

## For Your Visitors

The Agent belongs to the person clicking the button. Their work is not a
contribution to the publisher's repository, and the button grants the publisher
no access to it. The visitor can inspect what will be installed, which services
are needed, and whether existing work is reused before admitting the relevant
action.

Account creation takes place in NyxID. A visitor may launch and use/delete
their own Sandboxes without receiving direct Sandbox-create, profile-management,
publishing or administrator permissions. Default limits are counts, not prices;
a quota refusal includes the supported reset information.

Closing the page does not cancel admitted work. Returning to its owner-only
progress URL restores the recorded state. A disconnected popup or browser is
not proof that a write failed. Existing work remains unaffected by a publisher
retiring the Trigger.

## Badge And Embed Threats

Badges read no cookies, set no cookie and contain no private visitor information.
Known public badges use `public, max-age=300, stale-while-revalidate=3600`;
unknown ids produce a generic badge with a short cache lifetime. ETags follow
public revision/label/theme/availability. SVG uses closed labels, inline mark
paths and system fonts, with no script, foreign object, external resource or
publisher title injection. Its CSP, nosniff and referrer headers are explicit.

The embed script is a small static asset served with a content-hash ETag,
one-day public cache lifetime and descriptor SRI. A placement or referrer is
telemetry, never proof that a caller is authorized. All mutations remain
authenticated, owner-scoped, JSON-only and protected against cross-site form
admission. Commands, environment, credentials, origin validation and worker
recovery are covered by the broader [threat model](CMA_Model.md#threat-model).

Host, fresh-account, connection, quota and fault evidence must be established
against the final deployed images as G-T1-G-T3 and journeys J0-J7. A local
snippet fixture does not by itself prove a working public deployment.
