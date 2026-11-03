# Ticket 15 evidence — in progress

Recorded 2026-11-03. Branch `codex/cma-ui-15`, isolated worktree
`/Users/liuyitian/code/cma-ui-worktrees/ticket-15`. The latest merged integration
at this checkpoint is `7cd701d0fe6c2496256a01eca59919ee8d899c82` (including
04/07/08/10/12/13/14), merged in `ed371149e817261cf74a32ad3a79f25e856c834a`.
Original acceptance wording is unchanged. Linux browser
acceptance and its live resource budget remain pending; this is not a completed
handoff to 16/17.

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
Those live/resource branches are **not yet validated** under a compatible Linux
policy. The authorized public service and Web part of the complete R chain
remains a separate outstanding requirement.

No production runtime source or dependency changed between binary source
`8be729357` and merged `ed371149e`; only the three browser test/harness files
changed under the runtime crate. The candidate therefore contains the current
15 runtime implementation, while its external tests have separately pinned
committed provenance.

## Linux feasibility boundary

`linux-final-policy-diagnostic.log` records uid 10001, no capabilities,
`NoNewPrivs: 1`, default Docker seccomp, and the installed root-owned 4755
Chromium sandbox helper. `unshare -Ur true` returns EPERM; Chromium cannot enter
its sandbox and exits. The live navigation test in this exact image fails with
`unsupported`, zero frames and zero observers (`linux-final-native-blocked.log`).
No `--no-sandbox` flag or weakened browser isolation is used or accepted.

This establishes a limitation of the available Docker/runc policy. It does
**not** establish that actual production gVisor is unsupported. The parent is
independently probing an official standalone runsc OCI route; ticket15 does not
duplicate that probe. A successful compatible route must still run the complete
native navigation, public authorization, Web and resource-budget chain.

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
five-second windows, not sustained-load results. Required Linux CPU/memory and
wire-budget measurements remain pending, using a stable cgroup/container scope.

## Shared contracts and Web

`agent-browser.json` registers 36 shared cases for GET/PUT/DELETE browser routes.
The real public router and Core authorization run with a controlled runtime
port; frame fixtures prove protocol handling only. The same cases pass through
typed TypeScript and Rust SDK methods (36 HTTP cases each). The shared JSON
inventory contains all three browser operations and their exact named problems.
Before the parent's 14/07 repair merges, its global count is 238 actual versus
233 declared; the five unrelated missing entries remain visible, not exempted.

The merged Web has four focused panel tests and one actual Chrome rendering
check at 1440/390/320 pixels. It decodes real development JPEGs through a
controlled public response, verifies default selection and retained manual
selection, freshness expiry, reconnect, hidden/closed leases, scope changes and
preserved conversation drafts. It is U evidence, not the missing complete R
chain. Stable screenshots and logs are under `ticket15-proof/web-final` and
`web-chrome-merged.log`. Web/SDK TypeScript checks and the merged Web build pass.
