# Reference

<!-- nav: 5 Reference -->

`Client::builder(base_url)` returns a `ClientBuilder`. The builder accepts an
optional request timeout and bearer token, and produces a cloneable `Client`.
The timeout covers both response headers and the complete body. Reverse-proxy
path prefixes are retained; resource identifiers and query values are encoded
as data, not interpolated into request URLs.

`Client::read_conformance()` performs `GET /api/v1/conformance` anonymously and
returns `Conformance`, containing `service`, `status`, and the currently exposed
`interfaces`.

| Resource Group | Operations |
| --- | --- |
| `agent_profiles()`, `workspace_profiles()`, `sandbox_profiles()` | `list`, `read`, `create`, `save`, `publish`, `list_revisions`, `read_revision`, `set_visibility`, `retire` |
| `marketplace::<C>()` | `list`, `read`, `fork` |
| `sandboxes()` | `list`, `read`, `create`, `delete`, `pause`, `revive`, `events` |
| `workspaces()` | `list`, `read`, `create`, `delete` |
| `worktrees()` | `list`, `read` |
| `agents()` | `list`, `read`, `create`, `input`, `steer`, `interrupt`, `answer_approval`, `stop`, `delete`, `events`, `transcript`, `usage`, `resources` |

`C` is one of `AgentProfileContent`, `WorkspaceProfileContent`, or
`SandboxProfileContent` from `cma_sdk::profiles`. Profile drafts and immutable
revisions keep the corresponding content type. `save` requires the exact
`draft_version`, replaces content, and retains omitted name or description.
`publish` requires the exact draft version. `fork` requires an explicit immutable
revision rather than selecting a moving latest revision implicitly.

Owned profile drafts require profile-management authority. Exact published
revision reads also support accessible public and owner revisions with the
service's resource-use authority; authorization is always decided by CMA.
Marketplace reads validate the requested kind, identity, revision consistency,
and required Sandbox trust metadata.

`PageOptions` holds an opaque `cursor` and optional `limit`. Owner lists and
marketplace results return `ResourcePage<T>`, whose `next` is the exclusive
cursor for the next call. Child Workspace and Worktree lists return
`ResourceList<T>`. `SandboxListOptions` and `AgentListOptions` add typed filters;
`MarketplaceListOptions` adds search text.

`EventOptions` holds an exclusive durable sequence `after` and optional `limit`.
Sandbox events return `Vec<SandboxEvent>`; Agent events return
`ResourcePage<AgentEvent, u64>`. Persist processed sequence numbers before
advancing a polling cursor. These are bounded durable reads, not live feeds.

Creation returns HTTP 201 resources without claiming readiness. Sandbox
lifecycle and Workspace deletion return HTTP 202 resources. Agent input returns
HTTP 202 with `InputAccepted { queued }`; other Agent commands accept an empty
HTTP 202 response. Agent resources report Sandbox-wide counters, with `None`
when a counter is unavailable.

Mutations require `&IdempotencyKey`. The constructor accepts 1 to 255 visible
ASCII characters without spaces. Invalid identifiers, zero revision numbers,
and zero Sandbox profile resource allocations are refused before HTTP.
Unknown status or event tags, malformed JSON, wrong resource identities, and
unexpected success statuses are protocol errors. Response bodies are limited
to 8 MiB even when the peer omits `Content-Length`.
