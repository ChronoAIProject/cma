# Ticket 15 evidence — complete

Recorded 2026-11-03. Branch `codex/cma-ui-15`, isolated worktree
`/Users/liuyitian/code/cma-ui-worktrees/ticket-15`. The latest merged integration
is `cf6774fd87dd39be8325e01984165efba12c7e87`, merged in `e091dd678`.
This retains 04/07/08/10/12/13/14/18 and all public-contract repairs.
Original acceptance wording is unchanged. The complete native/runtime/public/Web
chain and stable full-runtime Linux resource measurements pass. Feature/proof
chain source is pinned at `f495ff6b2`; `da83bbdc0` then corrects only Core
capability precedence for a paused provider with no browser adapter. The merged
10-case environment corpus caught this mismatch and now passes unchanged;
36 browser and 44 deliverable cases also pass after that correction.
Final source/artifact hashes are in the external `ticket15-proof/manifest.json`.

## Final end-to-end evidence

`ticket15-proof/native-chain-final/` holds the exact OCI config, reusable bounded
runner, native/public/Web logs, real JPEGs, result JSON and screenshots. The run
executes the own immutable candidate below, with native test transport archived
from `42f52fe43b60775d7efbd023ec5ccec2595e0b60`. Actual public and Web test sources
include the final `f495ff6b2` driver corrections.

The path actually exercised is:

1. Layer 4 HTTP installs credentials, prepares Workspace/Worktree/Agent home,
   launches pinned Codex/ACP, and accepts real conversation WebSocket calls.
2. Explicit owner PUT enables this Agent's browser. Two native Agent execution
   tools run `cma-browser navigate` against local pages A and B. Four controlled
   model responses produce two completed tools and actual HTTP page requests.
3. The supervisor and mediator produce bounded Chromium frames. A private file
   RPC forwards exact runtime control requests/receipts across the proof's
   network-none boundary to the real Core service and public router. It replaces
   only private runtime transport; it supplies no fixture frames or navigation.
4. Actual public browser/environment handlers, Core authorization, typed SDK and
   Web consume those responses. Authentication/store and unrelated conversation
   startup data are controlled fixtures. This is not production IdP, Mongo,
   hosted network transport or GKE deployment evidence.

The native process exits successfully. The named Rust public-chain test passes
**1/1**, and actual Chrome Web check passes **1/1 in 11.8s** (13.4s runner).
Final result: 15 forwarded runtime requests, 7 Web reads, 2 releases, foreign
read **404**, viewer enable **403**, stale incarnation **409**. Disconnect removes
Live; reconnect and close/reopen preserve page B and its context. Closing stops
polling and retains the conversation draft. Desktop, 390px and 320px screenshots
were inspected. The final check waits for the decoded image to lie inside the
viewport after resizing; earlier premature mobile screenshots are superseded.

| Page | Sequence | Actual JPEG SHA-256 | Bytes |
| --- | ---: | --- | ---: |
| A | 3 | `07bc02032033907f80f7c8508b0cfdcaa0982673709472c920f2de2d3035b423` | 7474 |
| B | 4 | `552820cbefb70fa3451cc729d8de0b3c093b40be38986b48646ca8fc66108c5d` | 7405 |

Both bind Agent `agt_shared_read`, context
`7756d0b5-decc-417c-a602-c3beb56ec785`, actual boot
`457ee74ecfe94add87fe46ba750addd1`, and public incarnation
`92eb64103ca089c1ce09332b6bf0209bda155f0f7de65ec947e4820dadcfa56d`.
Their hashes match the independent full-runtime HTTP native proof. Native
activity is established by actual tools and page requests, not screenshots alone.

## Primary full-runtime resource budget

The parent executed unmodified committed `22d8f876` native HTTP test under the
same verified gVisor and exact candidate binary: **1/1 passes in 27.3s**.
Artifacts: `runtime-proof/runsc-probe/http-evidence/`, `native-http-run.log`,
`http-bundle/config.json`. It proves two successful native helpers, one expected
pre-enable failure, six controlled model calls, two actual page requests, live
frames and reconnect. JSON `state: loading` is the initial enable acknowledgement;
the test separately requires each observed frame's state to be Live.

