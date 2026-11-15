# PR #1741 corrected candidate review

Candidate source: `095cbab547eeec8e29a97f69822cd1e1f96c3817`. Final integration qualification passed.
Base: `3bf6b51cf95a249ce277ea01b47fd12d539312b8` (`develop`).
PR: https://github.com/ChronoAIProject/cma/pull/1741
Review date: 2026-11-13. Final integration qualification: passed.

The original full review at `821790b422b336684256abbf2b00d30386f40e99`
found eight distinct blocker groups and remains recorded in the
[original primary review](2026-11-13-pr1741-primary-review.md). It includes all
126 original changed files and the full 11-file `8d318edc..821790b4` delta from
the recovered security review. The unpublished primary report was not used.
This continuation reviews every correction, their transaction and caller
boundaries, subsequent develop integration and the composed census reader.
The corrected source diff against the base above contains 163 files; its SHA-256
is `6c2da2e4cd60f4f6a73964dfa06744a87d522b155da3251eabd960827751927e`.

## Corrected behavior and review

All eight blocker groups have corrections and before/after evidence in the
[correction record](2026-11-13-pr1741-fix-evidence.md) and
[counter evidence](2026-11-13-pr1741-counter-evidence.md).

| Finding | Result | Fix commit |
| --- | --- | --- |
| B1 stale evaluator/refusal closure | Current policy and accounting revisions fence STOP entry; stale lift, correction, release and policy evidence cannot reclose admission. | `9727d6d81` |
| B2 cached STOP | The cache schedules evaluation; new cancellation receipts and platform pauses require current transactional authority. | `07bdc3b08` |
| B3 missing metering evidence | Current policy, clock and period evidence derive refusal without waiting for the evaluator. | `04818783e` |
| B4 absent owner valuation | Active global metering refuses unvalued turns and auxiliary requests before upstream dispatch. | `1b05f4506` |
| B5 recovery during STOP | Unpublished failed-runtime recovery refuses reopening and preserves retained work until lift. | `af169e314` |
| B6 shared period writes | Optional bounded counters remove routine root writes; exact-sum admission retains the settings fence. | `c5e64f502` |
| B7 queued retained hold | Every reserved queued turn checks platform STOP, including after owner policy removal. | `25adbd3cd` |
| B8 exact-key Revive recovery | Current resource authorization and fingerprint checks precede new-commitment refusal. | `88d0bdb9b` |

Exact Revive replay with the same key and fingerprint returns its durable
receipt during STOP after current resource authorization is rechecked. A new
key is refused and a changed fingerprint returns idempotency conflict. Tests
cover both stores, revoked membership, unchanged durable records and the real
Mongo service boundary. The lead confirmed this interpretation of the migration
contract and same-key HTTP guidance; historical implementation NOTES round 3/R7
is explicitly superseded. Returning an existing receipt creates no commitment.

The distributed layout serializes increases in committed accounting on the
existing settings document. Admission reads the exact sum of sixteen bounded
counters. Normal settlement and release can only preserve or reduce that sum;
late usage and cloud corrections also take the settings fence. Closure
persistence validates policy, root and all contributing counter revisions with
actual writes, as required by MongoDB snapshot/write-conflict semantics.

Conversion commits atomically with successful admission using pre-admission
balances. A refused first admission uses committed evidence; abort leaves no
partial layout. Legacy balances and holds use partition zero. New holds bind
to partitions 1–15 and retain their original period and partition across
restart, idempotent settlement and the writer switch being OFF. Old inflight
root writers conflict with conversion. Readers reject unsupported markers,
nonzero converted root balances, missing/malformed counters and mismatched
bindings. The final regression `4b95b1f70` exercises malformed durable evidence.

