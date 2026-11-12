# Worker supervision local validation

Scope: #1740; implementation branch `feature/cma-worker-supervision-1740`.
The initial checks used `feature/cma-observability-1470` at
`7bd7a5dfa21d3c9759bfaef5cb22443854ddd6cc`. After #1664 merged, the change was
rebased onto `develop` at `fa292b83473aa414a43ab9e23edc14a40e8b0aea`.
The follow-up PR targets `develop` and excludes #1664's existing changes.
Results below identify which checks preceded that synchronization.

## Fault matrix

| Case | Local check / required observation |
| --- | --- |
| Top-level panic | Common supervisor test: immediate observation, one-second first backoff, same owned queue and children, one child result |
| Unexpected top-level exit / storm | Five failures within sixty seconds enter a sixty-second cooldown; shutdown during cooldown does not restart |
| Suspended scheduling future | Six seconds to detect, seven seconds to resumed scheduling using paused monotonic time; required readiness fails during recovery |
| Child panic / failure | Existing Launch, Agent, command and lifecycle scheduler regression groups; only the child's slot is released |
| Hung read-only discovery | Five-second timeout; later scan and one admitted-work simulation complete by the next normal tick |
| Full slots / long reasoning | Forty seconds with one occupied Agent slot; loop stays available, one execution call, no duplicate/aborted reasoning |
| Zero due work | Existing idle/polling tests; successful scheduler activity does not require a business completion |
| Mongo outage / checkout saturation | Cached readiness and real-driver loopback health fixtures; no per-request Mongo work and liveness remains 200 |
| Optional warm capacity | Required capabilities can become ready while warm pools remain unavailable; one batch child preserves serialization |
| Shutdown / child cleanup | Pre-signalled shutdown never starts work; bounded draining; aborting the owning supervisor cancels its structurally owned child |
| Unknown commit | Existing failpoint regression recovers the original queued-input receipt and one effect |
| Stale writer / overlapping owners | Existing replica-set regression rejects the old Agent work holder after lease takeover; concurrent command reservation shares one receipt |

The supervisor's cancellation boundary contains only scheduling awaits. Execution
children, queues, in-flight IDs, paging cursors and local deferrals live outside
it. Mongo and provider effect contracts are unchanged. The core tests prove local
ownership behavior; a child result from an in-memory probe is not called a durable
Mongo or provider result.

## Reproduction and evidence

Build and run the common supervisor tests with:

```sh
cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p cma --bin cma worker_supervision -- --nocapture
```

The same test binary can run the `scheduler`, `health::`, `warm_pools::`,
`trigger::worker_tests`, `shutdown::tests`, `background::tests` and
`telemetry::worker::` filters. Mongo tests require disposable replica-set URIs
in `CHRONO_TEST_MONGODB_URI` and `CHRONO_TEST_MONGODB_FAILPOINT_URI` and run
serially because they install server failpoints. Never point these tests at a
shared or production database.

## Results

Before the `develop` synchronization, the focused suite ran 130 relevant tests successfully: six supervisor tests,
47 scheduler tests, eleven health/real-driver loopback tests, 29 warm-pool tests,
23 Launch worker recovery tests, two shutdown tests, six background-budget tests,
two worker-metric tests and four real Mongo replica-set regressions. The ordinary
warm-pool selection reports nine ignored tests; one of those (durable pool target
replay, lease/revision and UID binding) was run explicitly on Mongo 8.0.13. The
remaining eight are not claimed as executed. Final command summaries and exact test names are
in the evidence bundle.

The Launch worker group initially exhausted the default macOS test-thread stack.
It passed all 23 tests with `RUST_MIN_STACK=33554432`, matching the repository's
backend CI contract. No Launch execution code was changed to obtain this result.

Release/probe workflow contracts rejected all 146 unsafe mutation cases; source
formatting, source-size checks and compilation also passed. Strict Clippy finds
one existing `clippy::redundant_iter_cloned` diagnostic at
`sandbox_metrics/observer.rs:34`, unchanged from the base SHA. The follow-up
lint invocation keeps that diagnostic visible with `--force-warn` and retains
`-D warnings` for the other lints; that invocation passed. The original strict
failure is retained too.

The local validation environment is macOS/Apple Silicon. Docker's attempt to pull
the CI-pinned Mongo 8.0.13 image failed with `read-only file system` while creating
a daemon lease. The first native fallback, MongoDB 5.0.30, rejected the existing partial-index
`$in` expression during fixture initialization; its failed runs are retained as
environment failures. Tests then passed on a fresh isolated native MongoDB
8.0.13 replica set downloaded from MongoDB's official macOS ARM64 distribution,
with its archive hash and version recorded. Both servers bound only to ephemeral
loopback ports and are stopped after verification. These local results do not
qualify the deployment container image or production.

Raw results, command arguments, durations, environment/configuration, base SHA,
dirty-file digests and artifact hashes are retained in the local evidence bundle
at `/tmp/cma-1740-evidence`. The metadata explicitly records absent deployment
candidate digests rather than substituting the base SHA or a test executable.

