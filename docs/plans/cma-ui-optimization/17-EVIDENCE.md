# Ticket 17 evidence and handoff

Date: 2026-11-03. The eight original acceptance criteria are satisfied within
the approved A/U/R seams. This is local pinned-environment evidence, not a
production deployment or the integrated ticket 19 acceptance.

All generated logs, image/source manifests, screenshots and failed attempts are
outside the repository at
`/Users/liuyitian/code/cma-ui-worktrees/ticket17-proof` (called `PROOF` below).
The final commit, changed-path file hashes, validation revisions and artifact
hashes are sealed in `PROOF/HANDOFF.md` and `PROOF/manifest.json`.

## Delivered behavior

The existing browser mediator serializes native Agent navigation/input and user
lease commands. An acknowledged lease blocks actual native input at execution;
return resumes that same browser without sending a conversation task. A private
client key is bound to current account/credential identity, client, context,
runtime incarnation, token and revision. Public GET/POST browser-control routes,
both typed SDKs, capability discovery and the Web use that same authority.

The Web supports actual viewport clicks, bounded text and keyboard editing,
explicit acquire/return, exact-operation recovery, read-only audit, offline and
permission refusal. It does not claim control before the receipt. Pending
recovery metadata is scoped to account/Agent/incarnation/context; entered text
and lease tokens are not stored in session storage. Old account responses cannot
publish into the new panel. Control actions wait for an in-flight status read.

The detailed authority, 15-second expiry, 128-entry receipt/audit bounds,
incarnation policy and reader floor are in `17-CONTROL-CONTRACT.md`.

## Acceptance map

| Original criterion | Observable evidence |
| --- | --- |
| Supported runtime and current operating authority only | Real public authorization/feature probe; 37-case corpus includes shared writer/viewer, foreign, revoked, anonymous, paused, unsupported and unavailable. Live public permission switch refuses renewal with 403 and leaves input disabled. |
| Agent/user/transferring/unknown and verifiable identity | U receipt-held test; real lost acquire response displays unknown; native input is refused before the original operation is queried and user control is confirmed. |
| Same-browser mutual exclusion and competing identities | Native HTTP test performs actual USER-17 input concurrently with native Agent input; native receives browser_control_conflict. Full Web chain rejects a competing client, a stolen lease under another client, and recovery with another private key. |
| Return preserves context/page and sends no task | Real native Agent appends -AGENT; same context and one page load. Web draft remains unchanged; zero task input commands. Old user token receives 409. |
| Lost replies recover the original operation | Real acquire/input/return responses are deliberately lost after the public/runtime operation. Querying exact receipts recovers ownership; -ONCE is inserted exactly once. Exact retry and bounded receipt eviction also have R regressions. |
| Expiry/offline/account/permission/incarnation | Runtime monotonic expiry is tested without delivery of release. Full chain goes offline for 15.5 seconds, observes native -EXPIRED and public expired audit. Old incarnation/context and token refuse input; account-scoped recovery and late-response tests pass. |
| Bounded browser authority and public audit | Bounded click/text/key vocabulary, no public script/CDP/PTY. Authorized viewer reads acquired/input/returned/expired metadata without text, key or lease token. No service grant, approval mode or task authority changes. |
| Real input, competitor and reconnect | Actual Linux Chromium through pinned native Agent, Layer4 HTTP, Core/public handlers, TypeScript SDK and real Web. Backspace changes USER-17 to USER-1, then 7 restores it. This evidence is independent of disabled-button assertions. |

## TDD and regressions

| Boundary | Red → green | Artifacts |
| --- | --- | --- |
| First native known-form exclusion/return | 1 failed → 1 passed, zero skipped | `first-red.log`, `first-green.log`, `first-green/` |
| Runtime expiry | 1 failed/1 passed → 2 passed | `expiry-red.log`, `expiry-green.log` |
| P1 delayed Agent POST crossing disable/enable | Actual stale input applied with HTTP 200 → input and navigation refuse after context replacement | `delayed-context-red.log`, `control-runtime-final.log`, `linux-boundaries.log` |
| Lost receipts and old acquisition receipt | Each 1 failed → 1 passed | `recovery-{red,green}.log`, `receipt-generation-{red,green}.log` |
| Private client identity | Actor-binding regression → 2 named handler tests pass; SDK key-in-body regression corrected | `client-key-{red,green}.log`, `client-key-sdk-red.log` |
| Account recovery | 1 failed/4 passed → 3 files/10 passed | `account-scope-{red,green}.log` |
| Acquisition while current-state read is pending | 1 failed/5 passed → 3 files/11 passed | `acquire-busy-{red,green}.log` |

The parent review of early `1637de223` remains at
`../ticket17-early-native-review.md` relative to PROOF. P1 is closed by capturing
the native HTTP request's context before collecting its body and comparing it
again inside the serialized execution. P2 is closed by the runtime's monotonic
deadline, checked before native/user execution; neither relies on an upstream
expiry message. Both regressions pass with real Chrome, including the Linux
boundary suite of five named tests, zero skipped.

## Immutable runtime and actual chain

| Component | Exact identity |
| --- | --- |
| Runtime production build source | `556aef8ba35bbcba8cb60e1eb122cde36eda7f7f` |
| Runtime SHA-256 | `6283b369b5bedcd8c8d742786f061cce45fbcbb3ead18a2241db473286f624e3` |
| Final baked proof source | `23d92fdcc6fdd430e5e8ed5a1f6c42a8b27ee294` |
| Final image | `sha256:4ebc67d07fef6c471b433df01723b277a2db51a432df47a9523a5f8e68d8ca37` |
| Image tag | `cma-ui-runtime-proof:ticket17-23d92fdcc` |
| Native CLI / ACP | Codex 0.147.0; patched ACP 1.4.0, SHA `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c` |
| Linux browser / Node / gVisor | Chromium 156.0.8078.79 / Node v24.19.0 / task-local runsc release-20261102.0, aarch64 systrap |
| Final pre-merge Web/driver revision | `630aba9e0` (full SHA in external manifest) |

