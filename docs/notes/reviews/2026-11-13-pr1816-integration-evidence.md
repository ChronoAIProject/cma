# #1816 integration and review fixes

Scope: PR-A of #1478, tracked by sub-issue #1849. The parent stays open for
the hard <=60-second commit-time containment bound, remaining capacity work,
and release drills. The PR body is preserved without edits or comments; the
future merge commit closes #1849 only. No production access or activation was
performed.

## Source provenance

Original PR head: `192e7884814d062a8df92be70ba0bb2988bef33c`.
Original base: `821790b422b336684256abbf2b00d30386f40e99`.
Integrated develop: `734bc9c2ef5d3589aa25311003c69d9b94bf20a7`, the #1741 merge.
The original diff has 138 files, +6525/-197. This integration preserves the
current spend fixes, fixed-partition counter reader and default-OFF writer.
The original-head report remains in `2026-11-13-pr1816-original-review.md`.

## B1: authorization must be current when replay returns

The imported operator-STOP path called `lookup_sandbox_control(&record)` after
an awaited platform-state read. That interface had no resource authorization
proof. A share revoked during the await could therefore still recover a receipt.
The regression grants a real share, accepts a Revive, then revokes the share
through `ResourceAccessService::change_share` from inside the subsequent awaited
`admission_state` call. The expected outcome is a current-policy conflict with
no receipt, lifecycle, or event mutation.

Before the fix, integrated staged tree
`6904f78098b2956595079f87d5f0f36057bd0d85` retained the imported operator-only
lookup branch; the spend branch already used #1741's guarded reservation.
The same regression source now committed under
`layer2-core/src/sandbox/store/memory/agent_admission_tests/sandbox_controls_tests/replay_authorization_tests.rs`
ran two tests: operator STOP **failed** by returning `Ok(SandboxControlRecord)`;
spend STOP passed. Exit 100. Before-log SHA-256:
`22793f85eeb441c11176778570d9a9ce7092c4f8cfe377b8ccc172ecc123bec4`.
The before behavior is the original service's `AdmissionRefusal::Paused` branch,
which returned the result of `lookup_sandbox_control` without the bound proof.
No vulnerable source commit was added to the integration history.

The fix removes that proof-free lookup interface. Both refusal types now travel
as `Result<(), AdmissionRefusal>` through `reserve_sandbox_control`, where the
memory lock or Mongo transaction validates current resource policy before
fingerprint comparison and exact replay. New keys receive the saved refusal;
changed fingerprints conflict. The HTTP boundary maps the typed operator
refusal to 503, Retry-After 60, private/no-store.

After the fix, all 19 focused Sandbox control tests passed, including both
revocation-during-read tests, exact authorized replay, new-key refusal,
expiration, custody and membership checks. After-log SHA-256:
`aaf224c1e5a512ca845e234b96632e52193d400212fc353a7f68ce5b8d8bfa5e`.
Additional Mongo tests exercise exact operator-STOP replay, changed fingerprints,
new keys and real share revocation during the admission read. Their qualification
is recorded with the final gate results below when available.

The accepted #1741 replay decision still applies: same key and fingerprint with
current authorization recovers the durable receipt even during STOP, because it
creates no commitment or spend. The older refusal-before-replay implementation
note was superseded by the migration/idempotency contract and same-key HTTP
retry guidance, as recorded in #1741's evidence. This integration applies that
same decision to the independent operator stop.

## B2: the Rust SDK must replay the new public contract

The unmodified original-head SDK failed its existing admin corpus test with
`unmapped operator operation GET /api/v1/admin/admission` after compilation.
The original archive and log provenance are in the original-head report;
before-log SHA-256:
`1ffa884fdf2f2c2d78550b5e0d8bbe1f228d053b9b11d37d10179a993636eb1b`.

