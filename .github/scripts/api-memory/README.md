The original 23:55 SGT qualification used `light.py`: two actual API images run side by side for 20 minutes against separate fresh empty databases. Each receives exactly one request per second, rotating `/livez`, `/ready`, and `/openapi.json` (400 requests per route, 1,200 per image). RSS, anonymous RSS and task/stream gauges are sampled every 30 seconds. The corrected image must have a full-window slope below 1 MiB/min **and** the old image must show clear positive growth. A flat old control is a negative reproduction, not a paired PASS.

The completed run's **active memory verdict is PASS under the binding LEAD ruling of 15 November 2026, 00:41 SGT**, using stable corrected RSS and the anonymous-memory comparison (musl −0.014584 versus Zig +0.994493 MiB/min, Zig R² 0.970554). The original total-RSS-only NOT_PASS/NONREPRODUCTION result is preserved as superseded. See [the dated report](QUALIFICATION-20261115.md) and [scorecard](QUALIFICATION-20261115.json). This acceptance update changes no raw measurements or harness workload and requests no new run.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 .github/scripts/api-memory/light.py \
  api/target/api-memory-rca/light-parallel-new
```

Run only in the owner's allocated campaign window. Both fixed image manifest/config identities are verified from local Docker descriptors. The APIs use identical CPU affinity 0–7, two-CPU quota, 2 GiB memory, eight Tokio workers, logging and application settings; database names and host ports differ. No authentication header or cookie is sent. Startup requires the exact generated Mongo role for each synthetic database principal, an HTTPS public-origin setting, native home/pool configuration with every target set to zero, and a synthetic Kubernetes endpoint returning empty namespaced runtime/PVC lists. That responder runs inside the owned Mongo container, refuses all writes, and provisions no runtime resources. The normal Docker bridge is required for published loopback ports; `--internal` suppresses those bindings on this local engine. All published ports bind to 127.0.0.1.

The actual qualified-window attempt began at 2026-11-14T16:16:08.156726Z, retained under `light-parallel-9`. Earlier startup/setup attempts remain separate; none is treated as an API memory failure. The observer accepts Docker nanosecond timestamps under Python 3.9 by truncating only the process-age calculation to microseconds, while retaining the original Docker timestamp. Startup exceptions retain file/line/function without private argument values. All requested work and sample failures remain in the result; failure of one image during the loop does not cancel the other's observation.

The finite-history workflow below is optional follow-up depth. Its two offline exporter build attempts reached the 6 GiB guard and produced no fixture. It is not a prerequisite for the binding light-request qualification, and no further exporter retry belongs to the active campaign.

This incident-only harness compares the shipped Zig API image with the corrected official Alpine image at application source `67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6`. It changes no application production behavior. It requires the incident owner's scheduled local resource window. Preparing these files does not start that window.

The offline exporter is a single ignored Rust test using the production BSON codecs and semantic history validator. It writes one private fixture: one stopped Agent, its parent/workspace/worktree, and 515 durable events representing 128 completed turns. It starts no Mongo connection, API, provider or runtime. The fixture source is deliberately an incident helper, not a proposed general CI Mongo test.

After the image builder releases NORMAL3, the scheduled exporter command is:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 .github/scripts/api-memory/export.py
```

It builds in the owned `api/target/api-memory-export` cache clone, runs only the named exporter with Rust 1.96.1, and caps the retry at one job, 900 seconds and 6 GiB aggregate process-group RSS, with debug symbols disabled only for the CMA test executable. Attempt 1 exceeded the same 6 GiB bound and produced no fixture; its result and private log are retained separately. `api/target/api-memory-rca/export-result.json` records actual times and disposition; the compile log and generated fixture stay in its private subdirectory. Cargo revalidates the copied cache against exact 67b source plus the fixture.

After the exporter succeeds and the EG service cohort releases the runtime window, the owner runs:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 .github/scripts/api-memory/pair.py \
  api/target/api-memory-rca/private/fixture.json \
  api/target/api-memory-rca/paired-run
```

The order was declared before runtime: corrected `cma-api-batch9r-official:67b3fbc5df5f` first, shipped `cma-api-batch9-native:amd64-67b3fbc5df5f` second. This protects the corrected-image qualification deadline. Both receive the complete identical workload even if the other fails. No workload is intensified after observing results.

Each variant receives a fresh database containing the identical fixture. The fixed workload is 20 rounds paced at 60 seconds, each with 16 complete history reads, 16 follow/read-first-event/disconnect requests and two readiness/OpenAPI controls. Four clients run concurrently. All client futures join, sockets close, and at least 12 seconds pass before checking zero active streams and in-flight requests. A round that cannot leave that quiet interval fails instead of silently extending the pacing. Initial and final idle windows are 60 seconds each. Dataset head, counts and absence of pending work are checked before and after.

Both APIs use the same CPU affinity `0-7`, CPU quota 2, memory cap 2 GiB, eight Tokio workers and `RUST_LOG=info`. The one Mongo replica set uses 0.5 CPU and 768 MiB. The actual affinity, worker gauge, Docker caps, image identity, process start and restart count are checked. These local caps are not production parity. The Docker network is internal and ports bind only to loopback: API 18980, metrics 18991, Mongo 18917. Trusted synthetic authentication permits stored-history reads without provisioning an Agent or a runtime. The setup has no usable external provider or IdP. Cleanup removes only resources carrying the exact run's ownership label.

The local containerd image store reports the manifest as Docker `.Id`; that field is recorded as `docker_image_id`, not described as a config digest. The old image's exact manifest `322f08a15db3026560193e22eae6c590549366c2758edae2dff9b22be8ffa9e8` and config `599ca9467f6813bcdf7146f5dc173787fe2aaaa5e672e1bb85f75124e86e3c33` are independently asserted from its local descriptor. No registry reads are needed.

Artifacts retain startup attempts and RSS, original sample timestamps, RSS/anonymous RSS/thread counts, task/stream/request gauges, actual offered/completed/failed/not-offered counts and process identity. Fits report the complete 20-minute loop, all observations, the loop excluding its first minute, observations after actual process age ten minutes, quiescent boundaries and final idle. The required corrected-image gate is strictly **less than 1 MiB/min over the complete 20-minute loop**; a partial run never passes. Subsequent windows are diagnostics and cannot replace that gate.

A passing corrected image alone does not prove a causal actual-API reproduction. If the old control also stays flat, preserve the negative reproduction and distinguish it from the separately retained equal-work, zero-live-payload allocator probe. A finite history can reach an allocator high-water mark. This harness does not qualify create latency, provider work or deployed traffic.

Preparation checks (no API or Mongo process):

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s .github/scripts/api-memory -p 'test_*.py' -v
cargo +1.96.1 fmt --manifest-path api/Cargo.toml --all -- --check
git diff --check
```
