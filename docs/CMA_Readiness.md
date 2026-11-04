# Agent Manager dependency readiness

Refs #1343. Agent Manager keeps process liveness separate from application-store
readiness. `/livez` returns `200 ok` without consulting MongoDB. `/ready` returns
200 only when the most recent completed application-store check succeeded and
its monotonic age is less than five seconds. Startup, dependency errors,
expired checks and stale results return 503.

Each composed health router owns one background check loop, shared by all its
requests and clones. The check uses the application's Mongo client and has a
two-second application deadline covering server selection, connection checkout
and ping. The next attempt starts one second after completion. Requests read the
cached result immediately and never enqueue a Mongo operation. There are no
concurrent checks or catch-up ticks. A separate healthy connection pool cannot
mask a stalled application pool.

The JSON response retains `status` (`ready` or `not-ready`) and the `store`
failure reason. Its `dependency` object includes the last completion's UTC
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

## Verification

From `api/`, run the health regressions with bounded compilation resources:

```sh
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 RUST_MIN_STACK=33554432 \
cargo test --locked -p cma --bin cma health:: -- --test-threads=4
```

The loopback peer binds an ephemeral localhost port and requires neither a
Mongo installation nor production credentials. Unit tests also cover freshness,
startup, errors, single-flight behavior and dependency-independent liveness.

## Production verification

The deployment owner should verify the released commit and observe the normal
kubelet and load-balancer probes over representative busy periods. Confirm
`/ready` responses complete comfortably below the existing three-second probe
budget, readiness state-change logs contain bounded durations, `/livez` stays
responsive, and successful dependency checks resume after transient failures.
Check application admission and read latency alongside readiness; a successful
ping alone does not establish acceptable application throughput.

Exercise stalled checkout and unanswered commands only in an authorized
non-production saturation test. Verify 503 readiness, continued 200 liveness,
connection release, and automatic recovery. Do not stall the production database
or change replica count, pool size, probe manifests or alerting for this check.
The deployment owner closes #1343 only after production verification.
