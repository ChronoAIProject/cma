The 15 November 01:03 lead task uses `allocator_pair.py` for the single debugger02-owned local API comparison. Developer12 owns product PR/issue #2058; deployment02 owns official Alpine images. This harness is separate from the allocator-only product PR and starts no compiler. Its fixed source bindings are control `67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6`, allocator patch `1075a8c1374e878adcf168c8cd135f9c0fe9d5eb`, and candidate `dfc2e7ba55b337b169ec9de749224d95cc6a6631` (67b plus only that patch). Candidate lock SHA256 is `46e5cef4a4a4af47c953e421e4242af115935268fd0c6694ae9c1f4ac20f722c`.

The execution owner supplies a JSON binding file after the actual image handoff. Its exact nonsecret fields are:

- `application_base`: the full 67b source above.
- `allocator_commit`: the full 1075 allocator patch above.
- `candidate.source`: the full dfc2 composed source above.
- `candidate.docker_image_id`: the actual local Docker image ID, `sha256:` plus all 64 hex digits.
- `candidate.manifest`: the builder's full manifest digest.
- `candidate.config`: the builder's full config digest.
- `resource_context`: the scheduler's actual resource/overlap context, retained verbatim as nonsecret metadata.

The image inspector verifies these identities independently, including the OCI source label and amd64 architecture. Docker `.Id` can be the manifest on the local containerd store; it is never assumed to equal the config digest. The old local tag, manifest `322f08a15db3026560193e22eae6c590549366c2758edae2dff9b22be8ffa9e8`, and config `599ca9467f6813bcdf7146f5dc173787fe2aaaa5e672e1bb85f75124e86e3c33` retain their existing assertions.

From any directory, using absolute paths:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  /Users/shining/code/work/.tl-worktrees/cma-cma-reconcile/.github/scripts/api-memory/allocator_pair.py \
  /absolute/path/to/owner-image-bindings.json \
  /absolute/path/to/new-owner-evidence-directory
```

The output directory must not already exist. No image pull occurs. Both APIs run side by side, each at two-CPU quota, CPU affinity 0–7, 2 GiB memory/swap cap, eight Tokio workers, PID cap 256 and identical logging/configuration. One Mongo replica set has 0.5 CPU/768 MiB and a separate fresh database per variant. Loopback ports are candidate API18980/metrics18991, control API18981/metrics18992 and Mongo18917. Existing role generation, trusted synthetic identity, HTTPS public-origin metadata, native-home zero-pool settings, empty local Kubernetes responder, Docker bridge and Python3.9 nanosecond timestamp corrections are reused from the completed 9r campaign.

The predeclared mix uniformly rotates `/livez`, `/ready`, `/openapi.json`, `/api/v1/sandboxes/sbx_allocator_read`, and `/api/v1/workspaces/wks_allocator_read`, at one GET/second/image for 1,200 seconds: **240 requests per route per image**. Monotonic durations cover connection setup through complete body read/close; Python identity validation happens after the latency timestamp. Percentiles use nearest rank `ceil(p*N)`, with both per-route and aggregate p50/p95 retained. Failed requests retain type and duration without private exception text. All planned/attempted/completed/failed/not-attempted/late counts remain visible; a failed or incomplete pair cannot pass based on a filtered successful subset. The loop stops offering at the full-window boundary; work finishing outside that window cannot qualify.

The seed follows exact67b Sandbox/Workspace serde shapes and Mongo envelope projections, with one additional durable provider-choice document. It has one failed dormant Sandbox with an explicit synthetic failure, work step none and no runtime/provider credential; one ready Workspace with work step none; and zero Agents, worktrees or history events. The failed state avoids both active work and the paused state's required checkpoint/home. BSON envelope revisions and counters use explicit Long values because the production reader requires them. The generation-1 `runtime_backend_routes` row selects `agent-sandbox` for deployment `cma-memory-rca`; its key uses the exact source SHA256 domain separator `cma-provider-effect-v1` followed by a literal NUL and compact JSON tuple. This records a provider choice without creating a runtime. Startup creates its own migration marker; the seed does not forge that marker. Pre/post census requires the same provider-choice row plus the normal startup marker. This repairs campaign-1’s concrete pre-HTTP retained-provider-choice refusal. The corrected fixture SHA256 is `afb8b2826df75cd9494d16370ded44ae98f872d1ceebbc136e183f5706667396`. This is a source-derived fixture, not a claimed Rust-codec export. **Actual HTTP200 decoder/identity/hierarchy checks are mandatory before and after measurement.** No404/401 fallback or reduced route mix is permitted. The pre/post Mongo census requires unchanged counts/status/revisions, no runtime/credentials and no pending work. Optional palette/read bookkeeping may occur normally; no lifecycle/provider operation is requested. The fixture does not represent active runtime/provider traffic.

Memory samples include process RSS, anonymous RSS, process identity, affinity, threads and task/stream gauges: initial plus every30s through1200s =41 full-loop samples, then at least12s of quiescence. Full-window anonymous OLS must be **strictly below1 MiB/min**. Candidate aggregate p50 and p95 must each be **at most1.10×** the matched Zig control. Per-route percentiles remain diagnostic. Startup and after-process-age10min memory windows remain separate. Every request failure, observation error or fixture failure prevents PASS; no segment is discarded.

Only the initial and quiescent observations add swap diagnostics: `VmSwap` in KiB from the already-read process status, plus `memory.swap.current` and `memory.swap.max` in bytes from one bounded Docker exec per observation. Unsupported fields are explicitly `UNAVAILABLE`; an unlimited cgroup cap is recorded as `max`. The diagnostic duration is retained separately and occurs outside HTTP latency timing. These fields verify actual process swap and the cgroup swap cap without adding a recurring collector or changing the acceptance thresholds. Environment02 separately records VM-wide memory/swap context.

`measurement.json` retains raw measurement outcomes and gates, with verdict pending postflight. `qualification.json` is final only after the campaign's fixture postflight and owned cleanup complete. `campaign.json` retains bindings, startup/config/fixture state, failures and cleanup. Raw request/sample journals stay per variant. Private settings and service logs stay in `private/`, with no payloads or synthetic subject in published artifacts. The runner removes only its labeled containers/network; debugger02 verifies actual release and independently recomputes the result.

Preparation checks:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s .github/scripts/api-memory -p 'test_*.py' -v
git diff --check
```

These checks exercise the harness only. Successful official musl linkage, final Rust allocation entrypoint binding to `mi_*`, real fixture HTTP checks, full memory/latency results and EG sandbox/media gates require their respective owners' actual evidence.
