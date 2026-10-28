# Sandbox conversation and workspace recovery failures

Runtime readiness, conversation attachment, and durable tool confirmation are
separate boundaries. A Ready Pod with its retained home can still have an
unfinished conversation reopen. A tool confirmation record is scoped to the
exact original Agent, request, runtime incarnation and conversation; an older
runtime's confirmation does not authorize the replacement.

## Confirmation after successful native opening

The hosted adapter validates the returned launch and conversation receipt before
confirming each tool source. If confirmation is unavailable or its reply times
out, it retains the completed channel and retries that same confirmation. It
does not initialize the native bridge again, resolve a replacement attachment,
send a prompt, or publish the Agent as ready while confirmation is outstanding.

There are at most three attempts, each with a five-second deadline, within one
fifteen-second total budget per tool source. Brief backoff separates retries.
An immediate success has no extra wait. Refused or unsupported authority stops
immediately. Parent lifecycle/startup deadlines still bound the entire open.
Cancellation drops the current waiter and admits no detached retry task.

The source rechecks current authority on every attempt. Cancellation or timeout
does not establish that a database transaction aborted: a subsequent attempt
must recognize an exactly matching committed receipt without broadening it to a
different conversation, boot, request or authority. All required sources must
confirm before readiness can be published.

This prevents transient confirmation failures from unnecessarily abandoning a
successfully opened conversation. It does not manufacture evidence for an
already-initialized native bridge, recover an incomplete native setup, or
recover successful native setup evidence lost with the control-plane process.
Those cases retain their existing authority checks. Increasing retry counts or
copying an older receipt into a replacement runtime cannot repair them safely.

## Workspace preparation interrupted before completion

A started preparation journal can outlive a Git clone that never completed.
The private staging directory may then contain `.git` and owner data without
the CMA workspace identity marker, which is written only after Git succeeds.
During recovery of that exact journal-owned staging path, a proven missing
marker settles the existing indeterminate preparation outcome. It preserves all
staged files and leaves the workspace failed rather than exiting the whole
supervisor on every boot.

A completed staged workspace still requires its exact identity and resolvable
commit before publication. Wrong identities remain refused. Other I/O errors
remain retryable and block startup; this treatment does not make an unhealthy
filesystem ready. A missing marker in an already-published workspace retains its
separate refusal behavior. No marker is synthesized and no staging data is
deleted during this recovery.

## Verification boundary

Regression fixtures cover ambiguous confirmation readback, changed authority,
bounded timeouts/cancellation, one native open with unchanged tool binding,
interrupted and completed staged preparation, wrong identities, and genuine
I/O errors. Local fixtures do not establish that an existing production sandbox
has recovered; production acceptance must additionally confirm its original
home, conversation and admitted work after the corrected lifecycle completes.
