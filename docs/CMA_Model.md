# CMA Model

This is the design authority for CMA's Sandbox model. The maintainer's
terminology in [CLAUDE.md](../CLAUDE.md#21-terminology) and milestone epic
[#800](https://github.com/ChronoAIProject/cma/issues/800) take precedence.
Protocol details belong to the [API overview](API_Overview.md) and
[REST contract](API_REST.md); operations and measured evidence belong to
[CMA Operations](CMA_Operations.md).

## Object Model

```text
Subject owns
  Sandbox sbx_*                         isolated pod, immutable profile snapshot
    Workspace wks_*                     /home/cma/<name>, main checkout
      Worktree wkt_*                    .worktrees/wt-<n>, branch cma/wt-<n>
        Agent agt_*                     exactly one Agent per Worktree

Profile references
  Sandbox Profile sbp_* -> Workspace Profile wkp_* -> Agent Profile agp_*
  Every reference is { id, revision }; every instance pins content and digest.

Trigger trg_* -> stable public id pub_* -> visitor-owned Launch lch_*
  Placement plc_* identifies an embedding page, never an owner or permission.
```

A Sandbox is a resource boundary, not a conversation. Provider, model-access
mode, CPU, memory, software, environment and service scope are Sandbox-wide.
Model, thinking effort, permissions, skills and system prompt belong to each
Agent. Workspaces and Agents may be added from other usable published profiles
within the Sandbox's recorded limits. Worktrees are not shared by Agents, and
Agents never execute in the main checkout.

All descendants carry the same owner and their complete parent lineage.
Creation validates both lineage and parent readiness. Worktree and Agent
admission is atomic: a retry cannot claim a different Worktree or attach a
second Agent. Human-readable names are not authority or reuse identifiers.

Each mutable aggregate has a monotonic revision. A status transition and its
event append share a transaction. An expired worker's lease or fence does not
authorize it to publish into a newer operation. Process-local state cannot
replace durable admission, progress or recovery records.

The runtime records Workspace removal before filesystem I/O. That durable intent
permanently rejects preparation and setup for the same Workspace ID, including
delayed requests, completed-operation replays and recovery after supervisor
restart. A different Workspace ID may reuse the released directory name;
replaying the original removal returns its receipt without deleting the successor.

## Lifecycle States

The source whitelist is `layer2-core/src/sandbox/status.rs`. The following
tables give its accepted transitions; resource services additionally validate
ownership, lineage, active operations and readiness. Repeating a command is
handled by its recorded idempotency result, not by permitting self-transitions.

| Sandbox state | Next states |
|---|---|
| `provisioning` | `installing`, `retiring`, `failed` |
| `installing` | `active`, `retiring`, `failed` |
| `active` | `pausing`, `reviving`, `credential_rotating`, `rotating`, `retiring`, `failed` |
| `pausing` | `paused`, `retiring`, `failed` |
| `paused` | `reviving`, `retiring`, `failed` |
| `reviving` | `active`, `retiring`, `failed` |
| `credential_rotating` | `active`, `reviving`, `retiring`, `failed` |
| `rotating` | `active`, `retiring`, `failed` |
| `failed` | `retiring` |
| `retiring` | `retired`, `failed` |
| `retired` | none |

Only `active` directly accepts new work. A request needing a paused Sandbox
admits revival first. `reviving` also covers recovery of a lost active runtime;
credential rotation is distinct from runtime-image rotation. A paused Sandbox
has no active runtime binding. Failure details exist only with `failed` state.

| Resource/state | Next states |
|---|---|
| Workspace `preparing` | `ready`, `failed`, `removing` |
| Workspace `ready` or `failed` | `removing` |
| Workspace `removing` | `removed`, `failed` |
| Worktree `creating` | `ready`, `failed` |
| Worktree `ready` or `failed` | `removing` |
| Worktree `removing` | `removed` |
| Agent `starting` | `ready`, `failed`, `stopped` |
| Agent `ready` | `working`, `paused`, `stopped` |
| Agent `working` | `ready`, `waiting_approval`, `paused`, `stopped` |
| Agent `waiting_approval` | `working`, `paused`, `stopped` |
| Agent `paused` | `ready`, `stopped` |
| Agent `stopped` or `failed` | `deleted` |
| Workspace/Worktree `removed`, Agent `deleted` | none |

Stopping an Agent preserves readable history. Deleting it removes its Worktree
and Agent home through recorded cleanup. Removing a Workspace and retiring a
Sandbox are explicit lifecycle operations, not an unscoped directory deletion.
An error observing a resource does not change its state to failed.

## Profile Contract

Each profile has an owner, name, description, visibility, mutable draft and
`draft_version`. Names are 1-80 bytes and descriptions at most 2,000 bytes.
Published revisions are immutable and stored separately. Publication atomically
validates the current draft, resolves its references and creates revision
`n + 1`. A draft save is not publication.

The content digest is SHA-256 of UTF-8 JSON with recursively sorted object keys
and no whitespace. Array order is meaningful and preserved. Instances store
`{ id, revision, digest, content }`; neither later edits nor retirement rewrites
that snapshot. References always specify a positive revision of the correct
profile kind, readable by the publisher.

### Agent Profile

| Field | Contract |
|---|---|
| `model` | Optional opaque nonempty model name; omission selects the deployment default during admission. |
| `thinking_effort` | `minimal`, `low`, `medium`, `high`, `x_high`. |
| `permission_level` | `guarded` requests approval for privileged work; `autonomous` runs without provider approval prompts or an inner command sandbox inside the isolated CMA Sandbox. |
| `skills` | At most 32 `{ source: "ornn", id, version? }` entries; per-Agent installation. |
| `system_prompt` | Plain UTF-8 text, at most 32 KiB, installed verbatim in the Agent home. |

The profile prompt is standing guidance plus the repository's instructions,
not a replacement for repository `AGENTS.md`. CMA does not edit repository files
to install the profile prompt. Missing requested skills fail creation with a
typed refusal; they are not silently omitted.

For the supported Codex provider, `autonomous` is YOLO: `approval_policy =
"never"`, `sandbox_mode = "danger-full-access"`, and ACP mode
`agent-full-access`. `guarded` retains `on-request`, `read-only`, and ACP mode
`read-only`. The outer CMA Sandbox and its pinned NyxID service scope remain
the isolation and service-authority boundaries. Explicitly selected `nyxid`
or `ornn-agent-manual-cli` skill revisions install into the Agent home in place
of only their corresponding bundled fallback links.

### Workspace Profile

| Field | Contract |
|---|---|
| `location` | `{ type: "git_repository", url, reference? }` or `{ type: "directory", name }`. |
| `name` | One path component under `/home/cma`; may be derived from a repository name. |
| `setup_commands` | Ordered, at most 16, each at most 4,096 bytes. |
| `agent_profiles` | Ordered references to at most eight published Agent Profiles. |

Repository URLs name public repositories at `https://github.com/<owner>/<repo>`.
User information, alternate authorities, query strings, fragments and private
repositories are not supported. The optional branch/tag is validated; omission
selects the default branch. Preparation records the resolved commit. An empty
directory is git-initialized and also has a recorded base commit.

Directory names match `[A-Za-z0-9][A-Za-z0-9._-]{0,63}` and cannot start with a
dot. Setup runs after clone/init as the Sandbox user, with its credential
available. A main checkout is never an Agent's working directory.

### Sandbox Profile

| Field | Contract |
|---|---|
| `cpu_millis`, `memory_mib` | Explicit positive resources within the deployment's capacity bounds. |
| `nyxid_services` | Service catalogue identifiers; baseline scope is unioned by the server. |
| `agent_provider` | `codex`, the only supported value. |
| `model_access` | `chronoai_managed` or `own_credentials`. |
| `max_workspaces`, `max_agents_per_workspace` | Each 1-16, frozen on the Sandbox. |
| `workspace_profiles` | Ordered published references, count no greater than `max_workspaces`. |
| `software_install_commands` | Ordered, at most 32, each at most 4,096 bytes. |
| `environment_variables` | At most 64 unique `{ name, value }` pairs; nonsecret configuration. |

Command strings contain no NUL. Multiline commands require a complete heredoc.
Environment names match `[A-Z_][A-Z0-9_]*`; `PATH`, `HOME`, `NYXID_URL`,
`NYXID_ACCESS_TOKEN`, `CODEX_HOME` and `CHRONO_*` are reserved. Public values
are checked against deployment credential patterns, and a refusal never echoes
the rejected value. Pattern detection is not permission to store secrets.
Resolved Workspace names must not collide in one Sandbox Profile.

## Marketplace And Trust

Visibility belongs to the profile, not one revision. Public discovery shows
usable published profiles with name, description, author display, revision,
publication time and uses count. Full detail exposes all commands, environment,
services, limits and referenced published content, including nested profiles.
Visibility and retirement do not invalidate existing instance snapshots.

Fork creates a new profile owned by the caller and records `forked_from` with
the original id and revision. It does not create shared editable state.
Marketplace enablement is platform policy, independent of Trigger enablement.

Instantiating foreign commands or environment requires explicit trust over the
server-provided commands digest. A changed or missing digest requires a new
review. The server derives trust from ownership and the actual resolved content;
the client cannot bypass it by changing a display flag. Public visibility does
not imply that software is safe. Published Triggers guard referenced profiles
against retirement while visitors still depend on them.

## Provisioning And Credentials

```text
Admit pinned Sandbox and quota reservation
  -> provision isolated runtime
  -> install software as cma, with no credential or credential-bearing proxy
  -> deliver scoped Sandbox credential and activate proxy
  -> clone/init Workspaces and run their setup
  -> create Worktrees, Agent homes and bridges
  -> publish readiness and deliver any admitted first message
```

Install commands run once under `/home/cma`, not as root. Their receipts and
operation fingerprints distinguish replay from a new installation. Workspace
setup is a different, post-credential stage. Neither failure nor timeout may
silently cross that boundary.

There is one scoped key per Sandbox, containing the profile service set union
the deployment baseline. Domain records store scope and generation, never key
values. Renewal is recorded; a stale generation cannot install a newer key or
expand scope. Provider and model access remain Sandbox-wide, including the
shared own-account sign-in state.

Credential custody records distinguish `managed`, fixed `provided`, and
externally rotated `binding` sources. A binding supplies its key through encrypted
custody; each generation records the SHA-256 of the exact bytes while its source
retains the binding identifier. The supplied key's opaque receipt does
not assert a provider key identifier, verified grant, or expiry. An empty expiry
means unknown only for this source. CMA delivers the same key after installation
and again on revival, and never mints a fallback when its custody is missing.
Its issuer owns rotation and revocation. Retirement removes CMA's encrypted
copy without revoking the externally owned key. Ordinary service-access grants
cannot expand a supplied key's scope. Retained records without a source keep
their existing managed-key behavior.

An own-account Agent records a provider sign-in challenge under its existing
startup claim while authentication is still pending. This publication preserves
the open-operation intent, lease and selected skill material and atomically
marks the Sandbox sign-in state Required. First conversation readiness marks
it Complete in the same transaction as the Agent's readiness and durable
conversation-opened event. Reconnects replay those observations without
inventing a turn. Authentication waits are bounded by both the configured
startup budget and the remaining worker lease; a timeout preserves the visible
challenge and cleanup intent for recovery.

The supported per-Agent layout is:

```text
/home/cma/.cma/provider/codex/auth.json
/home/cma/.cma/agents/<agent_id>/home/
  config.toml
  AGENTS.md
  auth.json -> shared provider auth.json
  skills/nyxid -> /home/cma/.codex/skills/nyxid
  skills/ornn-agent-manual-cli -> /home/cma/.codex/skills/ornn-agent-manual-cli
  skills/<requested-skill>/...
  sessions/...
/home/cma/<workspace>/.worktrees/wt-<n>/
```

The adapter renders these provider conventions; the supervisor only applies
validated plans. The runtime contract is **3**, labelled
`dev.chrono.cma.runtime-contract`. Version or label mismatch fails admission.
The pinned programs are Codex 0.147.0 and codex-acp 1.4.0. Historical Agent-home
evidence is recorded in [Sandbox Provider](Sandbox_Provider.md#8-agent-home-evidence)
and must be repeated on the final rebuilt image, not inherited from an old tag.

## External Bindings

A provider binding records a management principal separately from its resource
owner. Its external profile is mapped to private, published CMA profile revisions;
instances pin those revisions so dashboard profile links remain meaningful.
One binding reserves a Sandbox and Workspace. Each external conversation reserves
one Agent and Worktree within its binding generation. The binding stores the
supplied key's SHA-256; encrypted credential custody owns the secret bytes.

Binding admission, reconfiguration, retirement, and conversation changes use
compare-and-swap revisions. Sandbox, Workspace, and Agent admission rechecks the
binding reservation in the resource transaction. Conversation admission also
checks its exact reservation. A worker that loses a retirement or reconfiguration
race cannot create a late resource. Binding admissions preserve their reserved
Sandbox identifiers and do not claim a warm Sandbox with a different identifier.

The controller discovers durable pending phases after restart. It retains the
previous environment until cleanup completes, stops and deletes conversation
Agents through the existing lifecycle, and reuses compatible immutable Sandbox
and Workspace snapshots. Retired identifiers are tombstones. Supplied credential
admission and cleanup share a binding transaction fence, including when a binding
is deleted before its Sandbox is inserted; a delayed request cannot recreate
cleaned-up key custody. Supplied authority permits child admission and revival
without requiring an unrelated owner refresh token.

Supplied binding key rotation reserves a new encrypted custody generation in the
same transaction as the binding hash and Sandbox delivery work. The Sandbox,
Workspace, Worktree, Agent and external conversation generation remain stable.
The existing credential worker delivers the replacement and retires local custody
for the old generation; it does not revoke the externally owned key. The binding
reports provisioning until delivery completes. A fully paused Sandbox updates its
retained authority for delivery on revival. Busy lifecycle work refuses rotation
until its current operation settles. Replaying an acknowledged rotation never
allocates another credential generation.

## Conversation And Delivery

The Agent owns a durable, append-only, monotonic conversation log. Sandbox
lifecycle has a separate per-Sandbox stream; Launch progress has its own stream.
Clients must not exchange cursors between these resources or projections.
Public projections do not expose worker leases, keys or credential values.

AG-UI `threadId` and Responses `conversation` both identify the Agent. A
Responses id identifies a turn, not another resource hierarchy. Durable text
and outcome projections are replayable. Live text/reasoning/tool observations
are explicitly ephemeral; disconnecting may lose them until durable completion.
Replayed and live-delivered durable content must converge without duplication.

Mutations use explicit command identities and exact payloads. Ambiguous writes
are reconciled against their recorded result; clients never manufacture a new
key merely because a response was lost. Streaming run input is submitted once;
only read-only replay automatically reconnects. A response failure does not
authorize another turn.

First-message admission, dispatch and completion are recorded separately from
Agent readiness. A worker crash cannot silently resend a possibly delivered
message. Ambiguous delivery requires recorded recovery and safe UI guidance,
not a second dispatch disguised as a refresh. An approval is a pending Agent
decision, not a reason to revive or replace the Agent.

## Pause, Revival And Rotation

An idle Sandbox pauses only when all Agents are quiet for the configured
interval. Admission of a message, creation or Launch revives it when required.
The owner keeps the hierarchy, history and identifiers throughout.

A whole-home checkpoint holds all writers under the current fence, captures
installed software, repositories, dirty Worktrees, Agent homes and transcripts,
and seals verified content before publishing recovery authority. Restore checks
the manifest, digests and exact operation identity before releasing the writer
boundary. Partial objects are never advertised as usable checkpoints.

After a pause checkpoint is durably published, subsequent teardown phases
revalidate its exact writer hold through authenticated runtime control. They
compare its epoch, object reference, digest and writer token with the pinned
checkpoint without retransferring the archive. Initial capture still exports
and independently validates the complete archive before publication; invalid
or lost hold receipts retain the existing writer-recovery behavior.

Periodic checkpoints wait until every live Agent's latest durable activity is at
least 120 seconds old. The store enforces this quiet interval inside capture
admission, so a concurrent message wins or the checkpoint wins, never both.
Explicit Pause, runtime rotation, and recovery of an already admitted capture
do not wait for this interval. This grace period permits follow-up work after a
reply; it does not reduce the time needed to save a large home once capture starts.

Runtime image drift performs a recorded rotation through this same boundary.
Only the latest completed checkpoint is retained; its replacement must commit
before the previous archive is pruned. Restart recovery resumes from database
authority rather than the memory of a former worker. A stale worker cannot
resume writers or retire a replacement runtime under an expired fence.

After restored Agents reopen, queue completion rechecks their internal metadata
and lease settlement within five seconds (or the configured retry delay if
shorter). External failures keep the normal retry delay, and an uncertain message
dispatch retains its full grace period. A conflicting completion cannot replace
a newer parent revision, release a replacement worker's lease or discard input.

## Trigger And Visitor Ownership

Triggers select exact public profile revisions and reuse policies. Reuse matches
the visitor's eligible instances by profile id, not a display name, publisher
resource or current revision. A Launch snapshots the selected Trigger revision
and its consent; a later publication cannot change work already admitted.
See [CMA Trigger](CMA_Trigger.md) for the complete a/b/c mapping and wire examples.

Visitors are derived from capabilities: they may launch and use/delete their
own Sandboxes but cannot directly create Sandboxes, manage Profiles, publish
Triggers or administer the platform. NyxID role setup is an operator step,
never a CMA callback or hidden first-party route.

GET, page navigation and OAuth return do not start work. A landing click may
retain a bounded, owner-bound intent across sign-in. The authenticated POST
records that consent. Requirements and connections are checked as the visitor;
inactive or unknown service state does not count as satisfied. Current metadata
can guide hosted or manual setup without rewriting historical Launch snapshots.

## Threat Model

| Boundary | Required protection |
|---|---|
| Public badge | Opaque ids, rate limits, no cookies, no private data, closed labels and safe SVG; caching varies by public content, never by visitor identity. |
| Embed script | Same configured origin, no credential/network API use, no eval, exact SRI and restrictive CSP snippets. |
| Publisher input | Plain text and validated URLs; no publisher-supplied HTML, script, badge font or arbitrary redirect origin. |
| Public software | Full transitive disclosure and digest-bound trust; install before credentials; nonroot home-local installation. |
| Environment | Reserved-name rejection and secret detection; values are publicly disclosed configuration, never credential storage. |
| Repository/skill archive | Validated source, bounded transport, safe paths and integrity; no credential-bearing arbitrary redirects or archive traversal. |
| Visitor admission | Capability checks, counts, retained command identity, same-origin JSON/CSRF defenses and explicit consent. |
| Connection handoff | Owner-scoped service lookup, short-lived handoff, validated hosted origin, active-service recheck; no credential in browser state or DOM. |
| Reuse | Complete visitor ownership and lineage; no selection of publisher resources. |
| Worker/recovery | Transactional progress, exact fingerprints, fences, bounded observations and integrity checks. |
| Telemetry | Placement/referrer are attribution only; request/support codes contain no secret or bearer authority. |

## Settled Decisions

| Decision | Contract |
|---|---|
| S1 | One Sandbox hierarchy replaces separate session and Chat models; no terminal/contribution/sponsorship subsystems. |
| S2 | Provider and model access are Sandbox-wide; Agent behavior is per-Agent. |
| S3 | Distinct provider homes and Worktrees; Agents never use the main checkout. |
| S4 | Software install precedes credentials; Workspace setup follows them. |
| S5 | Public executable configuration is fully visible and requires foreign-content trust. |
| S6 | One scoped credential per Sandbox, profile services union baseline. |
| S7 | Idle pause, demand revival, image rotation and two writer-fenced checkpoints. |
| S8 | Triggers pin three profile revisions; reuse matches profile ids and stable public ids survive publication. |
| S9 | Explicit landing consent; `start_if_ready` may admit in one authenticated POST; GET/callback are inert. |
| S10 | Visitor role and public registration are NyxID operator prerequisites; connect links are polled without a callback. |
| S11 | One descriptor owns Markdown, HTML, JavaScript, React and URL snippets. |
| S12 | Public REST manages resources; Agent conversations have AG-UI and Responses projections; PTY absent. |
| S13 | Counts only, no billing, pricing or monetary allowance. |
| S14 | CI enforces 500-line production source and the explicitly recognized test-file limit. |
| S15 | Implementation and local gates precede merge, direct deployment and final production evidence. |
| G-A3 correction | The pinned bridge does discover added skills; #835 corrects the original negative hypothesis. Profile immutability is a product rule, not a presumed provider cache boundary. |

## Evidence Required For Completion

Every gate records the CMA commit, immutable runtime image digest, provider and
identity-service versions, redacted transcript, result and cleanup. Unit tests
prove local invariants, not deployed readiness.

| Gate | Required observation |
|---|---|
| G-A1 | Two concurrent bridges honor different Agent homes/configuration. |
| G-A2 | Profile prompt and repository instructions both apply in observed order. |
| G-A3 | Initial discovery and actual post-start skill behavior on pinned versions. |
| G-A4 | Shared own-account auth and managed loopback model access. |
| G-A5 | Exact conversation-id capture and restore from the Agent home. |
| G-S1 | Credential/proxy absent during install; installed software survives pause/revive. |
| G-S2 | A fresh visitor reaches a ready Agent with baseline-only services. |
| G-P1 | Restart during pausing/reviving and image rotation preserve two Agents, dirty work and software. |
| G-T1 | README/camo, HTML, strict-CSP JavaScript and React click-through with attribution. |
| G-T2 | OAuth, API-key and device-code service handoffs complete through the assistant. |
| G-T3 | Fifty visitor clicks produce at most five admitted Launches, with typed quota/reset. |

Journeys J0-J7 and the install/clone/Worktree/start/delivery/worker/browser/popup/
identity-outage fault matrix are specified by
[#833](https://github.com/ChronoAIProject/cma/issues/833). Record their actual
outcomes in the operations evidence register. A failed, missing or historical
gate is not a passing final-release gate.

Event ingress and channel delivery belong to the separate
[CMA Event Gateway](CMAEG_Protocol.md). CMA owns provider bindings, environments
and Agent conversations; the former in-CMA Bridge execution pipeline is retired.
