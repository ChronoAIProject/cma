# Explicit retained child cleanup

A reset retires an allocation and preserves its resources. An explicit child
cleanup command records fresh management consent to remove that exact retired
child from a reused parent. Historical cleanup observations, reset, Bridge
deletion, resource age and discovery alone do not create a cleanup job.

## Public operations

All routes require an authenticated Human or nondelegated API caller, current
Bridge management, and current management of the whole parent Sandbox. An Agent
share cannot expose parent allocation inventory or admit cleanup. Responses use
`Cache-Control: private, no-store`.

| Operation | Result |
|---|---|
| `GET /api/v1/bridges/{id}/allocations?limit=50` | Ordered allocation IDs, generation, revision, exact Sandbox/Workspace/Worktree/Agent target, placement and state. |
| `GET /api/v1/bridges/{id}/allocations/{allocation}/child-cleanup` | Immutable acceptance if requested, latest proved progress and time of observed holds. |
| `POST /api/v1/bridges/{id}/allocations/{allocation}/child-cleanup` | `202` and retained acceptance/status after the explicit job, audit and request are committed together. |

Allocation pages accept limits from 1 through 100. `next_cursor` is an opaque,
versioned continuation bound to the Bridge, or null when no more rows were
observed. Keep and pass it unchanged. It is not an authorization token.
Each page examines a bounded set of allocation candidates and omits parents
that do not exist or are inaccessible. A short or empty page may still have a
continuation; continue until `next_cursor` is null. Storage or authority outages
are errors and are not presented as empty inventory.

The POST requires `Idempotency-Key` and this JSON body:

```json
{
  "expected_bridge_revision": 3,
  "expected_allocation_revision": 4,
  "action": "remove_child"
}
```

New acceptance requires a retired `reuse_parent` allocation and current authority
of the actual actor who configured its approved profiles. Both expected revisions
must match. A different administrator or an API credential presenting a different
actor gets `bridge_child_cleanup_original_actor_required`; this slice has no
separately attributed delegation for that action. The accepted receipt retains
the actual actor and time. Parent ownership, original pinned profiles, allocation,
creation provenance and retained request must agree before any effect.

A successful response is acceptance, not proof of physical removal. Preserve the
original key and complete request after network loss, interruption, 409 or 503.
Retrying the identical request recovers its original acceptance under current
read authority before new-write revision checks. Changed input conflicts. A
second key cannot replace the job for the same allocation. A previously recorded
Stage A request can be explicitly adopted, preserving its exact original actor,
request, audit, holds, seal and timestamp; the new job records separate consent.
Missing accepted job, audit or allocation marker after removal started is an
inconsistency, never permission to reconstruct a new job.

## Progress and holds

Progress advances through `requested`, `admission_closed`, `removal_started`,
`process_closed`, `home_removed`, `worktree_removed` and `complete`. Before an
explicit job exists the response is `not_requested` with a null receipt and
observation time. The receipt is immutable; progress is a separate observation
and can temporarily lag already committed effects after a lost response.

The worker first closes new child input, then waits for accepted commands and
replies to drain. It does not cancel, abandon or resend work to force quiescence.
Busy Agent/Workspace operations, pending commands/replies, uncertain command or
reply outcomes, bounded inventory or unavailable authority/runtime evidence hold
progress. In particular an Unknown send never becomes an automatic retry.
`observed_at_ms` describes the retained observation, not a promise that an earlier
quiescent state remains true. Fresh authority and quiescence are required again
before every new effect. Other Workspace children remain in place and do not
prevent removing the selected child when its own proof is valid.

Only explicit jobs enter bounded discovery. Four scheduler tasks share durable
60-second claims; successful nonterminal progress can be revisited after 250 ms,
and observed holds after five seconds. Claim expiry bounds the effect authority.
A stale claimant cannot publish over a newer claim. Unknown runtime outcomes
retain the original operation identity and lifecycle exclusion. A subsequent
worker proves retained B/C records before taking another step; an already proved
Complete result can recover its missing job publication without executing again
or requiring the parent to be Active. Normal GET also remains available while
an authorized parent is Paused and does not require execution credentials.

`complete` requires permanent exact child removal evidence and releases its Agent
reservation. `workspace_retained` and `history_retained` remain true. This API does
not remove a Workspace, release its reservation, remove root storage/history,
or delete a parent Sandbox. Missing or inconsistent proof holds the job.

New acceptance uses the management feature switch. Accepted cleanup drains
independently of switches for new allocation/execution, with fresh authority on
each effect. Old binaries do not discover these new jobs. Shared-child admission,
restore tombstones and lifecycle exclusions still require coordinated compatible
writers before production enablement; adding a job collection is not evidence of
mixed-version or rollback safety.

## Typed failures

| Code | Meaning |
|---|---|
| `invalid_bridge_child_cleanup` | Invalid allocation/cursor, limit, revisions or action. |
| `bridge_child_cleanup_not_found` | No permitted Bridge/allocation/whole-parent scope. |
| `bridge_child_cleanup_conflict` | Changed revisions, conflicting key/input, or changed evidence. Preserve uncertain original input. |
| `bridge_child_cleanup_original_actor_required` | Current caller cannot supply original configuring-actor authority. |
| `bridge_child_cleanup_unavailable` | Current authority or storage is unavailable; acceptance may need exact recovery. |
| `bridge_child_cleanup_evidence_inconsistent` | Original durable evidence is missing or inconsistent; no new effect can advance. |

Existing authentication, Bridge management, body-format and idempotency-header
problems retain their existing public names. A transport failure does not prove
that a command was rejected or that a runtime effect did not happen.


## Later separate Workspace removal

`workspace_retained: true` describes this child operation's permanent effect
boundary. A later [separately consented owned Workspace cleanup](BRIDGE_WORKSPACE_CLEANUP.md)
may remove the originally created Workspace after child completion. Its own
request, receipt and permanent completion proof authorize that additional effect;
this child receipt and its original bytes remain retained and unchanged.