The host cgroup is mounted read-only at `/host-cgroup`. CPU uses cumulative
`cpu.stat usage_usec` over monotonic time, rejecting changed cgroup identity or
backwards counters. Memory samples charged `memory.current`. This scope includes
the Rust runtime, native Agent, browser and proof processes; it excludes public
API/Web and unrelated VM processes.

| Mode | Seconds | Leases | Captures | CPU % of one core | Peak charged bytes | Observation JSON B/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Normal | 5.246 | 1 | 4 | 20.35 | 968544256 | 9741.88 |
| Hidden | 5.063 | 0 | 0 | 16.13 | 965300224 | 0 |
| Closed | 5.051 | 0 | 0 | 15.28 | 952246272 | 0 |
| Multiple | 5.368 | 4 | 5 | 19.78 | 933285888 | 38089.01 |

These are approximately five-second measurements on two small local pages,
not sustained-load guarantees, browser-only working memory or full HTTP/TLS
wire accounting. Hidden/closed stop capture/delivery while Agent/browser work
may continue. Lower sampled values in later phases do not establish that more
observers reduce cost. The VM has 2 vCPUs and 4 GiB RAM. OCI requests 2 GiB memory,
512 PIDs and one CPU; the earlier host sampler observed memory/PID limits but
`cpu.max='max 100000'`, so an enforced one-core CPU cap is **not claimed**.

The smaller mediator/CLI-only run remains at `runsc-probe/cgroup-budget.json`:
normal 12.87% / 518.50 MiB, hidden 5.75% / 505.11 MiB, closed 4.98% / 500.80 MiB,
four observers 7.94% / 490.17 MiB, whole charged peak 539.16 MiB, peak 99 PIDs.
Its 100ms host samples avoid process-RSS double counting. This is supplemental
evidence with a smaller boundary, not the primary full-runtime cost.

Application limits: explicit default-off enablement; at most one enabled Agent
browser per Sandbox, 16 expiring observer leases, one shared frame, 1280×720 JPEG
quality 45, ≤262144 decoded bytes/frame, ≥1000ms between capture attempts,
≤16 delivered frames per rolling second, bounded CDP/control buffers/deadlines.
The limits also bound payloads on complex pages; measured small-page bandwidth
is not a universal maximum.

## Immutable Linux candidate

All commands use Docker context `colima-cma-ui-proof`. No deployment, production
credentials, Docker restart, Mongo stop or VM shutdown is involved.

| Item | Exact identity |
| --- | --- |
| Runtime build source | `8be7293571476efd4c0e00e4b38e5abc9efd091b` |
| Candidate image | `cma-ui-runtime-proof:ticket15-final` |
| Immutable image ID | `sha256:ddda63f74cd1b2067102fb43e66e1a2c47ced2f569c701bcc52d309bf816e112` |
| New runtime binary SHA-256 | `364d71a3424476e5c7b27583b0df39cce1751e0d92be06347baa62a1e6dbf531` |
| Rust builder | `rust:1.96.1-alpine@sha256:a41f7740f8b45d45795624eec13a8b42263cc700f19f7e4e86e04d3dda08a479` |
| Image base | `sha256:b8fb789ea6e1e6a50f6faa4d34df90a49856823e8f28526cdc7ac211ebe907d4` (04 final) |
| Patched ACP 1.4.0 bridge SHA-256 | `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c` |
| Pinned native provider | Codex 0.147.0, unchanged provider bytes |
| Linux browser / Node | Chromium 156.0.8078.79 / Node 24.19.0 |
| Policy-test correction source | `ad48bfd28dd291cdefb85a7e9c210d7a1688f97c` |
| Native HTTP test source | `22d8f876074714ba958a38e85b5cbd19180564cc` |

The immutable source archive, Dockerfile and binary are in
`/Users/liuyitian/code/cma-ui-worktrees/ticket15-proof/candidate-final`.
`linux-final-build.log` records the two-job build of the **new 15** runtime.
This minimal proof image includes pinned `chromium-sandbox` and the new mediated
navigation command. It is not the complete production image.

