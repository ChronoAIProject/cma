# EG cohort lifecycle evidence

This Python standard-library component prepares one N1 or N2 cohort for the
developer-08 admission driver. It owns no requests, fault injection, credentials,
cluster mutation, deployment, or disposal. Its output requires independent review;
it never emits application acceptance PASS. Issue #1957; EG closure #1966.

`lifecycle.py` records the agreed ordered phases: prepare, workload, terminal_wait,
capture, workload_shutdown, resource_shutdown, child_wait, local_destroy, cleanup,
transition. Every start/end is synchronously flushed and fsynced. One run/cohort
identity and one monotonic origin bind the sequence. A crash leaves an unmatched
start. Phase budgets detect late returns; the caller must enforce cancellation
and the actual operation deadline. This recorder does not implement a watchdog.

`capture.py` is a bounded, read-only Kubernetes capture CLI. It pins the explicit
kubeconfig, local context, namespace UID and expected running image digest. It
reads the Deployment's actual selector and projects only Pod/runtime status.
Each kubectl call has a 10-second request timeout and a 15-second process timeout;
five sequential calls give at most 75 seconds of subprocess wait, plus local
parsing and filesystem work. This is not an application or recovery bound.

The CLI preserves a separate fsynced observation journal even on failed reads.
It stores hashes of successful projected output, never raw Pod names, addresses,
environment, credentials or error text. A namespace replacement invalidates the
capture. Captured container ID/start/restart identity is not process-exit proof.
Co-location is reported explicitly; readiness cannot pass placement or capacity.

Before real use, obtain exact-source tooling review and the owner image/config,
dedicated database/credentials, endpoints, controls and coordinated window. HTTP
JWKS/provider fixture access must satisfy the product's loopback rule through a
reviewed per-Pod proxy, or use trusted TLS. No URL-policy relaxation is supplied.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_lifecycle.py
python3 capture.py --plan /task/private/plan.json \
  --kubeconfig /owner/private/kubeconfig --kubectl /absolute/kubectl \
  --container gateway --out /task/evidence/cohort-before.json
```

The capture command is an interface example, not an executable approved window.
An existing output/journal is never overwritten. It creates no Pod or fixture.

The plan has schema `eg_lifecycle_plan_v1`, UUIDv4 `run_id` and `cohort_id`,
replicas 1 or 2, full Git `candidate_commit` and `candidate_tree`, SHA-256
`bindings` for application_image/namespace/config/fixture/tools/owner_window,
per-phase positive integer `budgets_ms` (at most 120000), mandatory per-phase
integer `return_bounds_ms` (at least that phase's budget, at most 120000), nullable
`expected_timeout_phase`, and `departing_instances` (container-instance hashes).
Both maps must be registered before execution. The difference is the explicit
timeout-enforcement/return allowance; there is no implicit or result-driven
tolerance. These bounds do not change the historical 120-second harness.
No WIP driver budget is supplied as a measured result.

The owner runner imports `Lifecycle`, creates a new evidence directory, calls
`start(phase)` before its real operation and `end(phase, status, receipt_sha256)`
after return. Status is ok/failed/timed_out. A timeout before its registered
budget is rejected. Every status exceeding its complete return bound becomes
late_return; ok/failed also remain late after the operation budget. Each end
retains observed_status separately, both bounds, duration and whether return met
the complete bound. A late timed_out observation cannot qualify COMPLETE even if
cleanup later succeeds. A failed workload must
still proceed through terminal accounting and custody checks. The run owner
controls the next action; this library does not start another cohort.

Before local_destroy, call `confirm_custody()` with the exact registered departure
set and independently proven exit set, collector/fixture close state and the
no-late-effect barrier. The receipt hash is fsynced before the disposal phase. A missing exit or barrier
blocks that phase.
Neither Pod deletion, fixture `/control/close`, a healthy successor nor a ready
snapshot supplies that proof. The artifact referenced by `receipt_sha256` must
establish it. Unconfirmed custody means retain the private fixture/database.

`finalize(accounting, custody)` retains distinct HTTP attempts and unique submitted
events. Submitted events partition into proven unadmitted, custody established
and admission uncertain. Custody established partitions into direct terminal,
reconciled terminal and unresolved. A reconciled terminal result appears once.
Terminal failures stay separate from success; expected/confirmed effects and
missing/duplicate/stale/wrong-owner counts remain visible. Any uncertainty or
unresolved custody prevents COMPLETE. The inner oracle owns per-event/target
and effect joins; aggregate arithmetic cannot establish their truth.

A registered controlled stall can produce registered_stall_observed, with normal
boundaries false. It requires a timeout between the registered operation and
return bounds, plus later confirmed
cleanup. Pure clock tests do not prove that the old fixture's post-marker Drop
or accumulated resource ownership follows this mechanism. Bind the actual owner
shutdown/transition implementation and failure-injection seam before claiming
N1/N2 timeout closure. Historical aadd212/c08 attribution remains unknown.

Validation is pure Python and mocked Kubernetes, with no application, Docker,
Mongo, actual child process, cluster request, load or production operation.
