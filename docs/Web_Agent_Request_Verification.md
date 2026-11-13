# Agent view request verification

Tracks #1667 and #1802, part of #1543. Request reduction is a client behavior claim;
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

The full Console route also includes independent fifteen-second sidebar hierarchy
reads (#1803) and thirty-second accessible-detail revalidation. Their requests hit
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
