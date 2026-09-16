# Bridge operational signals

CMA emits structured tracing fields under `cma::bridge::operations` through the
existing application log sink. The deployment's default `RUST_LOG=info` includes
them. These observations do not grant authority or replace permanent command,
allocation, source and outbox receipts. No metric exporter or public endpoint is
added. Existing log collection can derive counters and latency distributions
from the following fixed event fields.

| Event | Fixed dimensions | Numeric observations |
| --- | --- | --- |
| `channel_operation`, operation `ingress` | outcome | elapsed_ms |
| `channel_operation`, operation `worker` | worker, outcome | elapsed_ms |
| `channel_operation`, operation `outbound` | outcome, reason | elapsed_ms, age_ms |
| `channel_scheduler` | worker | active, queued, discovering |

Worker kinds are inbox, allocation, renewal, child_cleanup and workspace_cleanup.
An `observed` worker result means that the bounded attempt returned successfully;
it does not claim provisioning, deletion or response completion. Scheduler queue
size is the local discovered page awaiting execution, not a global backlog count.
The scheduler emits its bounded occupancy at most once per 30 seconds. Each
completed work attempt emits its result and duration independently.

Ingress outcomes include persisted messages, persisted/refused control intents,
authentication failures, invalid payloads, capacity refusal, missing keys or
binding, disabled ingress and `acceptance_unknown`. The last outcome retains
uncertainty: the operator must recover the original callback receipt rather than
assume the request was never accepted. Unauthenticated headers, IDs, raw routes
and bodies never enter these events.

Outbound outcomes are confirmed, rejected, not_sent and unknown. In particular,
throttling without positive no-send evidence stays unknown; the same reason with
positive no-send evidence is not_sent. Credential expiry/revocation, unavailable
routes, lost responses and provider failures have bounded reason codes. The
age is elapsed wall-clock time since original outbox creation, clamped at zero
if the clock moves backward. Delivery latency measures this adapter attempt and
includes its local authority checks. A confirmed adapter result does not by
itself prove the subsequent Mongo receipt commit; consult the public outbox view.

Outbox events carry the internal outbox ID for exact receipt lookup. Worker
events carry SHA-256 of the persisted work ID, enabling comparison with a known
inbox/allocation ID while suppressing corrupt raw discovery values. These fields
are correlation data only: never extract them, a sender ID, a Bridge ID, a chat,
a message or an Agent ID as metric labels. Message text, provider receipt IDs,
reply contexts, credentials, bot tokens and URLs are excluded.

Recommended log-based monitors count authentication and capacity failures by
outcome, count outbound unknown/expired/revoked results by reason, and track
worker latency/failures and outbox age distributions by fixed worker/outcome.
Compare sustained scheduler saturation or rising outbox age with ordinary CMA
request errors and lifecycle retry events. The configured alert window and
thresholds must reflect the deployed load; these source changes do not create
cloud alert policies or claim an installed alert integration.

For an unknown delivery, use its original outbox ID and the public explicit
recovery operation described in [redelivery](BRIDGE_REDELIVERY.md). For lifecycle
stalls, inspect the retained allocation and sanitized retry reason described in
[lifecycle recovery](CMA_Channel_Lifecycle_Recovery.md). Neither signal authorizes
an automatic resend, database rewrite or cleanup of another user's resources.
