# Agent view request verification

Tracks #1667 and #1802, part of #1543. Request reduction is a client behavior claim;
compiled fixture results do not prove production capacity or database latency.

## November 13 transport and idle-observation verification

Issues #1799 and #1803 use the compiled public-boundary fixtures with a fixed
100ms delay, one case per browser, Chromium 145, Firefox 146 and WebKit 26.0,
and zero retries. The baseline is `112b09c6b`. New transport selections retain
the post-eligibility identity check because eligibility crosses a network boundary.
The independent application-startup identity read is unchanged.

The affected local Vitest run passed 479 tests across 47 files. Coverage for the
new identity gate, access observation/hook and creation-scope modules was 98.34%
statements, 90.9% branches, 97.22% functions and 100% lines. Web lint, typecheck
and E2E typecheck passed. These are the owner's fast local gates; they do not
represent a full CI run or production acceptance.
The whole-file Prettier check for `Web_Style.md` has existing upstream failures;
the candidate introduces no additional formatting changes, and its changelog
remains append-only.

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
eligibility contract. Each observation expires after its next poll delay plus
the ten-second request timeout (15 seconds settled, at most 15.5 seconds during
readiness). A successful slow refresh does not pause sending within this window.
One transient failure retains still-valid evidence; repeated failures and expiry
hold input. Foreground return retains valid evidence or shows checking until a
new observation arrives. Immediate denial handling and late
result fencing remain enforced. Removing this check needs a complete lifecycle
subscription or a compatible bounded lifecycle endpoint; ordinary conversation
traffic cannot prove lifecycle readiness.

Owner Sandbox and Workspace pages share the composer observation-key backoff,
including Workspace status where applicable. Their prolonged transition ceilings
are fifteen seconds after two unchanged minutes and thirty seconds after ten;
settled pages remain at five seconds. Visibility and explicit revision/retry reset
readiness. Failure retries remain five seconds, with refusal/missing-resource
handling unchanged. Child reads depend on Sandbox identity/revision and Workspace
membership/status/counts, with a fifteen-second visible fallback for child-only
Agent changes and a thirty-second interval after ten unchanged minutes. Retained
children keep their refresh deadline across navigation; account/client changes
cannot reuse them. Wrapper source identity is metadata in a WeakMap, never an
added client field or an authorization cache.

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

The `hierarchy-polling.check.ts` compiled fixture fixes the Sandbox at
`credential_rotating` and its Workspace at `ready`. Baseline mode records five
real minutes with one and four tabs on each route; candidate mode records twelve
real minutes in Chromium, Firefox and WebKit. It uses the same 100 ms public
boundary delay and records route counts, per-minute counts, adjacent start and
completion gaps, same-URL overlaps and unexpected writes/errors. Component timer
tests separately enforce the page poller's 14/5/3 request bounds for minute one,
minute three onward and minute eleven onward, plus state/visibility/action resets.

The baseline Console route also includes independent fifteen-second sidebar
hierarchy reads and thirty-second accessible-detail revalidation. Develop now
backs off unchanged sidebar reads through 15/30/60 seconds (#1888 / #1803). Their requests hit
the same URLs and must remain in full-route totals; the owner-page limits alone
cannot establish those limits for the whole Console. Do not suppress authorization
or remove sidebar traffic from the fixture to claim the full-route target.

The 2026-11-13 Chromium baseline used the compiled `a3a47f8dc` page behavior
(with the behavior-preserving composer extraction), 100 ms fixture RTT and five
real minutes per tab. Counts include the full Console, not only owner-page reads:

| Route / tabs | Sandbox detail per tab | Workspace detail per tab | Agent detail per tab |
|---|---:|---:|---:|
| Sandbox / one | 209 | 199 | 186 |
| Sandbox / four | 208–211 | 198–199 | 186 |
| Workspace / one | 187 | 204 | 0 |
| Workspace / four | 192–193 | 198–203 | 0 |

Median same-route completion-to-next-start gaps were approximately 1.5 seconds
on the Sandbox route and 1.6 seconds on the Workspace route. Independent access
revalidation produced one Sandbox overlap per Sandbox tab and one to three
Workspace overlaps per Workspace tab; other measured routes had none. All ten
tabs completed with no unexpected fixture requests, script errors or writes.
These full-route overlaps differ from the serialized owner-page poller and do
not identify the production client. Baseline entry SHA-256:
`18fc54f6f83dfe26c2b6e3829d7d6cb9b3b89cb8f2a223b0401d84f24403850b`.

The candidate (page changes through `26f6a325a`, before integrating #1888) passed
six twelve-minute cases, one per route in Chromium, Firefox and WebKit, with zero
retries, writes, unexpected requests or script errors. The same Chromium
five-minute window shows the isolated effect of this change:

| Route | Detail | Baseline | Candidate first five minutes | Reduction |
|---|---|---:|---:|---:|
| Sandbox | Sandbox | 209 | 64 | 69.4% |
| Sandbox | Workspace | 199 | 35 | 82.4% |
| Sandbox | Agent | 186 | 19 | 89.8% |
| Workspace | Sandbox | 187 | 55 | 70.6% |
| Workspace | Workspace | 204 | 65 | 68.1% |

Across all three engines, twelve-minute Sandbox-page totals were 129–130 Sandbox,
87–89 Workspace and 43 Agent reads; Workspace-page totals were 103–106 Sandbox
and 129–130 Workspace reads. The compiled candidate entry SHA-256 was
`a4ed484a14411b7d663d76e94213b7b92898379748aaeaf27a52cd98b042a980`.

The full-route targets are **not met** by #1802 alone: minute three onward still
included 9–10 Sandbox reads on the Sandbox page, with 7–8 in minutes eleven and
twelve; the independent sidebar and accessible-detail observers remain present.
Full-route observations also retain up to one same-URL overlap per measured
route, while the owner-page and child-hook tests enforce zero overlapping reads
within each observer. The compiled check asserts a total budget that includes
those independent consumers and records the full waterfall; passing that check
does not claim the issue's stricter full-route acceptance. #1888 separately backs
off navigation; its effect is excluded from the before/after figures above.

The final affected component run passed 293 tests across 27 files. Coverage for
`pollDelay`, `useSandboxContents`, `useOwnerWorkspace` and `detailReads` was 95.51%
statements, 90.72% branches, 96.82% functions and 97.59% lines. Web lint, typecheck,
e2e typecheck and targeted Prettier passed. Existing composer polling assertions
were unchanged. Retained-view tests render the actual Console wrapper and cover
revisit, profile/child reuse, access refusal and different account/client scopes.

The November 12 production loop remains **unattributed (case C)**: the available
read-only Cloud Logging identity was denied access to load-balancer log views for
06:46:29–06:52:10 UTC. No browser/nonbrowser source, address count, referer or
connection concurrency was established. SRE can supply aggregate categories
without raw client addresses or identifying User-Agent values.

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

For #1802, deployment engineers must record exact Web/API generations and observe
a dedicated authorized transitional Sandbox page for ten minutes, with the same
case before and after deployment: both detail routes' per-minute counts, p50/p95,
and share of completed HTTP handler time. If no suitable transitional resource is
available, record that case as unobserved. This work does not establish the owner's
one-second production Agent-tab criterion until deployed end-to-end measurement.

Rollback uses the existing Web generation rollback. `PROLONGED_POLL_MS` and
`LONG_POLL_MS` in `sandbox/pollDelay.ts` control the prolonged page ceilings; setting
both to 1500 restores the old transition cadence. There are no new runtime flags
or infrastructure changes. Keep the composer's independent maximum unchanged.
