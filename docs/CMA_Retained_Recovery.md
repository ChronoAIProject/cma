# Retained runtime recovery assessment

Related to #1575 and #1448. This first slice provides an owner-authorized,
read-only assessment. It does not enable recovery admission. The existing
managed runtime update and failed-unpublished recovery rules remain unchanged.

## Supported request

An authenticated owner with `UseOwnSandboxes` may call:

```http
GET /api/v1/sandboxes/{id}/runtime-recovery
```

The response contains exactly a fixed `disposition` and
`admission_enabled: false`. It contains no runtime, home, boot or conversation
identities, provider error text or history content. Another owner's Sandbox
returns 404; insufficient capability returns 403. An unavailable dependency or
occupied assessment permit returns 503. A bounded assessment timeout returns
`evidence_unavailable` with admission disabled.

| Disposition | Meaning |
|---|---|
| `active_lifecycle` | The durable lifecycle is already Active; this does not certify native health. |
| `recorded_boot_present` | The recorded boot is still present under exact current native/home authority and the requested histories passed inspection. |
| `exact_boot_replacement` | A different boot was observed under the same exact native/home/image/generation authority before and after history inspection. This is an observation, never permission to interrupt it. |
| `exact_suspended_runtime` | The complete child set is empty and the exact retained native graph is independently verified suspended. This does not mean the retained home or provider object was deleted. |
| `evidence_unavailable` | A dependency, runtime status or bounded inspection did not provide complete evidence. |
| `authority_changed` | The parent, complete children, native graph, generation or home no longer matches. |
| `terminal_failure` | The parent is outside the inspected Failed/runtime-loss or failed-restore shape. |
| `child_activity` | Child work, dispatch, cleanup, stop intent or a live native Agent process keeps inspection held. |
| `unsupported_image_authority` | The exact immutable image, contract or retained native home cannot be established. |
| `provider_evidence_refused` | The provider refused its bounded inspection; no raw error is exposed and no narrower cause is invented. |
| `incomplete_history` | The provider could not prove the complete original conversation history set. |

The service admits at most one assessment concurrently per API process, without
a waiter queue. Its 20-second bound includes authorization, complete durable
snapshots, provider reads and final snapshot revalidation. Cancellation releases
the permit. No proof is cached or retained between requests. Rollouts may have
two API processes, each with its own bounded permit; this is not a distributed
recovery-admission budget.

The store validates complete Agent logs, including terminal children. Reading
an already published generation here does not relax the original unpublished
admission predicate. The provider checks exact retained authority before and
after history inspection, including the existing stale-authority revalidation.
Missing observations and transport errors never prove provider absence.

## Owner action and remaining implementation

There is **no recovery action in this slice**. GET does not start work; POST on
this route is unsupported. An owner whose explicit-image Sandbox is Failed
cannot use this endpoint to make the original Agent sendable. The existing
`runtime-update` endpoint continues to exclude custom images. Repeated requests,
another API restart, or a different image must not be suggested as recovery.

The separately reviewed admission slice still needs:

1. Explicit owner/operator intent bound to the exact incident and image, with
   idempotent retry receipts and no caller-supplied proof or arbitrary target.
2. A positive, causal loss proof that preserves both expected and observed boot
   identities. A healthy replacement, unknown transport result, terminal cause
   or missing history remains held even after consent.
3. Complete parent, Agent, Workspace, log-head and dispatch/work authority in one
   transaction. Reuse runtime-loss settlement for already-lost turns; preserve
   attempted and ambiguous input receipts without replay. Pending stop/delete,
   removal and capacity-terminal work remain outside admission.
4. Durable quiescence followed by a final native suspension fence, generation
   reservation, credential delivery and original-conversation publication, with
   cancellation and unknown-commit reconciliation at every boundary.
5. An explicit reader/rollback compatibility design. Existing recovery records
   bind to the recorded boot. Adding a replacement boot to an opaque plan alone
   does not establish that an older worker enforces its new suspension rules.
   No new durable authorization may ship until rollback behavior is demonstrated.

This slice writes no new durable fields or events and performs no native
quiesce, suspend, resume, allocation or deletion. Removing its API image leaves
all records readable and all previously admitted operations unchanged.

## Verification

Targeted tests cover exact default/custom images, unchanged managed-update
selection, owner authorization before child/native reads, complete log checks,
stale snapshots, deadline/cancellation permit release, provider history refusal,
replacement work, before/after boot changes and absence of runtime effects.
The public contract includes success and authorization/dependency failures.
Production inspection, when separately requested, uses this supported owner
read; it does not execute a repair or generate a test conversation.
