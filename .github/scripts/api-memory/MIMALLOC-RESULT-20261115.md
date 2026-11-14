**API9m local qualification: PASS**, completed 15 November 2026 before the 02:30 SGT deadline. Debugger02 executed the sole paired campaign and independently recomputed memory fits and latency percentiles from the complete raw data. Developer01 verified all 18 hashes in the final handoff. This result qualifies the measured local workload; deployment remains under the lead and deployment owner.

[PR #2060](https://github.com/ChronoAIProject/cma/pull/2060), closing #2058, merged at 01:29:55 SGT as `14a1a7ec3`. It added mimalloc 0.1.52 / libmimalloc-sys 0.1.49 with default features, secure and C-symbol override off, and a global allocator in both binary roots. The official API image uses exact batch-9 `67b3fbc5d` plus allocator commit `1075a8c13`, composed source `dfc2e7ba55`; its original Alpine/musl locked build passed. Final ELF inspection proved all four Rust allocation entry points reach their corresponding mimalloc implementations.

EG mimalloc failed the mandatory x86 sandbox check with an allocation abort under the unchanged 384 MiB address-space limit. The lead selected the previously qualified original-musl EG image `67d1defc107395c4738977d6f495acc9df59b12071e753881c211dff31d3a063`. [PR #2062](https://github.com/ChronoAIProject/cma/pull/2062), closing #2061, merged at 01:53:49 SGT as `6b9a3a418`; it restores only EG’s original allocator and dependency edge. API mimalloc and its qualification artifact are unchanged. No sandbox limit or syscall filter was weakened.

Actual paired loop: **01:59:59.896978–02:20:00.111976 SGT**, 1,200.208242 seconds. Both variants completed **1,200/1,200 HTTP200 requests**, 240 per route, with zero failures, omissions or pacing delays over one second. All 41 loop samples and one quiescent sample are retained. The resources were identical: two CPUs, 2 GiB memory, zero additional swap, CPU affinity 0–7, eight Tokio workers and PID cap 256 per API; one Mongo replica set used 0.5 CPU/768 MiB and separate fresh databases. All 32 API settings matched except the database name and URI authentication source.

| Image / memory | Initial MiB | Peak MiB | Final loop MiB | OLS MiB/min | R² |
|---|---:|---:|---:|---:|---:|
| mimalloc / anonymous | 104.488 | 115.219 | 107.840 | -0.254749 | 0.137722 |
| mimalloc / total RSS | 176.023 | 187.066 | 179.688 | -0.252571 | 0.135133 |
| zig / anonymous | 103.637 | 127.543 | 127.543 | 1.123422 | 0.944039 |
| zig / total RSS | 180.367 | 204.961 | 204.961 | 1.137034 | 0.941496 |

The candidate passes the predeclared full-window anonymous slope <1 MiB/min gate. Initial/final process VmSwap, cgroup swap.current and swap.max were zero for both images. No samples were discarded; the full-window and after-process-age-10-minute fits remain in the machine-readable result. The candidate’s negative OLS slope is not a claim of monotonic decline.

| Route population | Requests per image | Mimalloc p50 / p95 ms | Zig p50 / p95 ms | Candidate/control p50 / p95 |
|---|---:|---:|---:|---:|
| All five routes | 1200 | 6.447 / 12.406 | 6.672 / 12.679 | 0.9663 / 0.9785 |
| /livez | 240 | 2.056 / 5.800 | 2.114 / 6.792 | 0.9727 / 0.8540 |
| /ready | 240 | 1.808 / 3.335 | 1.885 / 3.429 | 0.9594 / 0.9725 |
| /openapi.json | 240 | 6.999 / 11.593 | 7.022 / 11.772 | 0.9967 / 0.9848 |
| /api/v1/sandboxes/sbx_allocator_read | 240 | 9.980 / 14.787 | 10.077 / 15.345 | 0.9904 / 0.9636 |
| /api/v1/workspaces/wks_allocator_read | 240 | 7.708 / 10.680 | 7.772 / 11.328 | 0.9918 / 0.9428 |

Both aggregate percentile ratios pass the predeclared <=1.10 gate. Latency covers the full HTTP body using a monotonic clock and nearest-rank percentiles. Route-specific values are diagnostic; none substitute for the aggregate gate.

The two domain records are a dormant FAILED Sandbox and READY Workspace with a durable generation-1 provider choice. Actual HTTP200 identity/hierarchy checks and the complete state/routing census matched before and after measurement; the retained pre/post evidence files have the same hash. There were no Agents, worktrees, history records, runtime bindings, provider credentials or pending work. Campaign-1 is preserved separately as INCOMPLETE before HTTP readiness: its missing provider-choice fixture record was repaired in harness `27b6bf602`, with no image/config/workload/threshold change. Corrected fixture SHA256: `afb8b2826df75cd9494d16370ded44ae98f872d1ceebbc136e183f5706667396`.

Quiescence, postflight and owned cleanup completed at **02:20:13.651503 SGT**. Both processes retained their identity, final streams and HTTP in-flight counts were zero, and independent 02:20:33.137775 readback found zero owned containers/networks, all five loopback ports closed, no supervisor and no slot-2 lock holder. NORMAL2 was released.

Native validation: strict binary Clippy and dependency audit passed; the actual cma and cmaeg allocator test harnesses each passed 2/2 after the bounded same-cache debug0 continuation. Supervisor duration was 450.8 seconds and peak cma compiler-family RSS was 11,255,872 KiB. Earlier compiler memory-guard stops remain retained; native tests do not override EG’s mandatory sandbox failure. The final Python qualification harness passed 22 focused offline checks.

Exact candidate manifest/Docker ID: `sha256:ed9ccc89aa766fa7abfa272ff57c83d813a9e9cd66d5e0d20e7dc90fd359e848`; config: `sha256:e39a42383f4cc20445ad9c5b518489ff7a55762424871e3e7b23087444944d6c`. Exact control manifest/Docker ID: `sha256:322f08a15db3026560193e22eae6c590549366c2758edae2dff9b22be8ffa9e8`; config: `sha256:599ca9467f6813bcdf7146f5dc173787fe2aaaa5e672e1bb85f75124e86e3c33`.

The local mix includes successful Sandbox and Workspace reads but no active runtime/provider operations. It does not establish production-load latency or leak freedom over an unlimited lifetime. Production memory and latency checkpoints remain applicable. This report records no registry publication or production rollout.

Canonical execution/analysis packet: `/Users/shining/code/work/tech-lead.staff/debugger-02/api-mimalloc-20261115/HANDOFF.json` (SHA256 `3a09da1c087e2d38b7af36b4031c0a1d598823c2a2423c0298b71b7a323ec2d0`). Adjacent `qualification-analysis.json` retains complete fits, route values and raw evidence hashes. This report’s [machine-readable companion](MIMALLOC-RESULT-20261115.json) preserves the exact bindings, measured results, cleanup and verification of all 18 referenced evidence files.
