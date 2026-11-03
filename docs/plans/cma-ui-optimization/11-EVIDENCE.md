# Ticket 11 delivery evidence

Date: 2026-11-03. Branch: `codex/cma-ui-11`. Worktree:
`/Users/liuyitian/code/cma-ui-worktrees/ticket-11`.

## Delivered behavior

The production source is one explicit comparison template, not model generation.
Only a durable successful latest answer to `Compare … and/versus/vs …` is eligible.
The template sends **Add explicit comparison criteria and explain the trade-offs.**
through the Agent's selected normal transport. The manual draft and attachments
remain owned by the composer. Suggestions are separate from the answer renderer;
this ticket does not edit `StreamEntry` or `MessageStream`.

| Acceptance | Evidence |
| --- | --- |
| Bounded scenario; unrelated requests have no suggestions | Full-conversation U tests cover the comparison and an unrelated completed follow-up. Source is labelled Scenario templates. |
| Viewer/Agent/turn/answer/source/item identity; 0–3 candidates | `SuggestedReplySet`, supplied-set validation, three-item keyboard test, empty/invalid/unavailable-set test. Production emits one template. |
| Latest final answer only; read-only and active/failed/superseded turns cannot send | Canonical `ConversationMessage.finalAnswer` and backend `AgentTranscriptTurn::final_answer()`; `latestTurnId` prevents old-answer revival. Core rejects missing-output completion and a newer admission even after its rejection. |
| Exact visible input and normal gates | U tests retain Unicode, multiline and trailing-space text. Public A test verifies the same exact text in finite history for both AG-UI and Responses v2. Existing connection and approval flows remain in the shared send function. |
| Draft preserved; immediate state and duplicate-click lock | Double-click/lost-receipt U test, polite region status, composer value assertions, existing connection/control regression tests and Chrome keyboard/touch tests. |
| Definite refusal versus uncertain recovery | 422 permits retry; stale 409 expires the group; unsupported Responses fields leave manual sending available. Uncertain requests retain exact scope/text/key. Both transports recover that original request. |
| Permission and cross-tab fences | Public owner isolation plus fresh normal admission authority; U access-loss/viewer-switch checks; actual Mongo concurrent replicas admit one request. A second key for a consumed answer is refused. |
| Refresh and failure isolation | Pending refresh remains locked with recovery; accepted AG-UI receipt suppresses stale refreshed history. Invalid supplied data does not disturb the answer or draft. |
| Keyboard, touch and narrow screens | Real Chrome at 1440, 390 and 320 px in light/dark themes: full text, minimum 44 px target, no clipping, focus ring, Enter/tap, uncertainty and explicit recovery. |

## Commits and integration

- `078b9bafe`: Web Style rule and append-only changelog, before UI implementation.
- `0cba6d8ea`: initial bounded suggestion and draft-preserving send.
- `be7114aea`: public API, Core/Mongo admission, SDK contracts and native provider proof.
- `111007559`: transport journals, recovery and UI integration.
- `500f5b9e4`: canonical final-answer derivation shared with ticket 09.
- `4bd1921d1`: reusable supplied-set seam and accessible state captions.
- `b927e7fbd`: accepted AG-UI refresh and explicit unsupported-server handling.
- `6a552ca05`: exact Unicode/newline/trailing-space public history proof.
- `599b1e53e`: shared send and retained-input presentation extraction to keep both
  ticket-owned integration files below the 500-line source limit.
- Merge `8e7d5ad97` includes actual integration `d0d085c90` (tickets 01, 03, 09),
  including the E0308 baseline correction and `c7452a7ed` baseline documentation.
  Shared final-answer metadata is not duplicated; operation-output metadata remains
  independent. Final checks after this merge are scoped, not a new broad-suite claim.
- Final merge `38418c436` includes integration `9a900786a` and ticket 06. The two
  overlaps retain both `latestTurnId` and `turnOutcomes`, plus both the normal send
  helper and PlanProgress. The memoized suggestion selector lives with the suggestion
  contract so the combined surface stays within its size bound.

## Validation and retained logs

Logs are retained outside the repository at
`/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/ticket11-evidence`.
The filenames below are in that directory. Early failed attempts are retained;
only the named final runs count as passing evidence. Broad Web, public/Core and SDK
runs describe the post-01/09 checkpoint `edc4f3f28`; the later 03/06 integration is
covered by the narrower final checks, not a claim that those broad suites were rerun.

