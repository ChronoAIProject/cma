# Ticket 18: operation record replay

Branch: `codex/cma-ui-18`, worktree `ticket-18`, starting at `d19c225bd`.
The user approved the original ticket's U/A/R seams and local tracker. The first
vertical slice holds operation 2 while operation 4 arrives, then returns to 4
without admitting another execution.

Own the public durable operation query, typed SDK parity, replay view and
turn-specific navigation. Reuse `AgentPanel`, `onSelect`,
`AgentComputerObservation`, recorded bounded output and public event storage.
Shared ticket 15 seams are `AgentSidePanel`, environment capability declarations,
and style appendices. Replay belongs to Operation output; Browser retains its
own private context and lifecycle. Ticket 18 implements no browser or control.
No Layer 4 changes are planned; if needed, build a separate immutable candidate
and coordinate the shared Linux build budget first.

Validation uses isolated Rust artifacts and at most two test workers. The native
proof uses the pinned Codex/ACP and public read path, with an execution counter.
Logs are operation evidence, never recorded browser frames. Final evidence and
file manifest will be recorded here before completion.
