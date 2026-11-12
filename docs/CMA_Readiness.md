# Agent Manager dependency readiness and background work

Refs #1343. Agent Manager keeps process liveness separate from application-store
readiness. `/livez` returns `200 ok` without consulting MongoDB. `/ready` returns
200 only when the most recent completed application-store check succeeded,
its monotonic age is less than five seconds, and all required local scheduling
capabilities are available. See [worker supervision](CMA_Worker_Supervision.md)
for the worker inventory, recovery limits and bounded `workers` response. Startup, dependency errors,
expired checks and stale results return 503.

Each composed health router owns one background check loop, shared by all its
requests and clones. The check uses the application's Mongo client and has a
two-second application deadline covering server selection, connection checkout
and ping. The next attempt starts one second after completion. Requests read the
cached result immediately and never enqueue a Mongo operation. There are no
concurrent checks or catch-up ticks. A separate healthy connection pool cannot
mask a stalled application pool.

The JSON response retains `status` (`ready` or `not-ready`) and the `store`
failure reason; local execution capability loss uses `workers`. Its `dependency` object includes the last completion's UTC
`checked_at`, monotonic `age_ms`, `duration_ms`, and fixed failure reason:
`pending`, `unavailable`, `deadline`, or `stale`. Pending checks have null timing
fields. Healthy results have a null failure reason. Readiness state changes are
logged with fixed categories and duration, without Mongo error strings or data.

Deadline and router shutdown cancellation drop the read-only ping future. For
the locked MongoDB driver, dropping a checked-out connection returns it to the
pool; a connection interrupted during a command is closed rather than reused.
Ordinary tests exercise this path with the real driver against a loopback Mongo
wire peer, including checkout saturation, an unanswered command, socket closure,
subsequent pool reuse, and recovery. This is a specific ping contract, not a
claim that arbitrary Mongo transactions are cancellation safe: the driver warns
against general future cancellation, and cancelled writes can have uncertain
outcomes. Keep these regressions when updating the driver.

Dropping the last health router also stops its background loop. If a loop stops
unexpectedly while the router remains, the freshness limit prevents its last
success from reporting readiness indefinitely. A healthy cached result can
remain visible while the next check is in progress, until that check fails or
the freshness limit is reached.

Readiness-check result, completion timestamp and duration are also exported on
the private metrics listener. Query result and freshness together; these metrics
do not establish worker progress. See [CMA observability](CMA_Observability.md)
for the worker signals and collection/alert integration contract.

## Background Mongo budget

Agent connection reconciliation, Sandbox maintenance/reconciliation and online
Agent provenance backfill share a process-local budget of two concurrent work
chains. A deferred operation is not constructed or polled, so it does not enter
Mongo's checkout queue. Each worker is single-flight and releases its permit
before waiting for another attempt. This budget applies to these three worker
families; it is not a limit on every Mongo operation in the process.

Public/tool request handling and admitted Agent, Sandbox and Workspace lifecycle
execution mark foreground activity without waiting for background permits.
While foreground activity is present, new background admission is reduced to
one concurrent chain. Existing work finishes or reaches its deadline; one
background chain can still progress under sustained interactive traffic.
Request priority ends when response headers are constructed, so an open SSE
stream does not retain it indefinitely. This controls competing demand on the
shared client without changing the client's pool or connection settings.

Reconciliation attempts have a 30-second deadline; provenance scans and rows
retain their 15-second deadline. Successful reconciliation intervals are up to
one second for Agents and 15 seconds for Sandboxes. Provenance rows wait up to
250 ms. Busy-budget retries wait up to 250 ms; failed, timed-out or cancelled
attempts back off exponentially from two seconds to a maximum of 60 seconds.
These waits use jitter between 75% and 100% of the stated bound. The backfill
still waits 30 seconds at startup and five minutes between full sweeps. Budget
deferral retains the current backfill page/row; corrupt or unavailable rows
retain the existing skip-and-revisit behavior.

Cancellation drops the in-flight operation before releasing its permit. The
real-driver tests verify connection release on both a background deadline and
explicit task cancellation. As with readiness, this proves the checked-out
command cleanup path, not transaction outcome certainty. Reconciliation keeps
the existing durable authority/idempotency checks; cancellation never records
an uncertain write as a success or replays accepted user input.

Fixed-category completion logs record work kind, reason, elapsed time, pending
age, completed count and retry delay. A sweep deadline restarts reconciliation
from its existing discovery boundary. Repeated deadlines may delay later
candidates; production verification must confirm completed sweeps and backlog
progress as well as bounded durations. The budget does not cure a database or
network outage, and readiness correctly remains 503 if its dependency cannot
make progress.

## Verification

From `api/`, run the health regressions with bounded compilation resources:

```sh
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 RUST_MIN_STACK=33554432 \
cargo test --locked -p cma --bin cma -- health:: background:: \
  sandbox::agent_scheduler:: sandbox::provenance_backfill:: --test-threads=4
```

The loopback peer binds an ephemeral localhost port and requires neither a
Mongo installation nor production credentials. Unit tests also cover freshness,
startup, errors, single-flight behavior and dependency-independent liveness.
Background regressions cover shared concurrency, priority, retry/backoff,
permit release, scheduler admission during saturation and backfill cursor
preservation while the budget is busy.

## Production verification

The deployment owner should verify the released commit and observe the normal
kubelet and load-balancer probes over representative busy periods. Confirm
`/ready` responses complete comfortably below the existing three-second probe
budget, readiness state-change logs contain bounded durations, `/livez` stays
responsive, and successful dependency checks resume after transient failures.
Check application admission and read latency alongside readiness; a successful
ping alone does not establish acceptable application throughput.
Compare background completion/pending-age logs and reconciliation candidate
progress during the same busy windows. Confirm admitted lifecycle operations
continue while optional workers are deferred and that backfill/reconciliation
recover without a burst when dependency progress returns.

Exercise stalled checkout and unanswered commands only in an authorized
non-production saturation test. Verify 503 readiness, continued 200 liveness,
connection release, and automatic recovery. Do not stall the production database
or change replica count, pool size, probe manifests or alerting for this check.
The deployment owner closes #1343 only after production verification.
