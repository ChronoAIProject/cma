# Auto-pause campaign core

This directory contains the campaign engine, CMA public API adapter, native
runtime reader and offline contract tests. Native qualification requires a
disposable execution lane with a launcher and an owned fixture. Refs #1547.

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
native adapters are deliberately outside this extraction.

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

## CMA public API adapter

`runCmaCampaign` in `cma-public.mjs` uses separate owner and administrator
credentials and independently reads the Sandbox's native home. The injected
fixture validates custody and the deployed candidate before setup, prepares the
owned Agent and original file, provides scheduling observations and controls the
first synthetic turn. It also cleans up the fixture and restores policy. The
runner records a successful final result after cleanup returns successfully.

The loopback tests exercise the adapter against synthetic CMA and Kubernetes
responses. Use the same interface with a running CMA candidate and its native
Sandbox controller for runtime qualification.

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
