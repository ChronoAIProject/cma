API #2056: **PASS — accepted by the binding LEAD ruling of 15 November 2026, 00:41 SGT**, relayed by debugger-02. The active acceptance uses stable corrected total RSS and anonymous memory (musl anonymous slope **−0.014584 MiB/min**) versus positive Zig anonymous-memory growth (**+0.994493 MiB/min, R² 0.970554**). This is a dated acceptance decision on the existing completed run; source, images, raw measurements and their hashes are unchanged. The original 23:55 total-RSS-only rule produced **NOT_PASS / NONREPRODUCTION** and is preserved below as **superseded**, not reclassified as having passed its original criterion. The observed production regression began with batch 5, the first successfully shipped Zig API build.

The smallest proposed fix remains the already-built official Dockerfile image **9r, `8a09ee166cd7ec8a850fadef00b5beae8d62aa742e6080644f7e7c54813a2540`**, at unchanged application source `67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6`. Use `infra/api/Dockerfile`: `rust:1.96.1-alpine`, `musl-dev`, ordinary `cargo build --release --locked --bin cma`, with the existing runtime stages. The correction changes the build route without an application refactor or memory-limit increase. The active memory qualification is PASS under the dated lead ruling above. Deployment02 owns publication and rollout; this report grants neither and claims no production deployment.

| Completed local measurement | Official 9r / musl | Shipped 322f / Zig |
|---|---:|---:|
| Full-window RSS slope, MiB/min | -0.111378 | -0.196399 |
| Full-window RSS R² | 0.432822 | 0.014334 |
| Full-window anonymous RSS slope, MiB/min | -0.014584 | 0.994493 |
| RSS initial / peak / end, MiB | 126.449 / 127.715 / 124.145 | 164.684 / 184.367 / 153.754 |
| Anonymous RSS initial / peak / end, MiB | 57.957 / 58.910 / 58.113 | 90.137 / 110.980 / 110.980 |
| After process age 10 min: RSS slope, MiB/min | -0.318809 | -2.973008 |
| After first loop minute: RSS slope, MiB/min | -0.122887 | -0.353342 |
| Completed / planned requests | 1200 / 1200 | 1200 / 1200 |
| Failed / not offered requests | 0 / 0 | 0 / 0 |
| Full-loop RSS samples | 41 | 41 |
| Final quiescent RSS / anonymous RSS, MiB | 124.172 / 58.141 | 153.789 / 111.016 |

**Active acceptance, 00:41 SGT:** the lead accepted the anonymous-memory comparison together with the corrected image's full-window RSS slope below 1 MiB/min. The paired memory verdict is **PASS**. The [scorecard](QUALIFICATION-20261115.json) records this ruling and the exact unrounded measurements.

**Superseded historical result under the 23:55 rule:** the corrected image passed its standalone RSS limit, while the old image did not show clear positive full-window total RSS: slope −0.196399 MiB/min (R² 0.014334), initial 164.684 → peak 184.367 → final 153.754 MiB. At 00:33:38 SGT, total RSS fell 31.664 MiB while anonymous RSS increased 0.109 MiB, with stable process identity. The resulting original disposition was NOT_PASS / NONREPRODUCTION. That historical calculation remains correct for its original criterion; the 00:41 lead ruling supersedes its active gating role. Every sample and the full 20-minute window are retained. These are descriptive regressions on serial samples, not independent-sample confidence intervals or an extrapolated OOM forecast.

The synchronized loop ran **2026-11-15T00:16:08.156726+08:00 → 2026-11-15T00:36:08.334237+08:00**, actual 1200.170 seconds. Original UTC bounds: `2026-11-14T16:16:08.156726+00:00` → `2026-11-14T16:36:08.334237+00:00`. Each image received one GET per second, 400 each to `/livez`, `/ready`, and `/openapi.json`, with no authorization header or cookie. The sampler recorded RSS, RssAnon, process identity, CPU affinity, threads and task/stream gauges every 30 seconds, followed by at least 12 seconds of no load. One startup polling failure for musl and two for Zig preceded HTTP readiness; startup attempts are retained in full. The measured window began after both were ready and the explicit 12-second metric-settling wait.

Both APIs used separate newly created empty databases in one disposable local Mongo replica set. The APIs had identical 2-CPU quota, 2-GiB memory/swap cap, CPU affinity 0–7, eight Tokio workers, 256 PID limit, log configuration and application settings. Only database namespace/authSource and published loopback ports differed; `configuration-comparison.json` verifies equality without disclosing secrets. Mongo had 0.5 CPU and 768 MiB. Native warm/prepared targets were zero. A local read-only synthetic Kubernetes responder supplied empty runtime/PVC lists for mandatory startup preflight; it provisioned no runtime/provider/IdP. These caps and empty data are not production parity.

Full-loop task ranges were musl [46.0, 47.0] and Zig [46.0, 48.0]; thread ranges were [10, 11] and [10, 11]. Final task counts were 47/46; final active public streams and HTTP in-flight gauges were zero for both. Process starts, image identity, affinity and worker count stayed stable. This rules out growing observed task/stream counts in this loop; it does not directly measure every live heap allocation.

