# Retained runtime / desktop compatibility matrix

This is the local two-AM slice of M-FUNC-02 (#1537), based on integration
`e4047c92606e2cda4117a4cb3686119c97a621d3`. It contributes to the sole
`local-verifier-02` parent. It does not launch an application, alter a cluster,
read a Secret, seed Mongo, or select a different image. No connected sample has
been run by this contribution. The complete #1537 gate remains open.

## Inventory and source contracts

`matrix.mjs` defines 48 cells: native/hosted × previous/current runtime ×
previous/current desktop × six scenarios. A cell includes its ordered steps,
minimum sample count, expected result and handoff directions. Every execution
uses disposable local fixture resources and requires cleanup. The existing
retained runtime and its home must survive the handoff; cleanup only happens
after preservation is measured. All attempts, including setup failures, remain
in the parent journal. Twenty consecutive reconnect successes cannot be produced
by filtering failed attempts or retrying within a sample.

The pinned desktop releases come from the existing retained-client manifest:

| Client           | Source                                     |
| ---------------- | ------------------------------------------ |
| Previous         | `f779e5310595fb62e19537a76f664ba52e8342f9` |
| Released current | `4f03c35a80322673ec00df7a3b6b9399015f5a68` |

Both source preparation modules have SHA256
`4cd86d6ea26212d8492a2b8a2ecee842d57103adb8a0cde4f59d68bc882f9da5`.
That module is already available and tested with fake sockets. A full compiled
historical Web/noVNC artifact is still required to run these desktop cells.
Neither that module nor the current RFB probe establishes old executable
compatibility. Retained runtime versions must come from an immutable native and
hosted inventory; this contribution does not invent release images.

| Boundary                      | Exact source at the selected integration                                                  | Contract                                                                                                                                     |
| ----------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Public desktop lifetime       | `api/crates/layer1-public/src/rest/desktop.rs::relay`                                     | 30 real minutes; public authorization tick every 30 seconds                                                                                  |
| Legacy session                | `api/crates/layer2-core/src/sandbox/desktop.rs::session_expiry`                           | 90-second expiry; this is the legacy OFF path                                                                                                |
| Replica durable desktop claim | `api/crates/cma/src/store/sandbox_sessions/exclusive.rs::apply`                           | `(issued_ms / 1000 + 15) * 1000`: greater than 14 seconds and at most 15 seconds                                                             |
| Receiver desktop challenge    | `api/crates/layer3-adapter/src/desktop_ownership.rs::DesktopOwnershipChallenge::validate` | 1–10,000 ms, original deadline must survive confirmation                                                                                     |
| Replica renewal               | `api/crates/layer2-core/src/sandbox/desktop/owned.rs::renew`                              | Independent renewal every 3 seconds; late ACK cannot restore expired authority                                                               |
| File/browser admission        | `api/crates/layer1-public/src/rest/agent_filesystem.rs::bounded_request`                  | One static two-slot semaphore per AM process; shared across file, browser, browser/control and environment routes; 60-second operation bound |
| Capacity refusal              | Same middleware                                                                           | `429 file_capacity`; acceptance target is at most 1 second                                                                                   |
| Desktop occupancy             | `api/crates/layer1-public/src/rest/desktop.rs::problem`                                   | `409 desktop_busy`, Retry-After 1; do not substitute this for capacity 429                                                                   |

**The 90-second legacy lease is not the two-AM desktop lease.** Its compatibility
cell needs an actual legacy session followed by the parent's approved no-overlap
transition, plus original-claim receipts. A fresh replica-mode session cannot
satisfy it. The shorter desktop receiver challenge is also distinct from Agent
ownership's protocol floor; inventory both and do not infer support from a v1
label or an updated default runtime image.

## Commands and honest result states

From the checkout root:

```sh
node --test .github/scripts/multi-am-features/compatibility/*.test.mjs
node .github/scripts/multi-am-features/compatibility/plan.mjs > /private/evidence/MATRIX.json
node .github/scripts/multi-am-features/compatibility/plan.mjs /private/evidence/bindings.json
```

The planner exits 2 if any binding is missing. Complete bindings produce
`NOT_RUN`, never PASS. No input means all 48 cells are BLOCKED. The binding file
contains only sanitized provenance, not credentials:

```js
{
  api: { source: '<exact e404 source>', image: '<repository>@sha256:<digest>' },
  environment: { kind: 'local-kind', sole_parent: 'local-verifier-02' },
  ams: {
    a: { ready: true, draining: false, pod_uid: '...', instance_id: '...', source: '...', image: '...' },
    b: { ready: true, draining: false, pod_uid: '...', instance_id: '...', source: '...', image: '...' }
  },
  runtimes: {
    native: { previous: runtimeInventory, current: runtimeInventory },
    hosted: { previous: runtimeInventory, current: runtimeInventory }
  },
  clients: {
    previous: { source: '<pinned>', kind: 'compiled-web-with-novnc', artifact_sha256: 'sha256:...', build_receipt_sha256: 'sha256:...' },
    current: { source: '<pinned>', kind: 'compiled-web-with-novnc', artifact_sha256: 'sha256:...', build_receipt_sha256: 'sha256:...' }
  },
  parent_contract_sha256: 'sha256:...',
  legacy_transition_contract_sha256: 'sha256:...'
}
// runtimeInventory includes image@sha256, full source40, boot_id,
// inventory_sha256 and ownership_floor_receipt_sha256.
```

Schema validation does not verify the contents of these referenced receipts.
The parent must independently read/hash them, verify image provenance and
collect live identity before any sample. A ready plan is only an inventory.

## Executable contributions and remaining parent interface

`capacity.mjs::capacityProbe` uses real HTTP sockets and the existing `API` from
`cma/core.mjs`. It opens two incomplete authenticated JSON bodies against the
selected AM (one file POST and one browser PUT); neither body can decode or
commit. It waits two seconds for admission, probes both file and browser GETs,
checks `429 file_capacity` and elapsed time, and records `x-request-id` plus the
actual serving process before/after each request. It refuses replacement
processes behind unchanged ingress labels. Each holder has a 20-second deadline
and every holder is destroyed and joined on success or failure. There is no
automatic replay. Polling/observation requests are separated by at least two
seconds. The isolated fixture must have no unrelated file/browser traffic.

The parent supplies `api`, `am`, `agentId`, `runtimeIncarnation`, exact `image`,
`sampleServing(am)`, `whileFull()` and private `record(event)`. `sampleServing`
must report the live port-forward target: am, origin, source40, image@sha256,
pod_uid, instance_id, container_id, ready, draining, observed_at. Labels alone
are insufficient. `whileFull` must execute real retained-client desktop input,
then interrupt and Stop, and save independent receiver effects and timings.
Do not use `/browser/control` as the supposedly independent input path: that
route shares the same limiter. A failed control check throws and still drains
the holders. The probe's returned transport evidence cannot itself pass a cell.

After it returns, the parent checks permit reuse with successful file/browser
reads on a live disposable fixture. Stop may have stopped the prior Agent, so
this check must preserve that result and use the parent's explicit restart/new
fixture step. Then repeat on the successor after the planned handoff. The parent
must prove actual overlap between held requests and each control effect; a
successful control after holder timeout is a failure.

`observe-lifetime.mjs::observeLifetime` watches one real retained-client socket
using the parent's Upgrade monotonic timestamp, original socket ID, original
close-event promise, actual socket/receiver/frame samples, and private recorder.
It uses real time with no duration override. It requires continuous observations,
server-initiated close after 30 minutes and within a further 30-second transport
allowance. The parent owns cancellation/disposal; it must not replace a failed
sample with a faster clock or a different auto-reconnected socket. It must also
verify receiver drain and the subsequent explicit view-only reconnect.

The remaining four scenario definitions require parent adapters that are not
implemented here: retained bundle browser launch/manual reconnect, legacy
no-overlap transition, session-specific renewal-ACK withholding, and planned AM
handoff with receiver-effect attribution. Existing `owner/adapter.mjs` has
topology/owner/runtime calls but no retained-browser or renewal-cut interface.
This is an explicit missing contract, not an executable-case completion claim.

Every connected receipt must bind candidate source/image, client artifact hash,
runtime image/source/boot, actual serving AM, owning AM/epoch, request/session IDs,
raw effect and drain events, test window, all attempts, and cleanup finality.
Old input needs a negative receiver-effect observation; socket close alone is
insufficient. A handoff needs a distinct successor process with higher owner
epoch and unchanged retained runtime/home. Do not manually edit claims, force
delete pods, or launch a second application parent to manufacture these events.

The tests here use disposable loopback servers solely to verify the transport
tool's saturation, incomplete-body safety, evidence checks and cleanup. They
are offline tooling results, with zero AM/runtime/desktop acceptance samples.