The fix adds typed read/stop/resume methods, bounded reason validation, required
bounded newest-first history decoding, and corpus dispatch. Of nineteen new
cases, seventeen exercise real HTTP and two blank-reason cases exercise actual
local preflight. The complete admin corpus passes: 41 HTTP cases and four real
preflight cases, including existing cases. Four dedicated SDK tests cover
success, UTF-8 byte limits, malformed responses, full u64 request revisions,
and typed 403/409/503 failures with exactly one request per command.
After-log SHA-256:
`4528745415a69bd4a8465b3e7fbaa75d50e6f4edd96eb8dcf16804822e8c1484`.

## Integration decisions and review coverage

- Spend refusal keeps precedence at the inherited spend boundaries. A missing
  owner valuation still refuses under active global metering; operator
  continuation cannot bypass this rule. Existing turn holds still encounter
  STOP after owner policy removal.
- Admission reads derive metering state from the current clock/period and then
  overlay independent operator control. Missing/stale evidence cannot reopen
  spend, and an unreadable operator field blocks only fresh work. The imported
  unreadable-control test now queries the fixture's current clock rather than
  an out-of-period literal; its refusal/settlement assertions are retained.
- Runtime recovery continues already admitted work under operator STOP, while
  #1741's spend gate still blocks the recovered runtime liability. The failed
  platform-pause regression retains its spend refusal and now keeps operator
  STOP active when testing successful recovery after the spend lift.
- Fresh/continuation causes are server-attributed. Reviewed Manual-origin
  validation, binding fences, durable Launch/lifecycle identities, verified
  external-media parent receipts and exact replay ordering. Controls and reads
  remain available; retirement creates a tombstone directly without first
  admitting a provisionable binding conversation.
- Reviewed admin capability checks, independent CAS, concurrent duplicate stop,
  resume revision, bounded strict history, immutable audit writes and uncertain
  commits. Unknown commit results never rerun the transaction body. Shared
  settings replacement preserves the independent control/history fields.
- Reviewed REST, AG-UI, Responses, registration and Event Gateway projections;
  public refusals carry fixed text, without operator reason or identity. The
  public contract inventory, OpenAPI registration and both SDKs are included.
- `platform_settings` is already Independent in the merged retirement census;
  the existing filter excludes both singleton control and history records.
  No extra census rule or collection is introduced by #1816.
- Both `CHRONO_ADMISSION_CONTROL_WRITES_ENABLED` and
  `CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED` remain false by default in code and
  infra. Stored stops and previously activated counter layouts remain readable.
  The counter's #1741 measurements remain admission 97 -> 114 ms with doubled
  reply bytes versus settlement p95 65 -> 14 ms; activation is a later measured
  decision, not part of this PR.
- The next-snapshot scope is explicit. Plain majority-read/write and zero-
  liability snapshot paths have no proven hard late-commit deadline. Capacity
  and release qualification remain tracked by the open parent issue.

## Preliminary local qualification

All heavy commands used the shared build-slot wrapper; Rust used
`RUST_MIN_STACK=33554432`. Source changes after these runs receive final checks
before a GO verdict or push request.

| Gate | Observed result | SHA-256 of private log |
| --- | --- | --- |
| Affected backend nextest: cma, layer1-public, layer2-core | 4,141 passed; 1,305 ignored/filtered; 67.163 s test phase | `347e0bba1b3124038b01b6c40b6cad70e963c768cabe22799caeb8e606be0d31` |
| Rust SDK fmt, strict clippy, all-target build, nextest, doctests, dry-run example | 280 tests passed; 2 skipped; 37 doctests passed | `57b6b31a0bab5eaf450587baa2a9225e430a51a162b83cccf704b4066707d7f9` |
| TypeScript SDK lockfile install, format, lint, typecheck, coverage, build, docs, package consumer, audit | 4,471 passed; 1 skipped; 96.53% lines; no high/critical audit findings | `328dbb5ec18ee65d0e0712ca43f7e8ee8068604f148000b1c120011b4120761a` |

Mongo contracts and final backend lint/doc checks are pending at this evidence
checkpoint. No integrated-head GO is claimed here. Retargeted GitHub head is
still the original head and currently reports zero checks; those are not CI
qualification for this local source.