| Check | Result | Log |
| --- | --- | --- |
| Full Web suite after 01/09 merge | 2,862 passed; one independently reproduced upstream AppCreation failure | `ticket-11-merged-web.log` |
| Affected Web/controller/connection/performance suite after source extraction | 197 passed in 21 files, including all 15 suggestion cases | `ticket-11-size-scoped-web.log` |
| Scoped Web after 03 merge | 37 passed in four files | `ticket-11-final-scoped-web.log` |
| Final scoped Web after 06 merge | 98 passed in ten files, including suggestions, plan progress, transcript and performance | `ticket-11-final-web-after06.log` |
| Web typecheck, ESLint and build | Passed; latest final logs retained | `ticket-11-final-types.log`, `ticket-11-final-lint.log`, `ticket-11-final-build.log` |
| E2E TypeScript | Passed | `ticket-11-merged-e2e-types.log` |
| Final real Chrome | Six passed at 1440/390/320 px in both themes | `ticket-11-final-browser.log` |
| Baseline real Chrome | Six passed at `648f9bb32`; before images retained | `ticket-11-browser-before.log` |
| TypeScript SDK full suite | 3,559 passed; typecheck, lint, build and 48 documentation examples passed | `ticket-11-merged-sdk-ts*.log` |
| Rust SDK full suite | 233 passed, including both scoped wire tests; two existing server-fixture tests ignored | `ticket-11-merged-sdk-rust.log` |
| Public/Core broad suites | 408 public and 1,145 Core passed; two independently established baseline failures explicitly excluded | `ticket-11-merged-api.log` |
| Public/Core suggestion tests after 03 merge | One public test (both transports) and three Core tests passed; 06 changes only backend tests | `ticket-11-final-scoped-rust.log` |
| Mongo transaction/concurrency and receipt recovery | One actual named test passed against the dedicated replica set | `ticket-11-merged-mongo.log` |
| Linux pinned native provider and reload | One actual named test passed | `ticket-11-merged-native.log` |
| Changed-file formatting | Prettier and Rust format passed; unrelated baseline files not formatted | `ticket-11-format.log` |
| Source size | Ticket-owned files pass; preserved upstream Agent service (508) and Console (503) fail | `ticket-11-size-final.log` |

Rust evidence uses the private target
`/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/ticket11-cargo-target`,
APFS-cloned from dependency cache and cleaned of affected workspace packages before
compilation. Every suggestion run records actual named, nonzero tests. Shared-cache
zero-test results are not counted. Builds use the pinned toolchain, two jobs,
`--target aarch64-apple-darwin`, and `RUST_MIN_STACK=16777216` for the large public
corpus. The default stack initially overflowed; that attempt is not a passing run.

The five existing conversation/MessageStream performance cases pass in the affected
suite, including 10,000 historical rows and stable feedback controls. No timeline
or Markdown-rendering implementation was changed.

### Scope of the real proofs

The public A test runs real routers, authority/admission, command worker and finite
history with an in-memory store; only the external runtime sender is controlled.
The separate Mongo test uses real transactions and competing store replicas.
Chrome runs the built application in a fresh browser context with an external HTTP
fixture. These are complementary approved seams, not a claimed deployed journey.

Linux R runs actual Codex 0.147.0 and the exact patched ACP 1.4.0 bridge, verifies
the second prompt once, closes/reopens the process and reads the native thread back.
Image `cma-ui-runtime-proof:baseline` has immutable ID
`sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`;
bridge SHA-256 is `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`.
Its external model response is a bounded fixture. This proves native prompt delivery
and persistence, not live-model suggestion quality, production packaging, deployed
authorization or a new layer4 binary. Ticket 11 changes no layer4 implementation.

Before images: `runtime-proof/ticket11-browser-before`.
Final images: `runtime-proof/ticket11-browser-final`.
Both paths are under `/Users/liuyitian/code/cma-ui-worktrees` and include the width
and theme in each case directory. Desktop, 320 px light, and 320 px dark recovery
images were visually reviewed: the region follows the answer/footer, text wraps,
and the existing draft remains visible. Upstream navigation/feedback changes are
also visible in the final images compared with the original baseline.

### Known exceptions outside ticket 11

