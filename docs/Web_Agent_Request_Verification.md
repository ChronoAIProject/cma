# Agent view request verification

Tracks #1667, part of #1543. Request reduction is a client behavior claim;
compiled fixture results do not prove production capacity or database latency.

## November 13 transport and idle-observation verification

Issues #1799 and #1803 use the compiled public-boundary fixtures with a fixed
100ms delay, one case per browser, Chromium 145, Firefox 146 and WebKit 26.0,
and zero retries. The baseline is `112b09c6b`. New transport selections retain
the post-eligibility identity check because eligibility crosses a network boundary.
The independent application-startup identity read is unchanged.

`IDENTITY_HANDOFF_ENABLED` ships **false** under the owner's default-off directive.
Fresh-check batching is enabled independently. Switching that constant to true
requires owner activation; deployment alone does not enable proof reuse. The
unit suite exercises both states, one-use/250ms expiry, prepared-session
invalidation, pagination, account switches and mutation barriers.

| Retained Agent history | Baseline auth before data, excluding startup | Shipped-default auth before data | Baseline total auth through history/stream readiness | Shipped-default total auth |
| ---------------------- | -------------------------------------------: | -------------------------------: | ---------------------------------------------------: | -------------------------: |
| 1 page                 |                                            3 |                                3 |                                                   10 |                          9 |
| 3 pages                |                                            3 |                                3 |                                                   14 |                         13 |
| 10 pages               |                                            3 |                                3 |                                                   28 |                         27 |

These counts were identical in all three browsers. Total counts include startup,
publication, stream admission and initial queue checks. They are fixture request
counts, not production latency savings. The remaining adjacent checks deliberately
stay enabled in the shipped default.

The separate compiled activation build (`IDENTITY_HANDOFF_ENABLED=true`, used
only for local verification) passed twelve identity cases with zero retries:
one-, three- and ten-page retained selections plus a first-time selection in
each browser. Retained selections used **one** identity request before data,
excluding application startup, and **7/9/16** total auth requests respectively.
The ten-page case saves twelve requests versus the baseline, exceeding the
required eleven. Each additional page adds one post-page identity request.
First-time selection still uses **two** requests before data (eight total): its
eligibility read requires the fresh post-IO barrier. The follow-up identity
precondition review in #1876 tracks that remaining boundary separately;
this change adds no public identity-precondition contract.

Access-request idle observation uses the specification's thirty-second fallback.
`agent_access_requests/lineage.rs` admits live runtime-owned consent without an
`active_turn` condition, and owner service-access replacement can create a request
outside a turn. Pending/connecting/applying requests stay at two seconds; active
empty results back off at 2/4/8/15 seconds and receive durable-cursor hints.
Failures retain the displayed requests and retry at 2/4/8/10 seconds.

Creation categories are immutable in the provenance model and retained only
inside the mounted account/client scope. Private provenance details continue
their independent revalidation. The two personal navigation trees share public
categories; hidden trees stop both badge observers, and unavailable categories
retry at sixty seconds. Hierarchy results retain full descendant reads and use
15/30/60-second backoff, resetting on changes, mutations and return to visibility.

The twelve-Agent fixture has three Sandboxes and six Workspaces. Each idle
window lasts 120 seconds, starting after initial loading and any navigation
switch. The following route counts isolate the four changed observers by
excluding the open Agent's Sandbox detail and private-provenance URLs, which
also have independent observers that remain active.

| Case                                         | Access requests | Channel badges | Other Agents' provenance | Isolated hierarchy routes | Targeted total |
| -------------------------------------------- | --------------: | -------------: | -----------------------: | ------------------------: | -------------: |
| Baseline, never visited environments         |           56–57 |             48 |                       22 |                        72 |        198–199 |
| Baseline, visited environments then returned |              57 |             96 |                       44 |                        72 |            269 |
| Candidate, either navigation history         |               4 |             48 |                        0 |                        27 |             79 |

All three browsers produced the candidate counts above: **60.1–60.3% fewer**
isolated requests before visiting environments, and **70.6% fewer** afterwards.
The hierarchy list itself fell from eight to three reads per window. Known
navigation creation categories were not reread; the open Agent's separate
private-provenance projection still read twice. Same-URL requests starting within
one second of a prior completion fell from 22–70 to zero after visiting
environments. Aborted requests were zero in both fixture variants. All targeted
routes recorded zero new requests during the final ten hidden seconds.

For transparency, including the two independently observed URLs gives 227–229
baseline requests before visiting environments and 299–301 afterwards, versus
106–107 candidate requests. Those broader totals mix the changed navigation
work with the unchanged composer lifecycle and private-provenance observations;
their reductions are approximately 53% and 64%, respectively. The existing
readiness/idle/visibility and slow shared-Sandbox cases also passed with no writes.

The optional replacement of per-Agent channel reads with provenance inventory
has not been selected: no matched real-Mongo endpoint-cost measurement was made
in this web fast-lane change. Existing channel read concurrency, partial results,
12-second batch budget and 30-second/55-second authority cadence remain intact.

Production measurements are not available for this change. Deployment and the
dedicated-account acceptance run belong to the release/deployment engineers and
require the lead's exact artifact handoff. No production latency saving or
production request-rate reduction is claimed here; savings are unknown until
that acceptance is recorded. GitHub issue-comment evidence remains draft-only.

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
