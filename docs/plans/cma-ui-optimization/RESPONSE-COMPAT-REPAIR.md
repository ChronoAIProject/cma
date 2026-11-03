# Tickets 14/19: legacy Responses output compatibility repair

2026-11-03. Bounded repair on `codex/cma-ui-response-compat-repair`, based on
`a2965df09581de5fa0fa35af54b69db56cd83ab9`. This closes the specific legacy wire
defect identified in the external compatibility inventory; it is not another
ticket or completion of ticket 19's broader acceptance criteria.

## Actual red and green

The new Rust regression enters `POST /api/v1/responses` through the real router,
authorization, command admission/worker, durable update recorder and stream read
loop, using the existing in-memory store and a controlled runtime input port.
It records a command, then output without a status, then later completion.
Before the fix, its missing `response_id` assertion fails (1 failed test).

An isolated loopback server exposes that same handler to the unchanged typed
TypeScript SDK. The red HTTP capture contains a `response.cma_chat.tool_activity`
frame with only `call_id` and `output`. The SDK test fails, and independently
bundled current and baseline `c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706` readers both
reject those actual bytes with `response_id: expected a string`.

The stream now keeps each turn's recorded tool item by call ID and emits the
existing `response_id`/`item` envelope. Output preserves the recorded item ID,
kind, target and status; later status/title updates use their recorded values.
Omitted output stays absent until reported, and later updates without output
preserve earlier output. Output without any known activity context is skipped,
matching conversation-item projection, because no status may be invented.

The fixed live SDK test passes. Both current and baseline readers accept all
12 captured replay frames, including three correlated activity frames with
statuses `in_progress`, `in_progress`, `completed`. The four parser/schema source
files (`events.ts`, `stream.ts`, `types.ts`, `schema.ts`) are byte-identical to
baseline. No SDK production source or parser acceptance rule changes.

The test disconnects after output, retrieves the still-running durable response,
then records completion and explicitly replays the same request/key. Public
reads and replay show the same response, one runtime dispatch, and retained
output. Retrieve continues to expose the durable assistant response; tool output
is read through conversation items. Legacy cursor resume remains a 422 refusal.
No stream protocol, default transport or v2/replay implementation changes.

## Validation and limits

- `cargo test --locked -p layer1-public --lib responses::`: 56 passed,
  1 ignored loopback serving fixture; the two new regressions run normally.
- Existing default/opt-in output-boundary regression: 1 passed.
- SDK `responses-stream.test.ts` and `responses.test.ts`: 49 passed.
- SDK `responses-output-compat.test.ts` against the loopback server: 1 passed;
  its companion Rust serving fixture also exits successfully after SDK cleanup.
  This integration test skips without `CMA_RESPONSE_COMPAT_URL`; a skipped run
  does not count as its proof.
- SDK typecheck, focused ESLint, Prettier, changed Rust formatting and diff
  whitespace checks pass. All changed sources are below 500 lines; recognized
  test files are below 2,000. Whole-tree pre-existing size failures are recorded
  separately and are not repaired here.

Rust 1.96.1, two jobs, private Cargo target under the external evidence directory.
The integration cache was APFS-cloned, then `layer1-public`, `layer2-core` and
`layer3-adapter` were cleaned in that private target before the first build.
No integration/original target was written or used to execute these tests.

The known command text/output is a controlled recorder input, not a newly run
native shell command. This proves actual public HTTP and old-reader wire
compatibility, not native reporting completeness, Mongo restart, old-binary
schema rollback, browser layout or deployment. Existing skill-fixture/full-format,
508-line service and LocalAgentV2 head-404 findings remain outside this repair.
No UI changed, so `Web_Style.md` is unchanged. No push, PR or deployment occurs.

## Evidence and reproduction

Evidence root:
`/Users/liuyitian/code/cma-ui-worktrees/evidence/response-compat-repair`.
`manifest.json` records commits, source identities, commands, counts, limits and
SHA-256 hashes. The load-bearing artifacts are:

- `rust-red.log`, `sdk-red.log`, `parsers-red.log`, `red/http-stream-0.sse`;
- `responses-tests.log`, `default-output-boundary.log`, `sdk-responses-tests.log`,
  `sdk-green.log`, `parsers-green.log`, `final-server.log`;
- `green/http-stream-0.sse` (disconnect prefix), `green/http-stream-1.sse`
  (completed replay), `green/decoded-events.json`, `green/public-reads.json`;
- `baseline-identity.json`, `verify-parsers.mjs` and the two bundled readers;
- formatting, lint, typecheck and file-size logs, plus merge/checkpoint metadata.

Run the ignored `legacy_responses_compat_server` Rust test with
`CMA_RESPONSE_COMPAT_ADDRESS_FILE` pointing outside the repository. It binds only
`127.0.0.1` on an ephemeral port and writes the URL to that file. Pass that URL as
`CMA_RESPONSE_COMPAT_URL` and an external capture directory as
`CMA_RESPONSE_COMPAT_EVIDENCE` when running the SDK compatibility test. The SDK
test shuts down the fixture in `finally`. Always use a private Cargo target.

Repair-owned files: `responses/stream.rs`, `responses/mod.rs`,
`responses/stream_live_tests.rs`, `responses/output_compat_tests.rs`,
`responses/output_compat_tests/fixture.rs` under `api/crates/layer1-public/src/`;
`sdk/typescript/tests/responses-output-compat.test.ts`; and this document.
The final handoff retains this worktree; the separate merger owns integration.