The census extension `7ce0e2bec` composes with #1754's classification source
`49860d93` through `dc23792a3`; the full converted-layout fixture is
`e36a272d9`. Periods, observations and counters remain independent accounting
regardless of incidental ownership fields. STOP receipt ownership follows its
exact Agent key segment. Counter keys require canonical nonnegative BSON
signed-integer seconds and partitions 0–15. Malformed, padded, signed,
overflowing and out-of-range keys refuse capture. Reservation bindings restore
byte-exactly. The catalogue/hash, indexes, validators and TTLs are unchanged.

The earlier normal develop merge `29db7be80` includes #1754's merged history
`dd528496831e68ee008d38e2f81137b97830b486` and #1722's OAuth correction. The
merge was conflict-free. Reviewed #1722's authentication composition, credential
verification, session capability projection and command authorization overlap:
the resource-scoped token parser is only a negative eligibility filter; current
provider evidence grants authority. Stores still validate current resource proof
before exact receipt replay, and composition retains the platform-spend source.
No accounting, replay fingerprint, counter selection or STOP fence was lost.
No additional supported-path blocker was identified.

Develop advanced again while the first full integrated gate ran. Merge
`e80ef5a7b` incorporates develop `2abe8f22e`, including worker supervision
(#1755), authorization snapshot/enrichment switches (#1831), observation
retention (#1735) and native recovery transport (#1784). The sole content
conflict concerned imports/constants moved by independent Mongo transaction and
topology extractions; each remains owned by its module. Commit `ae7012307`
extracts the unchanged permission flag tests before composing the additional
feature gates, preserving their module path and assertions within the source
line limit.

Reviewed the composed authorization path: optional snapshot sessions affect
initial reads, while the committing proof check still reads current authority
inside its transaction. Reconciliation supervision retains the owned task set
and its durable fences across scheduler recovery; the platform-spend sweep and
STOP checks remain wired. Observation retention preserves the same usage fields
inside an accounted retained effect. Native recovery proof is independently
default OFF, binds the original runtime/open identity and an issued observation
revision, and does not send conversation input or reopen a runtime as part of
its proof. No supported-path spend blocker was found in these composition
boundaries. The complete checkpoint included Layer 4 for that runtime delta.

Merge `095cbab54` incorporates develop `3bf6b51cf` without conflicts after the
complete backend gate at `e80ef5a7b`. It adds #1835's default-OFF reconciliation
pacing and #1737's native identity preflight. Reviewed all production changes,
their tests and composition with spend enforcement. Pacing hints only suppress
repeated folds of already-dispatched commands with unchanged durable evidence;
missing, malformed or unavailable hints fall back to full reconciliation.
Fresh admission, reserved queued turns and the independent STOP cancellation
path retain their authority checks. Actual reconciliation still validates the
complete durable evidence; its success counters record only committed outcomes.

The native identity change applies the existing byte/control-character rule to
the borrowed fresh identity before copying it when JSON retention bounds are
on. The final activation guard still validates both flag modes. Accepted
identities, fixed errors, startup receipts and the absence of a fabricated ACK
are preserved. These changes introduce no new spend policy or accounting format.
The final integration gate covers the affected control-plane/adapter suites and
Mongo reconciliation, spend, counter, census and replay interactions. SDK,
Layer 4 and the complete Mongo harness have unchanged tree identities.

## Measurement and deployment limits

`CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED` stays **default OFF** in code and the
ConfigMap. The first measured concurrent admission p95 increased
**96.564 → 114.219 ms**, with reply bytes **82,140 → 168,090** over 30 admissions;
settlement p95 improved **64.965 → 14.338 ms**. Each converted accounting phase
had zero root writes and every phase had zero collection scans. The shared-host
repeat again showed admission cost and settlement benefit. Those first two samples and the
reproducible harness are in the committed counter evidence. The final source
sample measured concurrent admission p95 **63.224 → 55.244 ms** and settlement
**30.606 → 9.797 ms**, with the same doubled admission reply payload. The
changed admission timing direction confirms host variability; it does not erase
the earlier regression or establish production readiness. These measurements
cover internal accounting, not full provider throughput.

#1754 source and merged history are integrated. Compatible serving, census and
rollback artifacts must still be deployed before conversion. Strict predecessor
readers reject the new layout/binding fields; switching OFF does not undo a
converted period or make those artifacts eligible. Recovery requires the root,
its counters and retained reservation bindings together. Activation remains a
later measured decision. Gateway usage and operational drills remain release
owner evidence requirements. The original admin spend OpenAPI/corpus omission
remains an explicit nonblocking documentation follow-up. No production access,
deployment or activation was performed.

## Qualification

The complete backend gate ran at `e80ef5a7b1417e1cf124053f0477bd0339762ba0`,
after integrating #1754's merged history and develop `2abe8f22e`. Its wrapper
exited **0**, including disposable fixture cleanup and unchanged-HEAD checks.
The subsequent develop merge is qualified separately at the candidate source
above; the complete earlier run is not attributed to that later API tree.

All heavy commands used the shared build-slot wrapper. Rust/Mongo commands used
`RUST_MIN_STACK=33554432`. Mongo used disposable loopback fixtures with the
repository-pinned MongoDB 8.0.13 image and cleanup traps. Test counts come from
suite summaries, without double-counting duplicate success-output lines.

| Scope and source | Result |
| --- | --- |
| Ordinary backend, five crates at `e80ef5a7b` | **5,575/5,575 passed**, 1,264 skipped |
| Complete repository Mongo script at `e80ef5a7b` | **1,092/1,092 passed**, 28 ordinary partitions plus four isolated fixtures |
| Five-crate all-target/all-feature clippy at `e80ef5a7b` | Passed, warnings denied |
| Doctest compilation and workspace Rust formatting at `e80ef5a7b` | Passed; zero runnable doctests |
| TypeScript SDK at `29db7be80`, unchanged source tree at candidate | **4,441 passed**, one existing skip; **96.51% line coverage** |
| Ordinary backend, four affected crates at `095cbab54` | **4,957/4,957 passed**, 1,263 skipped |
| Focused Mongo integration at `095cbab54` | **91/91 passed**, including new reconciliation and spend/counter/census/replay cases |
| Five-crate all-target/all-feature clippy at `095cbab54` | Passed, warnings denied |
| Doctest compilation and workspace Rust formatting at `095cbab54` | Passed; zero runnable doctests |
| Source size, deployment keys and configuration documentation at `095cbab54` | Passed; 6,580 files; key counts 162 / 185 / 12 |

The SDK also passed `npm ci`, lint, formatting, typecheck, build, all 54 examples
across 11 documentation files and isolated package-consumer checks. Its tree
`3eec4d8afd915ebbc5b38933158aaad35e1ed571` is byte-identical at the candidate.
The sole existing skip is `responses-output-compat.test.ts`, which requires
`CMA_RESPONSE_COMPAT_URL`; no skip was introduced. Ordinary Rust skips include
ignored Mongo contracts executed by the complete repository script. The tested
Layer 4 tree `418d8fab4be22c6bfba6c8fdcd523af5dbdb2ca0` is also unchanged.

The Mongo selector harness passed 40/40 at the preceding integrated checkpoint;
its source is unchanged through the candidate. Earlier complete Mongo runs at
`4b95b1f70` and `29db7be80` each passed 1,082 contracts across 31 fixtures with
wrapper exit 0. Earlier baseline and intermediate failures remain recorded in
the correction evidence; no assertions were loosened, skipped or retimed.

The full qualification commands from `api/` were:

```sh
cargo nextest run -p cma -p layer1-public -p layer2-core -p layer3-adapter -p layer4-runtime \
  --locked --test-threads 4 --no-fail-fast
cargo clippy -p cma -p layer1-public -p layer2-core -p layer3-adapter -p layer4-runtime \
  --all-targets --all-features --locked -- -D warnings
cargo test -p cma -p layer1-public -p layer2-core -p layer3-adapter -p layer4-runtime --doc --locked
cargo fmt --all --check
bash ../.github/scripts/mongo-contract-tests.sh
```

For the final integration, ordinary tests and doctest compilation cover `cma`,
`layer1-public`, `layer2-core` and `layer3-adapter`; clippy still covers all five
crates. The focused Mongo run uses the same pinned, disposable fixture and:

```sh
cargo nextest run -p cma --locked --run-ignored only --test-threads 1 \
  --no-fail-fast --success-output final \
  -E 'test(reconcile_pacing) or test(store::budget::) or test(platform_) or test(sandbox_controls_tests) or test(request_budget)'
```

The final integration wrapper exited **0** after all 91 Mongo contracts, fixture
cleanup and unchanged-HEAD checks. Its manifest records every stage as zero.
The five new reconciliation Mongo cases also passed their existing bounds,
including terminal settlement within five seconds and the scan-count reduction.
No failure was hidden by a rerun.

| Supporting log or manifest | Source | SHA-256 |
| --- | --- | --- |
| Full ordinary backend log | `e80ef5a7b` | `1e100a6d71fa73215e5d952d661c10c3fe9c3f1c7398a2c84ab39f9fe0da99dd` |
| Full clippy log | `e80ef5a7b` | `653fad45f49674d841237d4dab4a7716e9c00fed081e9d8f431122fd988f064f` |
| Full doctest log | `e80ef5a7b` | `6d107c3a564facd3481aea9a91d102114c935bad3f7c6e26439d89e5817144c3` |
| Full Mongo log | `e80ef5a7b` | `9b421bc89c41fe7508d7c8a32a64825558ba1ea938eaa856d30e1bde24bbfae2` |
| Full backend manifest | `e80ef5a7b` | `bf8e0d0b335ac1ef481c78469e77ce69b9456824653edcc7529bea7c1f79ba46` |
| SDK log | `29db7be80` | `e9c50aa4d2b759f34e2963652adb1e8fee25a45306c85acd345b6e500ef7dd37` |
| Final ordinary backend log | `095cbab54` | `3b0a40138f4cd26ca24e8c10fdf8709f0c936396dc8f7e72f3a31fd823eaa2bb` |
| Final clippy log | `095cbab54` | `22f575102c8e9e85fa8c1b2989fc0ebf9203ac4f6e210240d70da8374adac8a3` |
| Final doctest log | `095cbab54` | `df2b47fcb9f06788b216f4f7d35d4e801ee9e6a53976abe92b4362ab829716c7` |
| Final Mongo integration log | `095cbab54` | `b304104d69f7ee013f5d6c1642fb5b6163732b6d775f29ba44e45813f314b4eb` |
| Final integration manifest | `095cbab54` | `f5311b256510e047ad7798ca7643b04fc71be4fc2ced0603adbbb5d9b5d9bc04` |

Raw fixture logs remain private supporting material. Conclusions, test names,
counts, measurements and fingerprints are committed here and in the linked
reports. The earlier remote CI run is distinct: head `821790b4` has 23 passing
and two failing checks (Mongo terminal-settlement transaction count and its
Backend aggregate). Develop's inactive-adoption correction resolves that
interaction; the unchanged settlement assertion passed in the complete local
Mongo run. CI has not run on this unpublished candidate.

## Evidence commit boundary

The final report and PR appendix are documentation-only descendants of the
qualified source. The final candidate's code/configuration/harness trees must
match these tested tree IDs; the publication request records that final SHA.
No earlier test result is attributed to a different code tree.

| Tree | Object ID |
| --- | --- |
| `api` | `d077059cedd5b68932fbb5315cbde4c3e43e43ee` |
| `sdk/typescript` | `3eec4d8afd915ebbc5b38933158aaad35e1ed571` |
| `.github` | `502b31ebc528b5884fbd8df1f66b8db219701ea8` |
| `infra` | `1c514df0214e6e48dede3b765ea5c78d5e259af4` |

VERDICT #1741 @ 095cbab547eeec8e29a97f69822cd1e1f96c3817: GO
