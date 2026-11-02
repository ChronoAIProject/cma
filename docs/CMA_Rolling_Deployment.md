# Agent Manager and web rolling deployment

The November 2026 deployment inspection found Agent Manager configured with one
replica and `strategy: Recreate`. Kubernetes therefore removed the serving API
before starting its replacement. Process-local request budgets and visitor Launch
reservations were the documented reason. Web already used the default rolling
strategy, but neither its manifest nor the API policy provided sufficient endpoint
propagation and connection draining. The API also ended HTTP draining after 30
seconds despite accepting 660-second Gateway connections.

Operation budgets now use majority-acknowledged Mongo GCRA buckets in
`request_rate_limits`. Database time controls refill and Retry-After. A local guard
still refuses obvious excess before database I/O; pre-auth and health guards remain
per pod. Failure to check a shared operation budget returns 503. It never grants a
fresh local budget as a fallback. Millisecond refill rounding is conservative.

Visitor Launch allowances commit in the existing Launch admission transaction.
The current platform limit, owner and UTC day determine admission. A shared
`visitor_launch_days` row serializes concurrent replicas. Existing same-day visitor
Launches seed the count during migration and remain a lower bound. Replay returns
the existing Launch without consuming another unit. Failed transactions consume
nothing; HTTP cancellation cannot release an already committed admission.

API and web retain one steady replica, permit one surge replica, set zero
unavailable replicas and require 30 seconds of stable readiness. GKE's NEG readiness
gate must also be healthy. A 30-second preStop delay allows endpoint removal to
propagate while the old process still serves traffic. Both backend policies set
660-second connection draining. API HTTP draining has the same bound and keeps
background workers alive until HTTP finishes; workers then receive their existing
30-second cooperative drain. The 735-second pod grace includes propagation, HTTP,
workers and margin. Web requests nginx graceful quit after propagation and keeps
the hook running until its master exits. Existing versioned web asset retention
must be preserved during image promotion.

Event Gateway retains its existing zero-unavailable rolling strategy and durable
turn drain, with the same 30-second readiness/endpoint margin, 660-second backend
connection drain and 735-second pod grace. Image Manager declares those rollout
and drain margins for activation; this change preserves its zero replica count
and does not activate an unpublished image.

These settings do not migrate or rotate Sandbox runtimes. Existing Sandbox,
Agent, home and conversation IDs remain independently durable. All other process
authorities still need their existing leases and writer fences during overlap.

## First migration from the old deployment

Changing a Deployment does not retrofit a preStop hook or longer grace into its
existing pod. The first promotion must therefore explicitly drain the old API
endpoint while retaining its process long enough for existing connections. Do not
assume the new pod template protects the old pod. Prepare the replacement from a
verified immutable CI image, prove readiness and quota behavior, route new requests
to it, retain the old endpoint for the complete connection-drain window, then
finish the canonical Deployment rollout. Preserve current image/runtime pins and
compare the live resource versions before each mutation so concurrent releases
are not overwritten.

The old binary cannot participate in shared request budgets. Its remaining
in-flight Launches can race the initial shared-quota observation. Route new
admissions exclusively to the upgraded API before ending migration, monitor this
bounded overlap, and retain the durable Launch count as the migration floor.
Subsequent rollouts of compatible binaries use the normal shared authorities.

Verify public web, API readiness, authenticated reads and active conversation
replay continuously during the promotion. Compare retained Sandbox runtime/home
identities and Agent IDs before and after, and check worker recovery and usage
interval continuity. A manifest validation or a Ready pod alone is not evidence of
client continuity. These notes describe the required rollout; they are not a
production deployment record.
