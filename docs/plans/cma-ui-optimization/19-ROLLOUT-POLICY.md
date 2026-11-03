# Ticket 19 bounded task experience policy component

This document covers this component only. It does not accept ticket19, certify
the composed native journey, approve release, or satisfy ticket13's literal
human-quality signoff. The primary19 implementer owns runtime/browser proofs,
metrics, screenshots, acceptance/release material and tracker changes.

## Ownership and public contract

Core owns `TaskExperiencePolicy`. CMA composes one policy from the strict JSON
`CHRONO_TASK_EXPERIENCE_POLICY` into command admission, inline lifecycle and Agent
browser services. Layer1 exposes authenticated `GET /api/v2/ui/task-experience`.
The successful response is private/no-store: `object: "cma.ui.task_experience"`,
`schema_version: 1`, `account_id` containing only the caller's id, and five boolean
`capabilities`. No mode or cohort ids are returned. Typed SDK methods are
TypeScript `client.taskExperience.read()` and Rust `client.read_task_experience()`.
Both execute the shared public corpus. Web uses public SDKs, with no client
allowlist. Account identity fences delayed and focus-refresh results.

| Capability | New admission or presentation | Retained behavior |
| --- | --- | --- |
| `task_home_admission` | Lifecycle requests with `initial_message` | Accepted creation replay, legacy creation without an initial task, manual conversation |
| `suggested_followup_admission` | Answer-scoped messages through common admission, including AG-UI and Responses | Exact accepted key/body recovery, unscoped manual input, edited draft and answer context |
| `desktop_enable` | Desktop creation and explicit browser enable | Capability discovery, observation, explicit disable/release, accepted creation recovery |
| `browser_takeover` | New acquire/input/renew | Current-authorized status, original receipt lookup/replay, explicit return |
| `replay_presentation` | Operation replay controls | Authorized public history/output, saved replay positions, ordinary operation output |

Eligibility is separate from environment capability. Existing verified-image,
capacity, adapter, permission, home, generation/boot and context checks remain.
No policy change rotates a transport or key, clears a journal, creates a replacement
Agent/home/runtime, restarts a Sandbox, or adopts another account/incarnation.
Unknown policy blocks new entry, while needed retained reads/recovery remain.

## Strict configuration and rollback

Code and infra default to `{"mode":"disabled","accounts":[],"capabilities":[]}`.
Supplied JSON requires all three fields. Modes are `disabled`, `accounts`, `all`.
Accounts are exact case-sensitive ids: at most1000 unique values, each nonempty,
at most256 UTF-8 bytes, with no whitespace/control characters. Empty cohort
admits nobody. Only the five capability names above are valid. Unknown/duplicate
fields, duplicate ids/capabilities, nonempty accounts outside accounts mode, or
capabilities in disabled mode fail startup without echoing config content.
All mode still requires explicit capabilities. Policy changes require a compatible
API configuration rollout; they never restart the running runtime.

Fresh authorization precedes read/replay/mutation. Common admission recovers the
exact stored request before gating new answer-scoped work. Lifecycle recovers its
original durable operation before policy or current desktop availability. With
takeover closed, Core first queries the original private-key-bound receipt. Only
a receipt revision newer than the original expected revision reaches the runtime's
exact-request replay check. Eviction cannot create new work at that stale revision.
Unknown/unaccepted acquire/input/renew is refused; authorized return remains.
Web keeps control status, return and original recovery mounted independently from
new admission. It never uses rollout closure as `BrowserControl.active=false`.

The native15-second lease is unchanged. Revocation/unavailability stops renewal;
Agent resumption may wait the remaining lease after accepted serialized input
settles. Closure is not instantaneous release. Retained recovery requires the
ticket16 creation reader, ticket15-aware runtime-status reader and earlier durable
operation readers. Stock pre15/16 binaries are unsafe rollback candidates.
Primary19 must execute its exact compatible API/Web candidate against retained
Mongo/home/native runtime, then re-upgrade without replacing that environment.

Progression can begin with one explicitly enrolled account and expand the exact
list only after primary19 acceptance and release review. This component adds no
analytics platform, assignment service or production observation claim. Release
windows, thresholds and observed metrics remain primary19's responsibility.

## Evidence boundaries

The parent's composed journey found early file-tree409 and model-catalog503
responses while the Sandbox was still preparing. This component fixes those
production readiness seams. Initial file IO waits for current environment
availability; an explicit environment404 or absent client capability retains
legacy access. Readiness changes trigger a fresh check. Browser auto-selection
does not start a hidden file read. Temporary expiry/failure quarantines an
already loaded file draft in its exact account/Agent/connection component,
withdraws content/actions and aborts IO, then restores the draft only after
fresh authority. Permanent denial or identity change withdraws that component.

Only model-catalog reads wait for delivered credential evidence: current active
Sandbox, recorded delivery completion, or prior Agent initialization. Stored
settings and the conversation remain mounted. Working, approval, paused/stopped
and a retained ready timestamp remain usable evidence. Shared callers use their
public status/ready timestamp without probing private ancestry. Corrective
owner model discovery does not wait for initial Agent success. These changes
have focused UI reds/greens; the primary worker owns the composed rerun.

External evidence: `/Users/liuyitian/code/cma-ui-worktrees/ticket19-rollout-proof/`.
Actual public handler reds were policy404 versus200, answer-scoped admission200
versus403, and browser-control admission200 versus403; each then passed. UI manual
fallback, closure during control and fresh identity focus also have red/green
evidence. A first short `--exact` selector ran zero tests; its separate log is
retained and is not proof. Failures and retries remain in the evidence directory.

Public proofs compose real handlers/Core with in-memory stores and controlled
runtime/credential boundary fixtures. Typed SDKs exercise real HTTP dispatch.
These are policy/admission/recovery and wire proofs, not native execution, Mongo
restart, production identity, rollout outcomes or full composed acceptance. No
VM, Mongo or native jobs run here. Final exact counts, commands, changed-source
and evidence hashes and limitations are sealed in external `HANDOFF.md`.

Six matching fixtures replace the ticket17 synthetic lease UUID with one visibly
synthetic valid low-entropy UUID. No scanner exception/rule change/suppression is
introduced. Current-source repair does not pass the historical secret gate:
the parent separately owns unpublished-history repair while preserving original
user history and old proof source identities. This worker does not rewrite history.
