# New Agent admission scheduling

Default New Agent provisioning durably admits a Sandbox, prepares its default
Workspace, then durably admits the Agent. The Sandbox and Agent schedulers retain
their one-second discovery intervals and additionally accept local admission
wakeups, removing the need to wait for the next tick when capacity is available.

`MongoSandboxStore` shares one `Notify` per scheduler across its clones. An
admission emits its hint only after the transaction has committed, an ambiguous
commit has been positively reconciled, or an existing durable receipt has been
verified. Rejected, unresolved, and cancelled admissions emit no hint. The hint
contains neither a resource identity nor execution authority.

The Sandbox scheduler consumes a hint only with space in its existing limit of
four concurrent sweeps. The Agent scheduler consumes a hint only when discovery
is idle, its local queue is empty, and there is space within its existing limit
of 50 active Agents. It runs the ordinary bounded due query, then claims each
Agent with fresh time through the existing lease and CAS path. Admission wakeups
do not trigger connection reconciliation; that remains periodic.

Repeated hints coalesce to one pending permit. A full queue, occupied worker
slots, or an older discovery query cannot consume that pending hint. After
capacity becomes available, the scheduler can discover the newly committed
work without waiting for a tick. The existing periodic scans recover admissions
whose process-local hints were lost, including process failure after commit or
admission by another process. Shutdown retains the existing drain and abort
boundaries.

Paused-clock scheduler tests cover immediate discovery, missed-hint recovery,
admission during a stale query, bounded wake storms, and shutdown. Mongo replica
set tests gate actual commit dispatch to verify no pre-commit notification,
rejected-admission silence, and notifications after commit and durable replay.
This change does not assert an end-to-end latency number; runtime preparation,
credentials, provider startup, and lifecycle/UI polling still contribute time.
