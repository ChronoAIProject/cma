# Queued command revival

An admitted message for a paused Agent uses its retained execution authority to
revive the Agent's parent Sandbox. Binding commands receive authority only for
their proved parent; they do not gain general Sandbox management permission.
The scheduling transaction rechecks the exact pending command, binding and
resource policy. A concurrent claim, cancellation or policy change fences stale
revival and rejection attempts.

Revival prerequisites follow the Sandbox's recorded credential source, using
the same `check_sandbox` contract as Agent and Workspace operations. Managed
credentials require renewable owner authority and the effective service scope.
Binding and provided credentials require valid retained supplied authority;
they do not depend on an unrelated managed owner sign-in or connection lookup.
Runtime credential custody and binding revocation fences remain authoritative
when lifecycle work restores access.

Before this fix, command revival always checked managed owner credentials and
service connections. That could refuse a valid supplied-credential Sandbox.
`prepare_queued` then discarded the refusal and the worker discarded preparation
timeouts, allowing the command to expire with `queue_timeout` without dispatch.
The historical production refusal cannot be reconstructed from those discarded
results; this code defect is covered by a supplied-credential regression.

Preparation now returns typed outcomes. Missing credential setup or required
connections immediately fail the unclaimed command with
`credential_setup_required` or `connections_required`. Restore the prerequisite
and submit a new command with a new idempotency key. The same typed refusals
apply if a prerequisite fails at the final dispatch check after revival. Revoked authority and absent
Agents retain their existing terminal codes. Temporary dependency failures,
authority revalidation, scheduling contention and preparation timeouts retry
under the original queue deadline. Already scheduled lifecycle work remains
durable across retries and cancellation. Ordinary runtime-unavailable deadlines
are unchanged.

Warnings identify the command/Agent, preparation boundary and fixed error kind.
The boundary observer emits at most one warning per invocation and caps its IDs
at 128 characters; it never formats the command, credential, service list or raw
upstream error. Cancellation records the pending boundary, and the worker
separately records an IO timeout. Retry pacing and the original queue deadline
bound repeated attempts. Owner prerequisite diagnostics distinguish missing
identity, connection lookup failure and a mismatched connection set. Scheduling
diagnostics distinguish an accepted transition, a noneligible lifecycle state
and a scheduling error. These distinguish the previously invisible production
refusal without retaining user content.

After deployment, verify that a channel message to a paused Agent either
schedules revival and dispatches once, or promptly returns its typed refusal.
Investigate any `queue_timeout` using the preceding command-ID diagnostics;
confirm whether authority, prerequisites, lifecycle contention or runtime
availability prevented progress. Production reproduction requires an authorized
owner action. This change performs no production mutation.

The new terminal codes are additive API values and are included in both SDKs.
An older API binary or SDK with an exhaustive enum may not decode a newly stored
failure; rollback must retain a reader that recognizes these values.