`runtime-policy-current-test.log` passes one named test against that immutable
binary. The corrected, committed test is mounted read-only from `ad48bfd28`;
the image's original test incorrectly expected wire tag `error` instead of
`failure`. The test proves explicit default-off behavior, current runtime and
Agent fencing, a real opened pinned ACP process, honest unsupported responses,
zero frames/observers/captures and successful disable. It does not prove native
page navigation. `runtime-policy.json` records the actual executed binary hash.

The immutable native HTTP harness is archived at
`/Users/liuyitian/code/cma-ui-worktrees/ticket15-proof/native-http-source`.
`immutable-native-http-policy.log` executes both corrected policy tests from
that committed archive against the exact candidate image: two named tests pass.
The additional test prepares a real Workspace/Worktree and Agent home through
runtime HTTP, launches the pinned bridge under the actual runtime supervisor,
and submits native prompts through its conversation WebSocket. Two native
execution attempts reach `/usr/local/bin/cma-browser`; the first cannot enable
a browser implicitly, and the second truthfully refuses after namespace-denied
enablement. `runtime-native-http.json` records **zero successful tool completions,
two failed native tool attempts, four model-boundary calls, no page requests,
no frames and no budgets**. This proves the actual HTTP/native refusal path,
not successful browser navigation.

For the parent's independent compatible-runtime probe, mount the archived
`api/crates` directory read-only at `/immutable` and invoke:

```sh
node --test /immutable/layer4-runtime/src/browser/native-http.test.cjs
```

Supply the image's existing `CMA_RUNTIME_TEST_BINARY`, `CMA_CODEX_TEST_BINARY`
and `CMA_ACP_TEST_BRIDGE`. Leave `CMA_BROWSER_POLICY_PROOF` unset for the real
navigation acceptance attempt; setting it to `1` deliberately tests refusal
and never yields browser acceptance. `CMA_BROWSER_PROOF_DIRECTORY` selects the
output directory. The live branch requires two successful actual native
navigations and real runtime HTTP frames, preserves context on release/reconnect,
and measures four short resource windows. It reads stable cgroup v2 `cpu.stat`
and `memory.current` (optionally `CMA_CGROUP_DIRECTORY`), rejects backwards
cumulative CPU and changing cgroup identity, and labels full-container scope.
The live/resource branch now passes in the parent's pinned gVisor HTTP run above;
the completed public/Web chain is recorded in the first section.

No production runtime source or dependency changed between binary source
`8be729357` and final feature source `da83bbdc0`; only browser test/harness files
and 18's native replay test changed in Layers 3/4. The candidate contains the current
15 runtime implementation, while its external tests have separately pinned
committed provenance.

## Linux feasibility boundary

`linux-final-policy-diagnostic.log` records uid 10001, no capabilities,
`NoNewPrivs: 1`, default Docker seccomp, and the installed root-owned 4755
Chromium sandbox helper. `unshare -Ur true` returns EPERM; Chromium cannot enter
its sandbox and exits. The live navigation test in this exact image fails with
`unsupported`, zero frames and zero observers (`linux-final-native-blocked.log`).
No `--no-sandbox` flag or weakened browser isolation is used or accepted.

This establishes a limitation of Docker/runc policy. Official runsc
**release-20261102.0**, aarch64/systrap, now passes mediator and full-runtime
native tests. Official archive SHA-256:
`909c95f245a46000d3bb0d5285f07dde23597056e7c3a4aab758b218e95bdab0`.
The parent verified the official checksum and actual version; commands,
configuration and downloads are retained in `runtime-proof/runsc-probe/README.md`.
The proof uses uid/gid10001, empty capability sets, noNewPrivileges, read-only
root, private writable tmpfs and no external network. No Chrome `--no-sandbox`,
privileged container, Docker restart or policy weakening is used. This proves
this pinned gVisor configuration, **not** the actual production GKE runtime.
Docker/runc refusal remains truthful. The earlier `--no-sandbox` primitive probe
is excluded from acceptance.

## Preserved native development evidence and corrected accounting

`native-browser.json` and `native-final.log` preserve an actual native Codex
execution through the pinned ACP bridge. A controlled external model response
selects the real native execution tool. Two completed native tool calls navigate
the private browser mediator to two local HTTP pages; this is not direct
screenshot-only navigation. The JPEG hashes are:

