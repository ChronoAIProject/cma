# Global Sandbox admission

Part of #1472; implementation issue #1660.

`CHRONO_SANDBOX_GLOBAL_LIMIT` is zero by default. Values 1 through 4096 opt into
one aggregate nonretired Sandbox ceiling across all owners and API processes
sharing this database. Invalid values prevent startup. This is an allocation
ceiling, not a claim that a particular cluster can sustain that many Sandboxes.

The existing admission transaction takes a global Mongo write fence and counts
at most the configured limit through `sandboxes_global_live`, with a three-second
query deadline. Competing owners must retry from a fresh transaction snapshot;
the same transaction persists the Sandbox, event and any warm/prepared claim.
A refusal or failed transaction spends no allocation. Driver failures fail
closed. There is no process-local quota counter and no startup counter backfill.

Provisioning, active, paused, resuming and retiring rows all occupy capacity until
`retired_at` is recorded. Keeping paused reservations makes simultaneous resumes
safe within this allocation bound. Existing legacy rows count immediately.
Exact-origin replay remains available when full and keeps the original resource.
Fresh direct, Trigger and binding admissions share this boundary. The low-level
unadmitted insert port refuses use while the limit is enabled.

Fresh manual creation gets HTTP 503 and `sandbox_capacity_unavailable`; no other
owner's count or configured ceiling is returned. Accepted Launch work retains a
retryable capacity failure and its original operation identity. Per-owner quotas
retain their distinct refusal and still apply below the global ceiling. This
does not change visitor daily limits, revocation or stop/delete admission.

## Activation and rollback

1. Deploy with zero, verify the additive index and all serving admission writers,
   and qualify a compatible rollback image. Mixed old/new writers cannot enforce
   a global ceiling. Do not enable during a mixed deployment.
2. GKE/controller and release owners approve a conservative limit using the
   largest admitted CPU/memory profile, existing nonretired inventory and the
   planned simultaneous-resume demand. Reserve unclaimed warm/prepared pools,
   unrelated workloads and failure/N+1 headroom outside this number. The pool
   controller provisions resources independently and is not bounded by this row
   count. No test or default in this PR selects production capacity.
3. Set the same limit in every admission process in a separately authorized
   release. Verify convergence before declaring enforcement. If inventory is
   already at or above the ceiling, new allocations refuse; existing work,
   exact retries, resume and retirement remain available.
4. Verify bounded different-owner admission/refusal, no count overshoot, replay,
   normal control availability and capacity recovery after completed retirement.
   Couple the receipt to source/digest/config and aggregate cluster headroom.
5. Roll back to the compatible image retaining the ceiling. Setting zero removes
   this protection and requires an explicit capacity-owner decision; it is not
   an automatic response to saturation. No durable receipt or resource is erased.

The limit does not reclaim retained homes or establish a CPU, byte, stream or
document quota. Runtime/network isolation and actual load remain separate
acceptance rows. The real-Mongo tests establish cross-process transaction
behavior, not production capacity or production execution permission.
