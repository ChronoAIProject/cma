# Local desktop replica qualification

The dedicated `.github/scripts/desktop-replica-contract.py` runner joins the public desktop router and real auth middleware to independent Mongo clients, separate concrete Codex/Hosted/Provider adapters, and a subprocess running the actual authenticated runtime receiver. The local identity provider, provider proxy and echo display are test fixtures. Browser OAuth, browser pixels, production provider behavior, native Kubernetes transport and the Linux desktop launcher require their own qualification.

Supply an owned disposable Mongo replica set on a loopback address through `CHRONO_TEST_MONGODB_URI`. Use the normal local build slot when required by the development environment. For example, from the repository root:

```sh
python3 .github/scripts/desktop-replica-contract.py --evidence /absolute/new/desktop-contracts
```

The output directory must be new. The runner uses `backend-nextest.sh`, including its source/archive boundary checks. It lists the exact parent tests and the ignored runtime child, binds their compiled binary hashes, then runs only the parents serially. The parent passes the child a random runtime credential through stdin; readiness contains no credential. Every receiver root is beneath the dedicated output directory. Successful parents close and await their exact child processes and remove their roots. A failed run retains its logs and remaining roots for inspection; it must not be described as successful cleanup.

The correctness cases first require a never-activated legacy connection to return an echo through the concrete provider transport without creating an ownership floor. They also cover two Agents in one Sandbox reached through different authenticated public ingresses, one winning socket, healthy renewal beyond the original receiver deadline, handoff, provider refusal, logout, confirmation acknowledgement loss, receiver process restart with a retained floor, old-owner/legacy refusal, uncertain revocation, and a delayed renewal acknowledgement. The production current-credential and lineage checks remain active. The proxy authenticates the concrete provider credential before translating it to the receiver's `x-cma-runtime-token`; the actual receiver enforces ownership and its persistent floor.

The existing modeled service/router/process contracts remain in the ordinary Mongo selection. These additional parent fixtures have exact entries in `mongo-contract-exceptions.json` because they also require the compiled runtime test binary. The dedicated runner selects each explicitly. The CMA fixture parents launch `link::desktop::tests::child_fixture::desktop_receiver_child`; the ignored runtime child is not another standalone passing test.

## Matched connection windows

The same runner supports `--mode legacy`, `--mode one` and `--mode two`. Each execution creates new resource/auth state and a fresh receiver root. Never reuse an activated root for the legacy arm: its durable floor must continue refusing legacy access, including after restart or replica mode is disabled. The owned native Direct configuration is unsupported and has no legacy fallback; this fixture exercises the hosted/provider-proxy path.

Each mode records three windows with five retained warmup attempts and 100 measured attempts per window. Timing begins before authenticated discovery, consumes the descriptor, establishes the public WebSocket and receives the exact synthetic echo frame. Session close and observation of claim release happen outside that interval. The first connection is separately marked cold for the fresh auth/transport objects; fixture seeding may already have warmed Mongo records.

Collect the three legacy windows and seal their raw hashes and a repeatability-derived noise allowance before running either owned mode. Keep errors, refusals and missing completions in their denominators. Fix the quantile and confidence-interval method before candidate inspection, report p50/p95/N/max and errors, and do not report p99 from only 300 measured attempts. An unstable baseline is inconclusive. A new fixture comparison does not overwrite an earlier regression or establish equivalence to an approved browser or production baseline.

`--diagnostics` is allowed only with a measurement mode and uses a smaller, separate cohort. It enables existing scalar desktop phase tracing, observes the existing ambient auth resolver histogram separately for discovery and WebSocket establishment, and enables numeric local-IdP handler observations. Handler timings exclude response serialization, HTTP transport and CMA verification. Diagnostic instrumentation contributes overhead; those observations are not timing-acceptance samples, and phase percentiles must not be added together.

The final-source OFF path also includes receiver custody/task registration, the legacy stream cap and per-frame cancellation. A matched OFF/ON result therefore does not prove equivalence to the pre-scale receiver. Exact source review, actual images/providers, separate CMA processes, abrupt owner loss, rollout/rollback and client-visible performance remain separate acceptance boundaries.
