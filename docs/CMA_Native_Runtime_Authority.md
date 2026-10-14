# Native runtime context proof

CMA collects adoption and process status through `SandboxAdapter::inspect_runtime`.
The default implementation preserves the separate adoption/status path. Hosted
native VCT runtimes can consolidate the resource predicates for one call when
an exact bootstrap acknowledgement already exists for the current Pod and
runtime-container incarnation.

The private proof has no clone implementation and never becomes cached dispatch
authority. It binds the rendered runtime request digest, home identity, Sandbox
UID and complete Pod binding. Every call reads the original Claim, Sandbox, PVC
and Pod inventory again. These four reads run concurrently because their names
come from the persisted attachment. Validation still processes Claim, member,
volume and Pod results in order.

The proof checks the original Claim UID/spec, current assignment and complete
home-origin annotation; Sandbox ownership, UID, immutable pool spec, readiness
and all rendered runtime metadata; PVC UID/storage/ownership/mount authority;
and exactly one owned Ready Pod with its container incarnation. Runtime
rendering and predicates use the same validators as the existing adoption path.
Explicit Template/Pool network policy remains a fresh admission check.

A missing acknowledgement or changed Pod/container selects the full bootstrap
path. Changed authority is rejected before RuntimeStatus. Status must still run
and validate the exact generation/home. The transport retains its independent
fresh Pod dispatch/open fences. A fresh home verification follows the status
reply. CMA still checks `runtime_process_matches` against durable parent state,
and subsequent effects still obtain their own dispatch authority. No evidence
is shared across durable steps or across a provider mutation.

## Verified request budget

A fixture with an already acknowledged native runtime measures the full Hosted
context before and after consolidation. Both variants use the same transport
fixture that obtains fresh Sandbox and Pod observations immediately before the
status request:

| Context | Kubernetes reads | Status RPCs | Writes/bootstrap |
| --- | ---: | ---: | ---: |
| Separate adoption and status | 21 | 1 | 0 |
| Consolidated inspection | 9 | 1 | 0 |

The nine reads are four pre-status graph reads, two fresh dispatch reads and
three post-status home reads. The next call repeats all nine. This removes
12 reads per context; nine such post-provisioning contexts remove 108 reads.
Initial transport opening still pays its existing before/after-open fences.
Connection reuse is a separate change and does not authorize skipping resource
observations.

Tests require all four graph reads to start before any response is released.
They also reject changes to Claim UID/assignment/origin, Sandbox UID/owner/spec/
origin, PVC UID/storage, rendered request and explicit policy; refuse the fast
path after Pod/container replacement or loss of acknowledgement; and reject a
PVC replacement between status and the final home check.

This is a measured local request reduction, not a production latency guarantee.
The wider design target of 85 post-provisioning reads also requires sharing
transport endpoint predicates and combining dispatch home/Pod reads. Those
additional changes are outside this consolidation.

## Provisioning from persisted home identity

Initial provisioning carries the existing home attachment through the Hosted
adapter and provider router. Native VCT admission uses that exact origin to
address fresh Claim, Sandbox and PVC reads. It no longer first reads the PVC
and then its owning Sandbox solely to rediscover the same origin. This removes
two sequential discovery GETs; it does not cache live resource authorization.

Both native home layouts retain current graph and explicit policy validation,
UID/resourceVersion-fenced publication, bootstrap proofs before and after
assignment, and the adapter's home verification before and after provisioning.
Malformed or foreign persisted authority is rejected locally. Legacy home and
provider paths retain their existing provisioning behavior. Fixtures verify
the exact two-read reduction, concurrent graph reads, forged identities and
mounts, live authority changes, publication conflict recovery, and router
forwarding. End-to-end latency still requires a production measurement.

Native admission validates the persisted origin and complete runtime request
before starting fresh Template/Pool policy reads alongside the Claim/Sandbox/PVC
home proof. Both proofs must succeed before publication, and policy errors retain
precedence over home errors. The member UID and resourceVersion from that same
home proof still fence the metadata write. Every later admission repeats both
proofs; legacy home admission keeps its sequential policy and recovery path.
The fixed timing label `admission_policy_and_home_proof` reports the combined
overlapping read stage once. This removes one serial read wave, not a read or
a validation boundary. Gated fixtures prove concurrency without timing estimates
and refuse publication on either proof's failure.

Three more native startup boundaries overlap only independent reads. Claim
admission reads its deterministic existing Claim and current Pool/Template
binding together. A replay still validates and returns the original Claim even
if speculative policy reads fail or remain pending; unfinished policy reads are
dropped once an existing Claim is observed. Only a missing Claim consumes that
proof before creation. Legacy claims keep sequential seed and binding validation.

Home adoption overlaps current explicit policy with the initial Claim, member
and PVC read/validation phase. The Claim still supplies validated dependent
names. Both branches finish before the original ordered Claim then Sandbox
annotation writes, using the same UID/resourceVersion preconditions; the final
fresh graph observation remains separate. Initial native bootstrap likewise
overlaps explicit policy with its pre-dispatch graph proof. Policy errors retain
precedence, and assignment is sent only after both pass. The independent proof
after assignment remains unchanged. `assignment_before_proof` includes the
overlapping policy wait as well as graph validation.

These changes remove up to three serial read waves without reusing authority
across operations or mutation boundaries. Gated tests cover both response
orders, failures in either branch, replay after policy failure, ordered writes
and resource-version conflicts. They establish ordering and freshness, not a
production latency reduction; any millisecond estimate requires live timing.
