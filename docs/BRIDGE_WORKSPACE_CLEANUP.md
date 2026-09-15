# Separate allocation-owned Workspace cleanup

A retired `workspace_per_partition` allocation may remove its originally created
Workspace only after explicit child cleanup reaches physically proved Complete.
This is separate consent and a separate receipt. Child `remove_child` retains its
Workspace within that operation's scope. Neither reset, age, child completion nor
an old runtime receipt schedules Workspace removal.

## Public operations and consent

Both routes require current authenticated Bridge management and whole-parent
Sandbox management. New consent additionally requires the actual original
configuring actor and fresh original authority. Agent shares cannot expose parent
inventory. Private responses use `Cache-Control: private, no-store` and `nosniff`.

| Operation | Result |
| --- | --- |
| `GET /api/v1/bridges/{id}/allocations/{allocation}/workspace-cleanup` | Safe target, current consent inputs, eligibility, capability and retained progress. |
| `POST /api/v1/bridges/{id}/allocations/{allocation}/workspace-cleanup` | HTTP 202 and immutable acceptance with separately observed progress. |

POST requires an original `Idempotency-Key` of 1 through 200 visible ASCII
characters without spaces and exactly these fields:

```json
{
  "expected_bridge_revision": 4,
  "expected_allocation_revision": 7,
  "expected_workspace_revision": 3,
  "child_cleanup_receipt_id": "bclj_1111111111111111111111111111111111111111111111111111111111111111",
  "action": "remove_owned_workspace"
}
```

All revisions are positive integers bounded by signed 64-bit storage; TypeScript
also requires safe integer representation. The child receipt is the exact original
`bclj_` identity with 64 lowercase hexadecimal digits. The independent Workspace
receipt uses `bwcl_` with the same digest length. Caller-supplied owner, actor,
path, runtime, target override, cancellation, cascade and replacement-key fields
are rejected. The server supplies target ancestry from the retained allocation.

New acceptance requires a retired allocation that originally created this exact
Workspace, complete child removal proof, matching original creation and ownership,
no additional historical Agent or Worktree, and current demonstrated runtime
support. A reused Workspace, inconsistent inventory, unsupported runtime or
unavailable authority cannot become eligible through elapsed time or a read.

## Status and retained recovery

`workspace_revision`, `child_cleanup_receipt_id`, `receipt`, and `observed_at_ms`
are required nullable fields. Ineligible unrequested observations can lack review
inputs. `eligible: true` requires a current Workspace revision, exact child receipt,
`runtime_supported: true`, no holds, and no existing Workspace receipt. Accepted
jobs retain the original child receipt and current exact Workspace revision even
when that revision advances beyond the original consent. Current runtime support
may change without invalidating historical acceptance or Complete recovery.

Progress is `not_requested`, `requested`, `admission_closed`, `removal_started`,
`runtime_removed`, or `complete`. Holds are unique and ordered as:
`allocation_not_retired`, `workspace_not_owned`, `child_cleanup_incomplete`,
`workspace_busy`, `other_workspace_children`, `inventory_limit`,
`authority_unavailable`, `parent_unavailable`, `runtime_unsupported`,
`runtime_changed`, `evidence_unavailable`, `evidence_inconsistent`.

Only `complete` with permanent terminal evidence sets
`workspace_capacity_released: true`. Acceptance and runtime-removal observations
keep that reservation counted. Complete has no holds. `parent_retained` and
`history_retained` are always true. Sibling Workspaces, original child receipts,
allocation lineage and conversation history remain retained.

Persist the original five-field request, exact target and original key with the
separate consent. After cancellation, response loss, 409 or 503, explicitly replay
those identical bytes and key. Recovery rechecks current authority before returning
the original receipt and current progress, and occurs before fresh revision or
runtime eligibility checks. Changed input and replacement keys conflict. No SDK
method automatically retries or selects a replacement target.

## Public failures and capability

| Code | Meaning |
| --- | --- |
| `invalid_bridge_workspace_cleanup` | Invalid allocation identity, revisions or child receipt. |
| `bridge_workspace_cleanup_not_found` | Allocation cleanup is absent in permitted scope. |
| `bridge_workspace_cleanup_conflict` | Consent, current revisions, eligibility or retained evidence changed. |
| `bridge_workspace_cleanup_original_actor_required` | The caller cannot supply original configuring-actor authority. |
| `bridge_workspace_cleanup_unavailable` | Current authority, implementation or storage is unavailable. |
| `bridge_workspace_cleanup_evidence_inconsistent` | Retained proof is incomplete or inconsistent. |

Authentication, concealed Bridge access, invalid JSON and required-key failures
retain their established public names. Unavailable responses include Retry-After;
this never grants permission for an automatic mutation retry.

Public types and protocol tests do not establish deployed runtime capability.
New consent remains unavailable until the parent runtime and all backend writer,
capacity and restore paths demonstrate the required semantics. Local REST fixtures
exercise inert acceptance and response validation; physical removal and permanent
capacity/restore proof belong to their runtime and transactional tests.
