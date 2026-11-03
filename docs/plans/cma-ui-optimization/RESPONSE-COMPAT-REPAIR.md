# Tickets 14/19: legacy Responses output compatibility repair

2026-11-03. Branch `codex/cma-ui-response-compat-repair`, base
`a2965df09581de5fa0fa35af54b69db56cd83ab9`. This repairs the external inventory's
specific legacy wire defect; ticket 19's broader acceptance remains separate.

## Red → green and scope

The new regression enters `POST /api/v1/responses` through the real authorization,
command admission/worker, durable recorder and stream read loop. It records a
command, output without status, and later completion. Before the fix the Rust
test fails on missing `response_id`. A loopback HTTP test with the unchanged typed
SDK also fails. Independently bundled current and baseline
`c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706` parsers reject those captured HTTP bytes:
`response_id: expected a string`.

The server now retains each turn's recorded activity item by call ID and emits
the existing `response_id`/`item` envelope. Output preserves item identity, kind,
target and recorded status. Omitted output stays absent until reported, and
later updates without output preserve it. Output with no recorded activity
context is skipped, matching conversation-item reads; no status is invented.

Both parsers accept all **12 actual green replay frames**, including three tool
frames with statuses `in_progress`, `in_progress`, `completed`. The SDK test
disconnects after output, retrieves the still-running response, records later
completion, and explicitly replays the same request/key: one runtime dispatch,
same response, retained output. Retrieve still returns the durable assistant
response; conversation items carry tool output. Legacy cursor resume still
returns 422. `events.ts`, `stream.ts`, `types.ts` and `schema.ts` are byte-identical
to baseline; no SDK production code, default transport or v2/replay code changes.

## Validation and boundaries

- Rust `responses::`: **56 passed**, 1 ignored serving fixture; both new
  regressions run normally. Existing default/opt-in output-boundary test: **1 passed**.
- SDK existing Responses tests: **49 passed**; actual loopback SDK test: **1 passed**.
  The Rust serving fixture also completes successfully after SDK cleanup.
- SDK typecheck, focused ESLint/Prettier, changed Rust formatting and diff checks
  pass. Repair sources stay below 500 lines and recognized tests below 2,000.
  Whole-tree size findings are recorded in `file-size.log`, outside repair scope.
- Rust **1.96.1**, jobs **2**, private Cargo target in the evidence directory:
  APFS-cloned cache, then private clean of `layer1-public`, `layer2-core`,
  `layer3-adapter` before building. Actual nonzero test counts are in the logs.

The store/runtime input port is controlled; this is actual public HTTP/old-reader
proof, not a new native command run, Mongo restart, old-binary schema rollback,
browser proof or deployment. Known skill/full-format, 508-line service and
LocalAgentV2 head-404 issues remain untouched. No repair UI or `Web_Style.md`
changes. Integration `7cd701d0fe6c2496256a01eca59919ee8d899c82` merged cleanly into
this branch; its preview changes do not alter the validated API/SDK sources.

## Evidence, reproduction and file manifest

External root: `/Users/liuyitian/code/cma-ui-worktrees/evidence/response-compat-repair`.
`manifest.json` pins source/merge identities, commands, counts, limits and SHA-256
artifact hashes. Read `rust-red.log`, `sdk-red.log`, `parsers-red.log`,
`red/http-stream-0.sse`, the green test logs, `green/http-stream-{0,1}.sse`,
`green/public-reads.json`, `baseline-identity.json` and `parsers-green.log`.

Run ignored Rust test `legacy_responses_compat_server` with an external
`CMA_RESPONSE_COMPAT_ADDRESS_FILE`; it writes its ephemeral loopback URL. Run SDK
`responses-output-compat.test.ts` with that URL as `CMA_RESPONSE_COMPAT_URL` and an
external `CMA_RESPONSE_COMPAT_EVIDENCE` directory. The SDK test shuts down the
fixture in `finally`. It skips without the URL; a skipped run is not proof.

Seven repair files: `responses/{stream.rs,mod.rs,stream_live_tests.rs,
output_compat_tests.rs,output_compat_tests/fixture.rs}` under
`api/crates/layer1-public/src/`; `sdk/typescript/tests/responses-output-compat.test.ts`;
this document. Keep this worktree for the separate merger. No push, PR or deploy.