## Deployment acceptance still required

Paused-time timing assertions establish local scheduling bounds. They do not
measure deployed fault-to-detection, safe scheduling and confirmed durable work
progress. Repeat those measurements separately against exact API/runtime image
digests and the intended deployment/Mongo configuration, retaining the original
work identity, state/receipt evidence and source/artifact hashes.

Existing long claims (including the fifteen-minute Launch default), dependency
outages and unknown provider outcomes remain explicit blockers to a universal
thirty-second durable-work-recovery claim. No faster local restart waives those
dependencies. Confirm active Sandbox/runtime/home/conversation preservation and
valid long reasoning in isolated provider journeys. Production collection and
notification/journey acceptance remain in chrono-probe#18.

## Actual local service verification before synchronization (2026-11-12)

Built and started the actual `cma` main binary with `cargo build --manifest-path
api/Cargo.toml -p cma --bin cma`, using an isolated native MongoDB 8.0.13 replica
set and database `cma_worker_supervision_service`. The binary SHA-256 was
`4baddf5a251d9371cb85041b71a4820e31a7f35a2f515368a1e6cb4180dbd942`.
The HTTP and metrics listeners bound only to `127.0.0.1`, on ports 58483 and
58484 respectively. No production Kubernetes context was used: the native
provider used an explicitly configured TLS loopback fixture returning empty
inventory and rejecting every mutation with HTTP 503. Warm/prepared targets
were zero. This exercises real application composition and Mongo stores, but
does not qualify Sandbox creation, runtime execution, home preservation or model
inference.

| Observation | Actual process result |
| --- | --- |
| Startup and reads | `/ready`, `/livez`, `/api/v1/sandboxes` and `/api/v1/conformance` returned 200 |
| Worker health | All six required workers and optional `native_warm_pools` reported `running`; all seven heartbeat gauges advanced over 15 seconds of idle serving; restart counters stayed zero |
| Mongo interruption | Suspended only the disposable Mongo process with SIGSTOP for 5.145 seconds; readiness first reported HTTP 503 with reason `store` after 3.073 seconds; liveness stayed 200 in every sampled request |
| Recovery | SIGCONT restored Mongo; the same CMA PID 3591 returned to HTTP 200 readiness after 1.027 seconds; Sandbox reads succeeded and restart counters remained zero |
| Cooperative shutdown | A separately owned CMA process received SIGTERM and exited with code 0 after 29.973 seconds; the HTTP listener closed, all seven workers logged `stopped`, and no drain deadline warning appeared |
| Restart | Restarted against the same isolated database; all four checked endpoints again returned 200 and all seven workers reported `running` |

Timings are observed wall-clock values from this one local run with HTTP sampling,
not guaranteed bounds. The provider fixture's intentional 503 responses exercised
optional warm-pool write retries, which account for the long shutdown drain even
with an empty execution database. No new executions or in-flight reasoning were
admitted. This process check does not inject scheduler panic/stall; those recovery
cases remain covered by the focused tests above.

Startup initially rejected a plain HTTP public origin; the local advertised origin
was corrected to HTTPS while the directly probed listener remained HTTP loopback.
The TLS fixture also required separate CA and server certificates. These were
temporary harness corrections; application validation and TLS verification were
not disabled.

Service logs, HTTP observations, metric snapshots, fault/shutdown results, final
running state and source/binary hashes are in
`/tmp/cma-1740-evidence/service-startup/`. Temporary launch configuration is in
`/tmp/cma-1740-service/`; credentials and private keys are excluded from the
evidence bundle. The last verified CMA PID was 3823 and it was left running for
local inspection, together with its loopback Mongo and provider fixture.

## Final synchronized verification (2026-11-12)

The final code checks used source `ae92da71f`, based on `develop` at
`fa292b83473aa414a43ab9e23edc14a40e8b0aea`, and the repository-pinned Rust
1.96.1. The pinned test executable SHA-256 is
`5337bfff5f16e7d2b0cc8834e2fb67c1656393c8b92ae16da1c1a1c151b08281`.
Strict `cargo +1.96.1 clippy --locked --manifest-path api/Cargo.toml -p cma
--all-targets --all-features -- -D warnings` passed without lint overrides.
The earlier pre-synchronization Clippy diagnostic above is historical and does
not apply to this pinned final result. Formatting, source-size, configuration
key/documentation, API documentation and provider/release workflow contract
checks also passed.
All 136 focused tests passed: eight supervisor tests, 51 scheduler tests,
eleven health tests, 29 warm-pool tests, 23 Launch recovery tests, two shutdown
tests, six background-budget tests, two metric tests, and four explicitly run
real MongoDB 8.0.13 replica-set regressions. The default warm-pool selection
still leaves nine tests ignored; its fencing/replay test was then run explicitly.
Eight other opt-in warm-pool integration tests are not claimed as executed.
The new regressions verify recovery while all Sandbox/Workspace slots hold long
children, retained-child draining during backoff, and immediate shutdown of a
suspended scheduling future. No duplicate child execution was observed.

