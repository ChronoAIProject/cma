# ADM-078 platform cloud observation

The case records one synthetic cloud observation through the real public
operator API in the sole parent's isolated deployment. It requires an exclusive
settings lease, enabled platform cloud metering, a writable baseline on both AMs,
and quiescent accounting/evaluator state for complete no-effect comparisons.
Setup supplies positive safe integer units and a fresh timestamp within the
current period, later than any current cloud observation and never previously
recorded in that period. The source parser accepts exactly
`YYYY-MM-DDTHH:MM:SSZ`; a JavaScript ISO string with `.000Z` is not accepted by
this API. Setup must supply the real whole-second form.

Both AMs must initially agree on public settings, availability and durable spend
state. The original POST body and ingress are retained before dispatch. Its
successful response must identify the exact units, timestamp and settings
revision, with valid recorded metadata. The case compares the entire durable
response and ledger against the prior state with only the cloud observation
replaced. A missing initial period ledger is supported: the source-defined open
ledger is derived from the actual public period settings, with zero counters,
no closures and no evaluation timestamp. It is never supplied by a fixture
success oracle. Both AMs must read back the accepted complete state.

`committed` is calculated at read time using the observation plus a wall-clock
projection. It may legitimately change between requests or be absent when the
projection is unavailable. Only that derived field is excluded from durable
equality. Every raw response retains it, the complete ledger, request ID,
serving snapshots and attribution. The shared request runner checks the AM
actually reached before allowing the case to proceed.

Eight source-defined invalid variants run through each AM: unsafe integer,
negative units, fractional units, future timestamp, pre-period timestamp,
malformed timestamp, unsupported source and an unknown request field. Each must
return exactly HTTP 422 / `invalid_request`; both AMs must retain the complete
accepted ledger after every refusal. With the authentic parent writer gate
disabled and read back through settings on both AMs, each AM must return HTTP
503 / `model_budget_writes_unavailable`. A non-operator sends a valid changed
value through both AMs and must receive HTTP 403 / `forbidden`. Refusal bodies
must contain no accounting values. All refused requests have full no-effect
readbacks; a successful-looking refusal that changes spent, reserved,
adjustments, history metadata or any other durable field remains a failure.

The case records mode intent before calling the parent's existing
`setWritesAvailable(boolean)` method. One memoized finalizer restores the
original writer mode, then independently reads settings and spend on both AMs.
It preserves restoration and each read failure separately. Unknown positive
write outcomes retain both-AM observations and remain unproven even when a
matching cloud entry is visible. No write, refusal, or restoration is replayed,
and no compensating cloud observation is submitted. The accepted immutable
observation and audit record remain until the parent disposes the local
database. Parent cleanup still runs after contributor cleanup fails.

This is a finite sequence: one successful observation intent, sixteen invalid
intents, two unavailable-writer intents and two forbidden intents. The healthy
path makes 75 public HTTP requests and two parent-owned mode selections. The
parent must admit a sufficient bounded case budget and retain real mode-change
evidence; it cannot fabricate a writer flag or run a second application campaign
from this case. Six automatic public recipes / 31 parent-required factories
remain unchanged. Offline models prove harness behavior only. Runtime acceptance
requires the sole parent's actual two-AM runs, restart witness and final disposal.
