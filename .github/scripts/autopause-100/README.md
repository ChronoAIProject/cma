# Auto-pause campaign core and native observer

This directory contains reusable campaign and lifecycle logic, an injectable
read-only native HTTP observer, and offline contract tests. Fixture custody,
a mutating execution adapter, launcher and cleanup remain the caller's
responsibility. Native qualification requires those components and an approved
disposable execution lane. Refs #1330 and #1547.

Run the offline tests from the repository root:

```sh
node --test --experimental-test-coverage .github/scripts/autopause-100/*.test.mjs
```

## Injected campaign adapter

Call `campaign(adapter, {count, record, now})`. `count` defaults to 100 and is
bounded to 1–100. `record` must durably store private, sanitized packets; a write
failure stops the campaign. `now` is an optional monotonic clock in milliseconds.
The core checks `adapter.origins.ams` and `adapter.origins.control_origin` for
HTTP loopback origins without credentials, paths, query strings or fragments
before invoking any adapter method. The adapter must use those exact endpoints,
reject redirects or destination changes, and bound every operation.
Offline tests check origin validation and propagate adapter validation refusals;
actual HTTP redirect rejection must be verified with the executing adapter.

| Method | Required behavior |
| --- | --- |
| `validate()` | Before setup, validate fixture authority, disposable scope, exact source/image bindings and endpoint ownership. Refuse an unavailable or unauthorized fixture. |
| `prepare()` | Record the original 900-second production policy, create only owned synthetic resources, await initial Ready/idle and return a baseline with original home, file, conversation and operation evidence. Register cleanup custody before effects. |
| `alwaysOnException(baseline)` | Prove ordinary-owner refusal, administrator-only activation, sustained exemption and normal policy restoration through supported interfaces. |
| `awaitAutomaticPause(baseline)` | Observe natural idle pause without manual pause/resume. Return a native snapshot: `operating_mode: Suspended`, positive current `generation`, empty `pods`, and a true Suspended condition for that generation. A paused label without actual resource release is insufficient. |
| `wakeWithMessage(baseline, index)` | Send one retained message identity through the supported public interface and wait for actual execution. Return the turn handle; do not replace a failed sample. |
| `home(baseline)` | Return original `sandbox_id`, `native_uid`, `home_identity` and exact nonempty `pvcs` evidence for comparison after wake. |
| `holdActiveTurn(baseline, turn)` | Observe an active turn across the applicable idle threshold without interruption. Return `interrupted: false`, positive `samples` and positive `hold_ms`, bound to independent runtime evidence. |
| `completeTurn(baseline, turn)` | Require one successful completion with the original conversation and exact receipt. Return sanitized receipt/hash evidence. |
| `verifyRetention(baseline, response)` | Prove original file bytes/hash, home, conversation, earlier responses and receipt identity remain intact. Throw on missing, changed or duplicated evidence. |
| `verifyAllReceipts(baseline)` | Recheck every retained receipt and history after all requested cycles. Failure must remain a final FAIL. |

The adapter owns assertions requiring application/network knowledge. Every
returned object must be sanitized evidence: no credentials, arbitrary provider
error bodies, customer content or private deployment identifiers. The core
records fixed stage error codes instead of exception messages. No caller boolean
substitutes for independently measured suspension, activity or retention proof.

The surrounding runner must use `try/finally` to clean up only owned synthetic
resources and restore policy, including uncertain setup attempts. Cleanup
failure prevents acceptance even if the core's cycle sample passed. Cleanup and
the mutating execution adapter remain outside this directory's implementation.

## Injected read-only native observer

`createNativeReader({origin, binding, timeoutMs, bearer, fetchImpl})` from
`native-reader.mjs` returns `readNativeSnapshot({signal})`. Supply an independently
verified original fixture binding; labels or newly discovered resources cannot
establish that authority. The reader pins Namespace, Sandbox and PVC identities,
the original PV, home and image digest. It supports one retained `home` PVC and
the runtime's home/state/transfer mounts, with an explicit `home_contract.kind`:

| Contract | Required storage and ownership |
| --- | --- |
| `legacy-detached-v1` | Detached PVC with original adoption metadata; explicit template volume; runtime home mounted at `/home/cma`. |
| `native-vct-empty-v1` | Original Claim assignment and Sandbox/PVC controller chain; retained VCT and initializer; runtime home mounted at `/home/cma`. |
| `native-vct-full-home-v2` | The same native authority checks with the full-home VCT layout and initializer; runtime home mounted at `/home`. |

Native bindings additionally pin the Claim's name, UID and request hashes, pool
configuration hash, storage class and capacity. Unknown or mixed contracts,
additional volumes, sidecars and incompatible mounts are refused. Synthetic
examples are in `fixtures/native.mjs` and `fixtures/native-vct.mjs`; they demonstrate
the input shape and tests, not fixture custody or deployed compatibility.

The reader issues only fixed resource GETs to a loopback HTTP(S) origin. All
reads share one timeout, defaulting to 10 seconds and bounded to 30 seconds;
redirects and response bodies over 4 MiB are refused. A complete namespace Pod
page is required, with at most 128 entries and no pagination. A Pod using the
original PVC is relevant even without tracking labels; ambiguous ownership
blocks observation. Related Pods, including terminating and completed Pods,
must disappear before suspension can be reported. Both the template and actual
runtime mounts must reference the original home.

The reader rechecks the Namespace, Sandbox, PVC and, for native VCT, Claim for
drift. These sequential reads are not an atomic distributed snapshot or a promise
about future resource state. Results retain sanitized identity/status evidence
and hashes with `scope: READ_ONLY_OBSERVATION` and `qualification: NOT_ASSESSED`.
Callers must propagate refusals instead of converting missing evidence to an
empty Pod list. The helper provides no resource mutation, command launcher,
cleanup authority or permission to execute a campaign.

## Injected lifecycle waits

Call `awaitCreatedIdle(read, {poll, awaitInitialIdle, now}, options)`:

- `poll(read, accept, {timeout, interval, code})` repeatedly reads bounded
  observations, invokes the acceptance predicate, propagates refusals and
  rejects at the deadline. Each individual read must also respect the budget.
- `awaitInitialIdle(read, {timeout, interval})` accepts Ready with no initial
  message, or waits for a supported setup/task message to complete. It refuses
  terminal, failed, unknown-purpose or unbound Working states.
- `now()` is a monotonic millisecond clock; it defaults to `performance.now`.
  Both wait phases share one deadline. Ready admission cannot reset the budget,
  and late/incomplete/inactive results cannot qualify.

Offline tests inject deterministic waits and virtual time to check startup,
initial-turn progression, refusal and deadline behavior without a cluster.

## Qualification limits

`summary.scope` is `CYCLE_SAMPLE`; `summary.qualification` is always
`NOT_ASSESSED`. A requested short smoke sample may pass, but it is never
M-SUST-03 PASS. Progress packets remain RUNNING until final receipt verification;
a final verification failure records FAIL even when all samples passed. Every
failed attempt stays in evidence and is never replaced by a retry sample.

Full qualification still requires 100/100 safe cycles, resume p95 at most 30
seconds, the production 900-second policy, administrator-only Always-on
exceptions, original-home/file/conversation/receipt retention, and a 48-hour
canary with zero active-turn interruptions. A 60-second accelerated test cannot
replace the production policy or canary. Bind results to exact application and
runtime sources/images, independent observations and successful cleanup. Neither
unit tests nor merging this core satisfy those requirements or authorize rollout.