The shared host was not reserved exclusively. The EG native soak overlapped the beginning and was reported released at 00:18:46.867761 SGT. The lead authorized the runtime build lane at 00:17; actual compile timing was not independently collected by this API sampler. The overlap is retained in `measurement-context.json`, and no API caps, requests, thresholds or window were changed. This small loop shows positive anonymous-memory growth in the Zig control and stable anonymous memory in musl. A sharp late fall in total RSS changes the full-window comparison. No samples are removed. The lead explicitly revised the acceptance basis at 00:41 SGT; no new numerical threshold was invented by the harness. No 12–25 MiB/min production-rate reproduction or exclusive resource window is claimed.

Production history, read once by log01 and independently fitted by developer12, corroborates the build boundary:

| Image | Successful build route | RSS slope after actual process age 10 min, MiB/min |
|---|---|---:|
| Last CI `859264c2`, source `ade94de34` | Original Alpine Dockerfile / CI 43015079026 | 0.000066 |
| B4 `dc8548c4`, source `b0129514a` | Original Dockerfile, emulated amd64 | 0.094 |
| B5 `3737903a`, source `a85fb1b98` | First shipped Zig | 22.318 / 23.326, separate processes |
| B6 `8f949944` | Zig | 16.460 |
| B7b `e6ead60b` | Zig | 12.192 |
| B8b `4c5497b9` | Zig | 12.884 |
| B9 `322f08a1`, attempt 3 | Zig | 24.527 |

The first B5 process started 14 Nov 00:15:04 SGT. The predecessor CI process ran 1,591 retained minutes and peaked at 140.516 MiB. B4's failed native packaging was never pushed; its shipped fallback used the original Dockerfile. Published variants without a retained metric lifetime remain unobserved. The evidence covers 18 digests and 39 separate process/gap segments; no restarts or Pod lifetimes were pooled. Full historical bounds and route receipts remain in log01's `FIRST-FINDINGS.md`, `image-rss-build-join.json` and deployment02's `image-build-inventory.json`.

The exact build difference is Rust 1.96.1 with cargo-zigbuild 0.24.1 / Zig 0.16.0 versus the official Alpine/Cargo route. Target name `x86_64-unknown-linux-musl` alone does not identify the allocator: the shipped B8/B9 ELF functions match Zig's SmpAllocator. Its small-allocation slabs and per-thread free lists retain memory under cross-thread allocation/free churn. Developer15's independent equal-work 4,096-round probe drops all payloads at every sample boundary yet grows from 11,664 to 47,556 KiB with Zig, versus 4,004 to 3,956 KiB with musl. That directly supports retention after free. Finite workloads may plateau; indefinite unbounded growth is not claimed. Installed Alpine musl package version is not equated with the statically linked libc archive.

The B4→B5 source audit found usage reports/warmer, sandbox metrics, ledger reads, telemetry, hosted relay/RPC and live-agent source unchanged. New discovery caches have explicit entry/TTL/byte caps. Audited stream channels and registries have bounded/coalesced/weak lifetimes. Conditional legacy relay backlog, pending RPC senders and metadata watermarks remain older hypotheses without onset evidence; no speculative application patch was made.

Exact source/image bindings:

- Application source for both: `67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6`.
- Corrected manifest/Docker ID: `sha256:8a09ee166cd7ec8a850fadef00b5beae8d62aa742e6080644f7e7c54813a2540`; config: `sha256:2a8c21da475b6b3d9d4518dca3b865e88db03697bffdb22e602a1b802e1d6686`.
- Old manifest/Docker ID: `sha256:322f08a15db3026560193e22eae6c590549366c2758edae2dff9b22be8ffa9e8`; config: `sha256:599ca9467f6813bcdf7146f5dc173787fe2aaaa5e672e1bb85f75124e86e3c33`.
- Corrected executable SHA256: `ce61a24eeb6191bc61de1935b53ae66afea96f83033e82804220b6a48b750f78`; builder output equals runtime bytes per deployment02's retained static check.
- Harness: `549fc1aa9bc449dbc3459ffb883fc392d742ea17`, branch `tl/cma-api-memory/fixture-67b`; 13 focused Python checks PASS, `git diff --check` PASS. No new application compiler or product-code change was needed for the light campaign.

Evidence root: `/Users/shining/code/work/.tl-worktrees/cma-cma-reconcile/api/target/api-memory-rca/light-parallel-9`. `measurement.json` is the runner's result; `qualification-analysis.json` independently recalculates RSS/anonymous fits, request denominators, sample coverage and gauge ranges, and hashes the raw files. Its active verdict and dated ruling are also retained in the source [scorecard](QUALIFICATION-20261115.json); the original verdict is preserved in `historical_dispositions`. All 13 recorded raw-evidence hashes were revalidated unchanged when applying the ruling. `musl/` and `zig/` retain `samples.jsonl`, `requests.jsonl` and every `startup.json` attempt. Private configuration and logs are not included in this report. Automatic owned cleanup finished at **2026-11-15T00:36:21.175724+08:00**; the separate `cleanup-verification.json` records readback of absence for the owned containers/network.

The 00:30 target was missed: the complete loop finished after 00:36 because eight retained setup/observer attempts preceded the successful run. Those attempts are not counted as API RSS failures. Two optional typed-history exporter attempts hit the 6-GiB guard and generated no dataset. The lead removed that dependency at 23:55; representative finite history, stream churn, accepted provider work and production soak remain unqualified by this light loop. The corrected route and completed local result are available before the original 01:30 RCA deadline.
