# Agent and machine warm pools

Agent-only allocations use `CHRONO_SANDBOX_IMAGE`; Machine Included allocations
use `CHRONO_MACHINE_SANDBOX_IMAGE`. Each image has an independent immutable native
pool plan and target count, configured by `CHRONO_NATIVE_WARM_POOL_TARGET` and
`CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET`. The existing public warm-pool list shows
both image identities and permits revision-fenced resizing of either current
plan. Its create operation continues to configure agent-only resource sizes.

Both pools preserve the same ownership rules: no owner credential or user prompt
is delivered to unclaimed members, and claimed homes retain their original
Claim, Sandbox, PVC and image authority. A new image creates a new plan; it never
retargets a claimed Sandbox. Startup waits for the observed durable target of
each configured image. A durable operator resize overrides the initial count.

Machine pool documents use schema 2 and `machine: true`. Historical agent pool
documents remain schema 1 and default to `machine: false`. The reader rejects
any other schema/channel pairing. Release timestamps and retirement authority
are independent for the two classes, so a machine promotion cannot retire the
current agent-only pool. Original release timestamps and tombstones still
prevent an older image from becoming current through a resource-policy change.
Older binaries refuse schema 2 at startup and cannot claim those records while
already running. Deploy this compatible reader before enabling machine
capacity; rollback must retain this reader and both released image pins.

Ready native Pods still require runtime and Agent initialization after assignment.
For the default Create Agent journey, `CHRONO_NATIVE_PREPARED_TARGET` and
`CHRONO_NATIVE_MACHINE_PREPARED_TARGET` independently prepare up to two complete
Sandbox/Workspace/idle Agent hierarchies. Machines start their desktop before
ownership transfer. Each class has its own durable target and transactional
admission fence; retiring agent-only spares does not retire machine spares.
Both settings default to zero. A matching native pool and a full native home
must be available before enabling either preparation target.

Machine preparations permit only the exact `CMA_MACHINE_INCLUDED=1` environment
marker and a pinned machine image. They receive no owner credentials or user
prompt. Ownership transfer requires matching immutable configuration, current
released image, lifecycle image and machine selection, and all existing Agent
readiness proofs. A custom configuration or depleted pool uses normal creation.
The preparation worker replenishes capacity after claims; the bounded spare
count is not a guarantee for an unlimited concurrent burst. Rollback after
enabling machine preparation must retain the machine preparation reader;
older readers reject its environment and cannot service those reservations.

Within each API process, the preparation worker runs at most one Ready health
probe per image class at a time. With two healthy reservations this keeps the
other available for admission while the probe holds its lease. Agent-only and
machine checks may run together; preparation and retirement still share the
four bounded worker slots. Durable reservation leases retain effect authority
across processes. Probe frequency, admission proof freshness and the owner's
fresh runtime checks remain unchanged.

The Mongo contract CI partition includes the prepared hierarchy suite, covering
separate caps and retirement, preserved agent-only spares during machine claims,
prestarted runtime retention and refusal of obsolete prepared machine images.

The delivery acceptance target is at most five seconds from Create Agent to a
usable Agent for each image, including a usable desktop for Machine Included.
A ready native member alone does not establish that latency: ownership transfer,
runtime assignment, desktop startup, credentials, workspace preparation, Agent
open and the public UI must all be measured. Production timing and Omarchy
acceptance remain pending. Do not advertise the five-second target as achieved
from pool readiness or fixture tests.

The API manifest reserves 500 millicores and permits bursts to two cores while
both preparation classes reconcile. A production sample with two agent-only
reservations used about 0.41 cores on average but hit the one-core quota in
663 of 2,709 scheduling periods, accumulating 12.8 seconds of throttling.
This capacity change addresses measured contention; it does not establish the
five-second creation budget. Verify that budget from the actual Create Agent
click through usable conversation and, for machines, a connected desktop.

Prepared ownership replay may combine Worktree and Agent-home preparation into
one authenticated runtime request. Both cleanup intents still commit before the
request; both original operation IDs, digests, resource fences and journals remain
authoritative. The runtime executes the pair in order under its mutation gate,
and the API publishes both validated receipts atomically. A partial or lost reply
retains both debts for recovery and never triggers an automatic separate-request
fallback. Prepared Codex close/load, fresh runtime identity proof and final
connection publication remain required.

The API selects this path only from the capability returned by its fresh bound
runtime inspection. It opts in with the HTTP Accept media type
`application/vnd.chrono.cma.agent-resources-status+json`; request JSON is unchanged.
Only that status response includes `agent_resource_preparation`. Old APIs receive
the prior status shape, including embedded Workspace protocol status, and old
runtimes leave the new capability absent. An absent capability keeps the existing
two-request path. Both image contract probes execute receipt replay and capability
negotiation against the shipped supervisor. The Omarchy build now copies the
supervisor from the current source build while retaining its pinned tool donor.
These checks establish behavior, not the end-to-end five-second acceptance target.
