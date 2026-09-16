# Owned Agent continuity runners

These committed tools reproduce the prepared #850 active-turn, retained native
history and passive browser observation. They consume successful disposable
canary receipts and use public CMA routes. They do not create fixtures, deploy an
application, change flags or permissions, inject provider faults, pause resources
or perform cleanup. The existing read-only observer supplies exact runtime
Pod/container evidence through its fixed scoped `kubectl get pods` command.

Use only the exact run-owned Trigger/manual pair after its successful setup. The
fixture journal uses `cma-public-canary-journal-v1` and records `run`, `origin`,
`owner`, `commit`, `commands`, `resources`, `baseline.firstTurns`, and `result`.
`resources` contains `sandboxId`, `workspaceId`, and two `agentIds` in Trigger,
manual order. `baseline.firstTurns` contains their actual original transcript
turns. The successful result retains `resources` and `nativeIdentity`, whose
entries contain `conversationSha256`, `runtimeGeneration`, and `openings`.
The separate result file comes from that same completed fixture. Do not invent
receipts or substitute Agent IDs to make another resource fit this contract.

The runners recheck the saved token's owner/mode, the public signed-in subject,
exact resource lineage and manual/Trigger provenance, original transcript,
native identity and runtime attachment. Any original cleanup intent prevents
continuation. Keep the pair quiet except for the single coordinated scenario;
another Agent input can invalidate the exact turn-count proof.

## Private configuration

Create configuration and evidence outside the checkout. Credentials remain in
the existing operator-owned mode-0600 token file; configuration contains only its
path. The output directory is mode 0700. The following values are placeholders:

```json
{
  "schema": "cma-owned-continuity-config-v1",
  "origin": "https://fixture.example",
  "run": "owned-fixture-run",
  "expectedSubject": "fixture-owner-subject",
  "context": "verified-fixture-context",
  "namespace": "fixture-namespace",
  "deploymentId": "fixture-deployment",
  "tokenFile": "/private/fixture/access-token",
  "fixtureJournal": "/private/fixture/journal.json",
  "fixtureResult": "/private/fixture/result.json",
  "outputDirectory": "/private/fixture/continuity",
  "markers": {
    "trigger": "ORIGINAL-TRIGGER-MARKER",
    "manual": "ORIGINAL-MANUAL-MARKER"
  },
  "limits": { "turnSeconds": 300, "pollSeconds": 2 }
}
```

Use the markers already established by the successful first turns. HTTPS origin,
cluster context, namespace, deployment identity, run and owner must match the
actual fixture. `turnSeconds` is 30–1200 and `pollSeconds` is 1–30. The optional
`browserExecutable` is an absolute local browser executable; omitting it selects
the browser installed for the repository's pinned Playwright dependency.

Use the repository CI Node version. For browser observation, install the locked
web dependencies with `npm ci` from `web/`, then run
`npx --no-install playwright install chromium` there. No additional dependency or
package registry publication is introduced by this tooling.

## Idle continuation

Supply a new bounded change name and the actual observed application SHA:

```sh
node .github/scripts/cma-bridge-continuity-owned.mjs /private/fixture/continuity.json capture change-one BEFORE_FULL_SHA
# Coordinator performs the named change and records readiness separately.
node .github/scripts/cma-bridge-continuity-owned.mjs /private/fixture/continuity.json continue change-one AFTER_FULL_SHA
```

Capture is read-only. Continuation verifies the retained baseline before adding
exactly one keyed v1 input to the original Trigger Agent and one streaming v2
input to the original manual Agent. Each recalls its original marker without
tools or file operations. Completed original turns must remain byte-equivalent
under canonical serialization; appended history is allowed. Same-SHA changes can
describe configuration rollout, but never imply application-image rollback.

## Active turn and browser reconnect

Use a distinct change name for this mode. Capture first, then start the passive
browser in its own terminal while both Agents are quiet:

```sh
node .github/scripts/cma-bridge-continuity-owned.mjs /private/fixture/continuity.json capture active-one BEFORE_FULL_SHA
node .github/scripts/cma-bridge-continuity-browser.mjs /private/fixture/continuity.json active-one BEFORE_FULL_SHA
```

Wait for `holding_owned_active_browser`. In the coordinator terminal, submit and
observe the one owned active turn:

```sh
node .github/scripts/cma-bridge-continuity-owned.mjs /private/fixture/continuity.json start-active active-one BEFORE_FULL_SHA
node .github/scripts/cma-bridge-continuity-owned.mjs /private/fixture/continuity.json observe-active active-one BEFORE_FULL_SHA
```

The prompt launches one 600-second sleep and prints a marker. If the runtime
tool yields a running session, only bounded wait/poll calls on that same session
are allowed. It must not launch the command again. Missing tools, new approval
requirements, an unidentified session, failed/interrupted turns and substituted
request/tool identities stop the scenario. Public tool summaries cannot prove
exactly-once physical execution; the receipt states that limit.

After the active checkpoint is armed, send this line to the browser's stdin:

```json
{"action":"arm"}
```

The browser requires the exact active prompt to be visible without navigation,
mutation attempts or manual refresh. Once both active and browser checkpoints are
armed, the coordinator performs the rollout and records actual start/readiness
times. Recover by reading the original response and command after readiness:

```sh
node .github/scripts/cma-bridge-continuity-owned.mjs /private/fixture/continuity.json recover-active active-one AFTER_FULL_SHA
```

Recovery submits no input. It checks the same native/runtime identity, retained
event/transcript prefixes and unchanged sibling turns, then waits for the
original response to complete. Before/after SHAs must differ for this active
application-rollout mode. After that result passes, send the actual full SHA to
the browser:

```json
{"action":"verify","commit":"ACTUAL_AFTER_FULL_SHA"}
```

The browser never reloads, focuses, clicks, resubmits or repairs the page. It
observes the real client's consumed stream bytes without teeing the body,
records metadata/errors without message contents, and waits for the original
turn's final marker in the same document. A successful close/reopen of the exact
Agent's v2 or legacy AG-UI stream inside the armed interval establishes observed
automatic reconnection. Polling alone, a failed new stream, or no observed
closure/reopen yields `partial`, not reconnect proof. Dropped instrumentation,
changed history, another document or mutation attempts prevent a pass.

Use `{"action":"status"}` for local status and `{"action":"close"}` to close
the isolated browser. Observation is bounded to 45 minutes. The browser process
retains `passed`, `partial`, `failed` or incomplete evidence under `browser/CHANGE`;
inspect that result rather than equating normal process termination with a pass.

## Recovery and evidence

Every POST stores and syncs its exact path, body and key before dispatch. Stream
headers are retained before body observation. An identical accepted request
recovers its receipt; pending/unsettled requests refuse automatic replay. The
per-change lock prevents overlapping continuation runners. Never delete a
journal, choose another key, or remove a stale lock to hide an uncertain outcome.
Read the retained command/response and let the coordinator determine recovery.

Outputs preserve the original fixture files and record new observations under
`continuations/CHANGE` and `browser/CHANGE`. Pin the executed source commit/helper
hashes, Node/browser versions, exact application images, runtime contract and
coordinator rollout times alongside those receipts. SHA arguments are recorded
assertions, not independent deployment attestation. Failed attempts remain failed.

These tools do not prove approval-wait survival, dirty-file or pause/revive
behavior, provider delivery, cookie/OAuth renewal, continuous availability between
observations or the complete #850/#873 matrix. They preserve the narrower existing
proofs and report their boundaries explicitly. Offline CI exercises scope,
retained requests, active/native history and passive browser mechanics without
network calls or production fixtures.