- A: `2183d5284f39f647c3175aa9d1301df5b7aadc03838f678b7d96edd34e83837b`
- B: `6bac282de9327554518725873bc163cf55245f230d5d13b8c4b0373e19f63e97`

The proof uses macOS Chrome 156.0.8078.80 and Node 26.12.1. It proves native
mediator navigation, bounded real frames and same-context observation reconnect;
it does not prove Linux equivalence or the complete public-service/Web chain.

| Mode | Window seconds | Observer leases | Captures | Response bytes | Sampled peak aggregate RSS KiB | CPU |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Normal | 5.190 | 1 | 5 | 51010 | 1737584 | unknown |
| Hidden | 5.135 | 0 | 0 | 0 | 1488448 | unknown |
| Closed | 5.108 | 0 | 0 | 0 | 1418048 | unknown |
| Multiple | 5.283 | 4 | 5 | 204064 | 1330848 | unknown |

CPU is explicitly null/unknown: changing child-process membership invalidates
cumulative process-tree deltas. No negative value was clamped or counted as a
measurement. The rejected earlier artifact is retained as
`native-browser-invalid-cpu-superseded.json`; **none of its CPU values are valid**.
RSS sums sampled mediator/Chromium process RSS and can double-count shared pages;
it is not isolated browser working memory. Response bytes measure application
observation JSON, not complete public HTTP/TLS wire bandwidth. These are short
five-second windows, not sustained-load results. The primary stable Linux
full-runtime measurements above supplement rather than rewrite this artifact.

## Shared contracts and Web

`agent-browser.json` registers 36 shared cases for GET/PUT/DELETE browser routes.
The real public router and Core authorization run with a controlled runtime
port; frame fixtures prove protocol handling only. The same cases pass through
typed TypeScript and Rust SDK methods (36 HTTP cases each). The shared JSON
inventory contains all three browser operations and their exact named problems.
The parent's 14 environment and four 07 deliverable entries, shared handler
cases and both SDK dispatchers are retained. The independent JSON policy gate
passes **238/238 OpenAPI operations, 2834 cases, 1620 exact named HTTP problem
pairs and 23 protocol-failure pairs** (`shared-inventory-final.log`). This is
distinct from the Markdown `API_REST` inventory check. Latest focused handler
replay passes 36 browser cases; Rust SDK passes 36 typed HTTP cases. The
TypeScript browser-name filter passes 56 cases (the 36 browser-operation cases
plus existing browser-owned protocol cases).

The merged Web has four focused panel tests and one actual Chrome rendering
check at 1440/390/320 pixels. It decodes real development JPEGs through a
controlled public response, verifies default selection and retained manual
selection, freshness expiry, reconnect, hidden/closed leases, scope changes and
preserved conversation drafts. It is U evidence; the actual chain is separate.
Earlier screenshots/logs remain under `ticket15-proof/web-final` and
`web-chrome-merged.log`. Latest merged checks: 3 Web files / 16 tests, typecheck,
production build, one Chrome U check, two named 14 private-file/shared-workspace
guard tests, shared browser handler/SDK cases and independent JSON inventory.
Existing build chunk warnings and baseline format/size issues are not changed.

## Handoff to 16 and 17

16 can use owner-authorized `PUT /api/v1/agents/{id}/browser` explicit enablement
after current runtime/Agent readiness. Default status is not_enabled; opening
Computer or creating a Sandbox does not start a browser. Incarnation comes from
the existing environment endpoint. There is no global desktop startup or Home
creation option in 15.

17 can extend the private mediated runtime browser command boundary, fenced by
Agent/Sandbox/runtime boot and browser context. CDP stays on inherited pipes;
native commands use a private Unix socket. The public service only forwards
authorized observation/configuration. No public input/navigation route, takeover
lease, exclusive-input arbitration, PTY or available-control claim is present.
Environment control remains unsupported.

The single AgentPanel/onSelect and observation owner are preserved; 18 operation
replay retains explicit selection and never supplies historical browser frames.
14 private-home/upload guards and shared Workspace behavior remain intact.
Only own proof services are stopped; worktree and VM remain for the parent.
No deploy, push or PR is performed.