- Public corpus `agent-skills-install-version-conflict`: expected
  `agent_operation_in_flight`, received `agent_skill_version_upgrade_unsupported`.
  Integration independently reproduced it on baseline `60fc98ee5`; our full run
  reproduces it in `ticket-11-private-api-final.log`.
- Core `installed_skill_version_change_is_reported_as_unsupported`: fixture creation
  returns `Conflict` at `live_skill_tests.rs:70`. Reproduced with one actual test in
  an immutable `60fc98ee5` archive and a separately cleaned private target:
  `ticket-11-core-baseline.log`. No unrelated test or implementation was changed.
- Web `AppCreation.test.tsx` still looks for the retired Sandboxes button at line 86.
  It fails identically on an isolated `d656e542e` archive without ticket 11:
  `ticket-11-upstream-web-baseline.log` (three passed, one failed). This is the one
  failure in the recorded post-01/09 broad Web run. The later 03 merge is validated
  only at the scoped checks above; no claim is made about that whole suite afterward.
- Baseline `cargo fmt --all --check` failures remain as supplied in
  `/Users/liuyitian/code/cma-ui-worktrees/baseline-rust-format.log`.
- Baseline `agent_operation_service.rs` remains 508 lines and byte-identical to the
  baseline reported by integration. Ticket 11 does not split or format it.
- The later ticket 03 merge also brings `web/src/Console.tsx` at 503 lines. It is
  unchanged from integration `9a900786a`; this unrelated overrun is preserved.
  Ticket-owned `AgentSurface.tsx` and `controller.ts` are 482 and 496 lines.
- Initial unrestricted Web concurrency timed out in an unrelated EventGateway test;
  the bounded rerun passed all 2,851 tests before the 01 integration merge. The later
  broad result is reported separately above. Standard Vite large-chunk warnings remain.

### Reproduction

From this worktree, use the Rust environment and private target described above:

```sh
cargo test --manifest-path api/Cargo.toml -p layer1-public -p layer2-core --target aarch64-apple-darwin --lib suggested_reply -- --nocapture
CHRONO_TEST_MONGODB_URI='mongodb://127.0.0.1:27027/?directConnection=true&replicaSet=ticket11' cargo test --manifest-path api/Cargo.toml -p cma --target aarch64-apple-darwin suggested_reply_mongo -- --ignored --nocapture
npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AgentSurface.suggestions.test.tsx
docker --context colima-cma-ui-proof run --rm --mount type=bind,source=/Users/liuyitian/code/cma-ui-worktrees/ticket-11/api/crates/layer3-adapter/src/codex,target=/candidate,readonly --entrypoint node cma-ui-runtime-proof:baseline --test /candidate/suggested_reply.test.cjs
```

`web/e2e/suggestions.config.ts` expects an isolated loopback preview on port 4362;
set `CMA_E2E_BROWSER_PATH` to `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.
The 4361 baseline preview and 4362 candidate preview are stopped after proof.
The `cma-ui-proof` VM remains running. Reusable fixture container
`cma-ticket11-mongo` (`mongo:8.0`, replica set `ticket11`) remains available through
the Mongo URI above on context `colima-cma-ui-proof`; it contains only disposable
test data, with no production connection or credentials.

## Handoff and changed areas

See [SUGGESTION_CONTRACT.md](SUGGESTION_CONTRACT.md) for the full 12/13 contract.
Ticket 12 owns editing and draft merge/replace; changed intent gets a new key and
must never overwrite an uncertain request. Ticket 13 owns generation, persistence,
cancellation and quality validation, supplying sets through
`AgentSurface.suggestedReplies`. `undefined` uses the template; `null` hides the
region. No dynamic generation or editing is claimed complete by ticket 11.

Primary paths: `web/src/agent/{suggestions,SuggestedReplies,messageSubmission}`, the
existing controller and transport journals; `api/crates/layer2-core/src/sandbox/agent_io/answer_context.rs`,
both store admission implementations and the existing AG-UI/Responses parsers;
both SDKs' answer-context and send contracts; API/Web Style docs and this tracker.
The exact ticket-owned changed-path manifest relative to the merged integration is
retained as `runtime-proof/ticket11-evidence/changed-paths.txt` under the worktree area.
Tracked source in the original and integration worktrees was not edited. No deploy,
remote publication or branch merge into the integration worktree was performed.