The first own image `3956e4f8461fad990bc37f67334fd79eb899046d292e8be2e2a76174f68ad8c7`
was built on exact ticket15 base `ddda63f74cd1b2067102fb43e66e1a2c47ced2f569c701bcc52d309bf816e112`
using a private Cargo volume. The derivative changes only baked test sampling:
wait for a fresh frame after return and assert increasing frame sequence.
All production API/Cargo source is byte-identical between the two source pins;
`candidate-23d92fdcc/runtime-equivalence.json` and both 3,886-file manifests record
that comparison. Runtime bytes are not represented as a build of later Web or
integration source. The original ACP 0644 image defect is corrected with
chmod0755/test-x for target and symlink; ACP content hash is unchanged.

`linux-native-fresh.log` has one named actual-runtime-HTTP test passed, no skips.
The actual form records USER-17 then USER-17-AGENT, one page/context. The fresh
`parent-oci/output/native/{user,returned}.jpg` visibly show those values.

The final full chain has public **1 named test passed, zero ignored**, Playwright
**1 passed**, and native process exit 0. It forwards 59 actual runtime requests
through the private file transport and executes five real native commands with
ten controlled model responses. Two native inputs are refused; return and expiry
permit the remaining two. `parent-oci/relay/web-chain-result.json` records:

```
USER-17 → USER-1 → USER-17 → USER-17-ONCE
→ USER-17-ONCE-AGENT → USER-17-ONCE-AGENT-EXPIRED
```

Before explicit disable/enable there is one page load and one browser context.
Public audit includes acquired, input, returned and expired. Competing client,
wrong private key, stale token/incarnation/context, account permission and all
three lost receipt checks pass. The public JPEG visibly contains USER-17-ONCE.
Desktop and fully settled 390/320px frame/action placement are asserted and
visually inspected. Parent independently records the exact screenshot hashes
and limits in `PROOF/PARENT-VISUAL-REVIEW.md`, retained in the artifact seal.

The final native/public/Web commands, configs, process list, runtime identities
and logs are under `parent-oci/`, `linux-*.log`, `public-chain.log` and
`web-control-chain.log`. `parent-oci/README.md` documents exact runsc flags;
`NATIVE-RUNNER.md` documents the host API and Playwright entrypoints. The host API
test checks runtime SHA/source identity against the candidate manifest before
publishing its real loopback routers. No moving runtime source bind is used.

## Isolation and failures retained

The Linux proof runs uid/gid10001, empty capability sets, noNewPrivileges,
read-only image root, private 512MiB tmpfs, no external network, no Docker socket
and no Chrome --no-sandbox. Actual process inventory shows runtime, Codex and
Chromium all uid10001. Cgroup readback is memory.max2147483648, pids.max512 and
cpu.max100000/100000 for these17 runs; this does not rewrite15's earlier unlimited
CPU observation. No Docker/VM restart or shared Mongo stop occurred.

The declared controlled boundaries are local model tool selection, fixture
IdP/resource store/startup/conversation and file RPC replacing only private
network transport. Actual native tool execution, runtime HTTP, authorization,
public router, SDK and Web participate. This does not certify production IdP,
Mongo, GKE/network routing, arbitrary native permission modes, unrestricted
same-uid hostile shell isolation, screen-reader behavior or all44px targets.
Native commands use the explicitly approved agent-full-access fixture mode.

Earlier full-chain attempts01–05 are preserved, never relabelled. They exposed
a test drop guard aborting ordinary GETs, the acquisition/read UI race, shutdown
before observer DELETE drained, and an overly broad frame timestamp selector.
Fresh output leaves also corrected an artifact-file EACCES without weakening
runtime permissions. An export filled the VM's11GiB root disk; only17's inactive
first exported copy was removed after runsc list was empty. Its immutable image,
source and evidence remain. The successful final chain uses a fresh relay.

The background conversation live-error/empty-state fixtures are visible and
explicitly outside17's bounded control acceptance. They are not hidden or
presented as integrated19 success. Parent's visual review retains that limit.

## Host validation and compatibility

Before the final16 integration merge: Web337files/3015tests pass; control-focused
3files/11tests pass; Web lint/types/full E2E types/build/CSS pass. Real sandboxed
Chrome computed styles resolve179properties and85 scoped Flow references in both
themes. TS SDK71files/3741tests pass with one existing opt-in skip; Rust SDK209
named tests across37targets pass with two existing opt-in ignores. New37-case
control corpus executes through one named dispatcher in each real-handler,
typed Rust SDK and typed TS SDK suite; existing browser36/environment10 cases
remain green. Four layers, CMA and Rust SDK all-target Clippy, SDK docs/build/lint
and the named OpenAPI/Markdown operation inventory pass. Post-merge validation
is recorded separately with its exact revision in the external handoff.

No gates or tests were disabled and no new ignores were introduced. The size
gate retains only the original byte-identical508-line agent_operation_service;
its SHA is in `known-baseline-limits.json`. Original unrelated format/skill/Core
and LocalAgentV2 head404 limitations remain separately recorded by integration.

No new durable Mongo schema is added. Control/audit/receipts are volatile within
an incarnation. The15-aware strict RuntimeStatus reader remains the rollback
floor even Browser off; parent independently proved the baseline rejects
agent_browser. Retained16 creation journals impose their additional reader
floor. Restarting a runtime cannot promise same-context recovery. Ticket19 must
verify the exact composed release and restart/rollback behavior; disabling the
UI is not an old-reader compatibility strategy.