The opt-in whole-service test also passed. It starts the actual `serve()`
composition in a **test executable**, opens real HTTP/metrics listeners, and uses
the isolated Mongo database and TLS provider fixture described above. Faults
are injected at the scheduling future's poll boundary; all injection code and
state are `#[cfg(test)]` and absent from ordinary service binaries. There is no
production fault endpoint or deployment configuration switch.

| Worker | Fault | First unavailable state (seconds) | Running and ready again (seconds) |
| --- | --- | ---: | ---: |
| `launches` | panic | 1.081 | 2.105 |
| `agent_commands` | panic | 0.108 | 1.092 |
| `sandbox_commands` | panic | 0.765 | 1.753 |
| `agents` | panic | 0.941 | 1.921 |
| `sandboxes` | panic | 0.950 | 1.994 |
| `workspaces` | panic | 0.905 | 1.908 |
| `native_warm_pools` | panic | 1.132 | 2.212 |
| `agent_commands` | exit | 0.237 | 2.254 |
| `agent_commands` | stall | 5.150 | 9.845 |

All nine cases ran in the same PID 22735. Each target's restart counter increased
by exactly one, with the corresponding panic/exit/stall reason; other workers
stayed running and their restart counters did not change. Required capability
loss returned `/ready` 503 with reason `workers` while cached Mongo stayed
healthy. Optional warm-pool failure kept readiness 200. Every sampled `/livez`
response was 200, and Sandbox list reads succeeded after each recovery.
The service then handled SIGTERM and returned successfully through its normal
shutdown path. The external sampler retained 422 HTTP observations and 40 metric
scrapes. These are individual observed wall-clock timings, not deployment bounds.
The stall followed two earlier failures of the same worker, so its four-second
backoff accounts for the longer recovery time.

Reproduce the whole-service test with disposable loopback Mongo/provider settings
and the explicit isolation guard:

```sh
CHRONO_TEST_WORKER_SERVICE_FAULTS=isolated \
CHRONO_SERVER_RATE_HEALTH_PER_MINUTE=10000 \
RUST_MIN_STACK=33554432 \
cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p cma --bin cma \
  worker_supervision::service_fault_tests::whole_service_worker_failures_recover_over_http \
  -- --exact --ignored --nocapture --test-threads=1
```

The elevated local health sampling budget prevents the application's enabled
rate limiter from rejecting high-frequency test probes; it is not a production
configuration change. The first harness run hit the default health limit with
429 and is retained separately. Final results and raw samples are in
`/tmp/cma-1740-evidence/service-worker-faults-final/`; the focused suite, exact
commands, source SHA and executable hash are in `final-pinned/` beside it.

Worker failure and recovery do not require Kubernetes to induce or repair the
fault. The loopback provider fixture only satisfies application composition;
no cluster operation or production context was used. This service test admits
no business executions. It does not measure confirmed durable work progress or
preservation of a real runtime/home/conversation; the separate deployment and
provider-journey qualification requirements above remain open.

## Ordinary service binary re-verification after synchronization

Built the ordinary `cma` executable from the same source `ae92da71f` with
`cargo +1.96.1 build --locked --manifest-path api/Cargo.toml -p cma --bin cma`.
Its SHA-256 is
`68c6781cab3915edfd027f5c120f940a8811cf8bac72ee6461ba0fa4bf048520`.
This executable contains no worker fault-injection hooks. It uses the isolated
local configuration above, including verified provider TLS and the current
exact scoped runtime Mongo role; no application authority checks were bypassed.

| Observation | Final ordinary binary result |
| --- | --- |
| Startup and reads | `/ready`, `/livez`, `/api/v1/sandboxes`, `/api/v1/conformance` all returned 200 |
| Idle scheduling | All seven worker heartbeats advanced over 15 seconds; worker restarts stayed zero |
| Mongo interruption | SIGSTOP of disposable Mongo for 5.107 seconds; readiness 503 with reason `store` after 2.552 seconds; liveness stayed 200 |
| Mongo recovery | Same CMA PID 24065 returned to readiness 200 after 0.511 seconds; Sandbox reads succeeded; no worker restarted |
| Intentional shutdown | Separately owned PID 24030 received SIGTERM, exited 0 after 0.009 seconds, and closed its HTTP listener; all seven workers logged `stopped`, with no drain deadline warning |
| Final local state | PID 24065 remains running on `http://127.0.0.1:58483`, metrics on port 58484; all seven workers running |

The shutdown run used an empty execution database and did not exercise draining
real business work. Retained-child shutdown behavior is covered separately by
the focused regressions. These local observations do not qualify production
images or real provider/runtime execution. Final ordinary service logs, metric
snapshots, HTTP samples, commands, source/binary identity and shutdown results
are in `/tmp/cma-1740-evidence/service-startup-final/`. The preceding
pre-synchronization process results are retained as historical evidence only.
