# Agent view request verification

Tracks #1667, part of #1543. Request reduction is a client behavior claim;
compiled fixture results do not prove production capacity or database latency.

## Observation policy

The mounted account view coalesces concurrent identical Sandbox, Workspace and
Agent detail reads. Each caller keeps its own cancellation. The shared transport
is canceled when the last caller leaves; errors and settled values are never
cached. A different mounted account gets a separate scope. Authorization,
mutations, streams and differently projected resource-access reads are not
coalesced or cached by this helper.

Preparation reads are sequential, with one-, two-, four- and five-second delays
and bounded ten-percent jitter. A durable conversation-available event can wake
the next read, with a one-second minimum between wake attempts. Events are hints,
not authority: the same current identity and lineage checks still gate reads and
publication. Hidden tabs suspend readiness; the loop stops on settled preparation.
An unchanged active first-message state on v2 uses its existing durable stream
instead of reloading transcript/history. Retained legacy tabs keep their JSON
reconciliation path while first-message delivery is pending.

Unchanged Sandbox transitions back off from the initial 1.5-second readiness
cadence to at most 5.5 seconds. State or phase progress resets that cadence.
After readiness, the separate five-second lifecycle observation remains: the
Agent stream does not expose the full checkpoint, operation, runtime and input
eligibility contract. Its ten-second freshness expiry, denial handling and late
result fencing remain enforced. Removing this check needs a complete lifecycle
subscription or a compatible bounded lifecycle endpoint; ordinary conversation
traffic cannot prove lifecycle readiness.

Queue discovery uses input/turn/status changes as hints instead of every usage
or text frame. Nonempty queues retain their two-second observation; empty active
queues use a ten-second fallback to discover changes from another tab. Idle empty
queues wait for events, focus or explicit actions. A compatibility 404 retries
every thirty seconds or on a hint; it never clears a prior real error. Successful
reads still have fresh authorization checks before and after data retrieval.
Stream reconnects without durable progress back off with equal jitter (half to
all of each delay, capped at ten seconds) and pause
while hidden. A healthy existing stream stays connected.

## Local comparison

Run the compiled preview with the recorded, loopback-only fixtures in
`web/e2e/ui-performance.config.ts`. The request-count cases use no real account.
The baseline and candidate use the same browser versions and boundary delay.
Set `CMA_REQUEST_BASELINE=1` only when measuring the unmodified baseline; candidate
runs must enforce the reduced-request assertions. Keep retries disabled.

Measure opening and sending one fixture message, sixty seconds of usage frames,
return navigation, thirty seconds of prolonged readiness, thirty seconds of idle
viewing, thirty hidden seconds and a repeated-focus reopen. The slow-Sandbox case
checks overlapping consumers against a three-second boundary response. Record
waterfalls, HTTP status classes, phase marks, script errors and entry/asset hashes.
Distinguish preparation reads from the independent thirty-second access refresh.
Do not remove authorization or error assertions to lower request counts.

## Deployed acceptance

Wait for the lead's issue-specific GO and exact Web/API artifact identities. Use
only the supported security harness and dedicated test accounts. The release
owner verifies the approved deployed build identity, entry and transitive assets,
initializers and blank-page guard before request evidence is accepted.

Use equal-duration before/after cases for the same existing authorized Agent and
browser. Observe active work only if a supported test Agent is already active;
never invoke an Agent, submit a message, change sharing, restart a resource or
create production load for this read-only check. If a needed case is unavailable,
record it as not run and retain local-fixture evidence separately.

Correlate browser phase boundaries to completed API log records using request
SHA-256 hashes through the approved harness. Aggregate by method and normalized
route template, including auth, Sandbox/Workspace/Agent detail, transcript, usage,
head/snapshot, event stream, queued inputs, command status and response detail.
Report started browser requests and completed server requests separately, with
status counts, duration, case count and active/idle/hidden mix. Normalize all
resource identifiers; retain no credentials, message contents or identities.

Requests per view require a counted view denominator. Requests saved per minute
are the difference in request rates for matching scenarios and routes. Daily fleet
totals alone cannot establish either a per-view rate or causality. Backend changes
can affect latency independently from client request reduction.

Require preserved history, drafts, live updates, reconnect behavior, fresh
revocation checks and honest loading/error states. Keep #1667 In Progress until
approved deployed acceptance is complete, then close it with aggregate evidence
and set Project 9 to Done. Merging does not authorize production execution or
close the broader capacity gate.
