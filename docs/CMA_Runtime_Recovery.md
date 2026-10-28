# Recovery after an unpublished runtime open

A native conversation may open successfully before its tool authority receipt
is saved. The runtime remains alive while the control plane cannot publish the
conversation. Treating every already-initialized response as either a successful
open or a reason to close the process is unsafe: the former invents authority,
and the latter can discard an unflushed conversation tail.

## Preventing and recovering lost confirmation

The open runner retains its original channel and immutable binding while retrying
only confirmation. Each source allows three attempts bounded by five seconds per
attempt and fifteen seconds overall. Changed authority stops the operation.
Provider transactions retry only definitively transient outcomes; unknown commit
outcomes require exact readback and never authorize a blind transaction restart.

New Codex runtime images expose the read-only completion protocol documented in
[CMA Open Completion Proof](CMA_Open_Completion_Proof.md). An initialized bridge
with a missing confirmation can be reused only when two challenge-bound receipts
prove the exact native conversation, descriptors and process/session generations
before and after applying settings. This does not prove optional tool readiness.
Unsupported or mismatched evidence remains a refusal.

Opening failures retain their original cause through reader teardown. A local
acknowledgement-channel closure must not replace a provider refusal in telemetry.

## Recovering an older failed runtime

The existing owner/operator runtime-update command can admit a narrowly scoped
recovery of a managed native home. It requires a complete Sandbox, Agent and
Workspace snapshot, original retained storage authority, original conversations,
and evidence that this runtime generation never published or accepted CMA work.
Healthy active generations cannot enter this path.

The read-only provider inspection records a bounded, opaque plan with process
identities and history hashes. The Mongo admission transaction rechecks the full
snapshot, fences every child and original event head, and records the operation
before any process effect. Existing Agent records, queued messages, output
checkpoints and user files remain unchanged.

At the checkpoint-verification phase the worker executes the admitted provider
plan. The Codex adapter journals the operation on the original PV and fsyncs it
before signaling the exact, source-verified Node bridge. Closing its native stdin
allows the pinned native process to run its normal EOF shutdown. The adapter
never force-kills the native process or its process group after a drain timeout.

The provider verifies original history prefixes, final persisted turn evidence,
exact process disappearance and unchanged runtime/home identity. A retry uses
the same journal and refuses replacement PIDs, a different boot or changed source
authority. Native proof failures retain the old runtime and retry; they never
authorize suspension. Historical abandoned turn markers remain history and are
not rewritten into synthetic completion records.

The worker persists a `RuntimeRecoveryQuiesced` receipt and completion event in
one lease-fenced transaction before the existing home-verification path may
suspend the runtime. Replacement uses the original native Sandbox and home,
advances its generation, applies the admitted image and reopens the original
conversation IDs. A lost control-plane reply replays the durable provider journal.
Loss of a replacement runtime retains the recovery incident and can continue
through the normal retained-home lifecycle.

## Automatic recovery and operational limits

Background recovery applies only after ordinary retries have failed or parked.
Its target must already equal the released managed image; it cannot silently
retarget an older admitted update. One automatic replacement attempt is allowed
per unresolved incident. Explicit owner/operator retry remains available, and a
successful publication clears the incident for a future distinct failure.
Candidate scheduling rotates so an unprovable earlier candidate cannot starve
other failed sandboxes.

A missing original volume, incomplete identity, unrecognized provider binary or
unprovable process state remains a refusal. No empty replacement volume or new
conversation substitutes for unavailable original data. Older API binaries that
cannot read the additive lifecycle metadata are not compatible rollback targets
after recovery has been admitted.

## Validation

Tests cover transient and unknown Mongo commits, stale child authority, duplicate
admission, exact receipt events, lost replies, PID/boot changes, history-prefix
retention, complete recovery and subsequent pause/update/retirement. Native
fixtures run the pinned Codex process with active work and a dirty workspace,
verify EOF shutdown, reopen the same conversation and send a subsequent prompt.
The Python process/journal tests run in Linux CI; GitHub CI builds and scans the
deployment images. Production recovery still requires checking the original
home, original conversation and resulting runtime independently for each sandbox.
