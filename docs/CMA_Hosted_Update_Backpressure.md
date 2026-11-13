# Hosted update backpressure

Part of #1472, implementation slice #1684. The independent strict flag
`CHRONO_RUNTIME_UPDATE_RELAY_BOUNDED_ENABLED` defaults to `false`.

When enabled, each hosted attachment uses a 32-update relay queue instead of an
unbounded channel. One additional update can be awaiting downstream delivery,
and the downstream channel retains its own existing capacity. Producers wait
for capacity; dequeue, overflow, cancellation and disconnect never synthesize a
durable ACK. The single-frame capture used to translate a runtime frame remains
separate and must produce at most one domain effect before publication.

The runtime must advertise retained output acknowledgements in its exact Ready
declaration. Otherwise opening refuses before initialization or prompt input.
The native retained journal keeps output until a committed consumer ACK and
backpressures stdout when full. Existing runtime, Agent, native conversation,
prompt and output positions are preserved. This flag does not upgrade a runtime
or enable a lossy fallback.

A task owned by the WebSocket reader forwards durable ACKs while frame delivery
waits for relay capacity. The task is aborted when its reader ends or is cancelled;
it also progresses independently of a dispatch waiting on socket writes. Its ACK progress handle is captured before processing
frames, so ACKs do not acquire the output mutex held by blocked delivery. Stream
identity and positive cursor are checked; duplicate/backward progress does not
advance the progress watch. Dropping the durable consumer closes the relay and
ends the attachment, leaving unacknowledged native output available for recovery.

Opening callers must continue draining output while the handshake progresses;
the existing CMA startup and reopen workers do so. A retained backlog larger
than 256 frames is valid. The flag does not truncate it or declare successful
opening before required handshake work completes. Ordinary durable folding,
retry and terminal identity checks remain in their original paths.

## Activation and rollback

Deploy off first. Before enablement, the runtime/release owners must identify
the compatible retained-output capability for cold, warm and resumed runtimes,
the exact API/runtime candidate, controlled fixtures and the coordinated harness
slot. Do not turn the flag on to discover compatibility by interrupting active
work. Existing attachments keep their selected mode until normal reattachment;
never terminate an owner-active Sandbox for mode convergence.

Rollback disables the policy for subsequent attachments. It does not alter
durable schemas, emit an ACK, clear a native journal or erase existing history.
Ordinary rollout continuity applies. No activation or production traffic is
authorized by this source slice. Sandbox/model lifecycle campaigns retain their
separate count/estimate and relay requirements.

## Evidence boundary

This is a per-attachment queue and backpressure bound, not the complete 128 MiB
global decoded-memory proof. Decoding working memory, raw startup retention,
deeply cloned updates, core startup buffers, public replay and the number of
attachments need shared accounting and qualification. The runtime's 8 MiB wire
journal does not establish an equivalent decoded-heap bound. Public observer
count admission is a separate slice under #1678. Runtime mapping and physical
reserve remain under #1487 and their external owners.

Local contracts cover ordered slow-consumer recovery, a full relay with more
ACKs than the ACK channel can retain, no premature ACK on enqueue/dequeue/drop,
startup with a retained backlog and correlated terminal, missing capability,
consumer loss and malformed retained identity. Flag-off behavior remains
covered by the existing ACP attachment/recovery suite. Source tests are not
production acceptance receipts.
