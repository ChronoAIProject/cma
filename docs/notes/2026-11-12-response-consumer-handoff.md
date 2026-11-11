# Segmented response-consumer handoff

Part of #1349 and #1674; response-consumer half of segmentation P1-4.
The authorized vehicle is the combined default-off segmentation PR. This slice
does not open an independent PR or authorize writer/recovery activation.
It supplies the response-side correction for pre-review P1-4 at `34bcc3667`.
The transcript-side lifetime traversal and its parity matrix remain in the
transcript slice; this handoff does not close the whole combined finding.

## Source and dependencies

Branch: `tl/cma-log-segments/response-consumers`.
Implementation pin: `b9d390634` (response behavior through `fbce2aa87`;
explicit compatibility integration correction in `b9d390634`).
Latest normal develop sync: `8167d01d896bd9d6f6ceba21a20a9e145ce059ac`
at `f389f41c8dc8b0c5f5594d5b10fa52b437bc0678`. It retains all 24
configuration fields/parser assignments and every test from both parents,
including default-off hierarchy batching and its snapshot prerequisite.
Develop `1d425cffc6e48bf33cc26635b6e1a58f92aec79f` is normally merged at
`9b82a99e112a62252b0e6f869100e6e860385b31`. Both parents' independent rollout
flags, configuration tests and bounded live-delivery behavior are retained.
The quota flag/parser/test omission found by the first CMA compile was repaired
in `11c936156`; all develop flag and test declarations are now retained.

| Dependency | Consumed pin and contract |
| --- | --- |
| S1 capture, audit and schema | `e955d37f1ede36e7303c8ddaa7ace01ce46efbcb`; snapshot-scoped capture/points, certified audit evidence, exact index inventory. |
| Gate/admission/output schema | `109016ed5d3294127a6718b283245886d4ab3cd7`; control validators and numeric `agent_log_output_change_seek`. |
| Recovery carrier schema | `8ba9d55215b1eeb9dd56f73e27817dc2f967266a`; frozen format-2 schema and each discovery index's own uniqueness contract. |
| S2 response pages | `0cd56bd745e4eb4202b72027360cdfb1d0896ea4`, merged at `60e1b2f67`; authorized command polling, deferred source observation and root registration. |
| S4 writer/output contract | `216fc0a59654d086e32e0d82be5533f5196d440c`, merged at `4d2662ae0`; scalar turn facts, immutable output changes and current tool references. |
| S4 historical response facts | `99f9826f4b98ff3784c03492ce877f883700526b`, consumed through `91b487f575dfdf60758bb4d211324a7036f4f5c5`; historical model knowledge, post-terminal usage and exact deferred-failure source references. |

The capture port lives in `layer2-core/src/sandbox/store/agent_response_log/indexed.rs`.
Core observations and validation live in `agent_reads/response_indexed*.rs`.
Mongo implementation lives in `cma/src/store/segment_response/indexed/` and its
parent module. The borrowed reader keeps one original snapshot for metadata,
numeric change seeks, tool state and original source-point proofs.

Public v2 consumers use `responses/v2/indexed_projection*`, `indexed_stream.rs`
and `routes_output.rs`. Legacy consumers use `responses/legacy_indexed.rs`,
`stream_indexed.rs`, retrieval and stream dispatch. Exact allocated paths remain
in the shared ownership manifest and its 01:55 allocation history.

## Delivered behavior

- Already-adopted records select the certified reader before optional legacy
  optimization flags. Unsupported or damaged adopted evidence is an explicit
  read failure; it never selects a lifetime fallback.
- V2 response objects and immutable replay use historical facts and original
  rows. Later tool output updates current items without renumbering an earlier
  terminal event. A deep replay cursor uses logarithmic counter probes and an
  ordered numeric seek.
- Small requested results remain readable beyond both lifetime row/byte limits.
  Oversized complete results return `response_pagination_required`; bounded
  item pages retain every output item instead of reporting successful truncation.
- V2 `/output` returns a format-specific continuation. Legacy retrieval accepts
  `output_page=true` and the returned `output_after` message ordinal. An empty
  change-only v2 page can still have `has_more`; clients follow its cursor.
- Each public page selects one item/change group. Metadata is separately
  bounded, source-point batches are bounded, and a single oversized item is an
  explicit error. No whole-turn vector is constructed for indexed observation.
- Original authorization is retained through dependency reads and rechecked
  after them and before releasing buffered frames. Purge, share revocation,
  deadline expiry and malformed coordinates cannot release a stale result.
  Soft-deleted retained history remains readable until actual purge, matching
  the existing resource contract.

Legacy live streaming preserves intermediate tool updates through bounded
original-row pages over the requested turn. It still traverses that turn and
does not claim fixed work for sparse long turns: the existing immutable output
index records first-terminal changes, not every intermediate legacy update.
Its tool state and event groups are bounded; observation-limit errors are
explicit. Indexed retrieval and v2 replay avoid this source traversal.

## Qualification

Each heavy phase ran from an immutable archive with a recorded content hash,
under an existing shared lock with a blocking wait and two Cargo jobs. The slot
was released between phases. Inventories require the new tests to be present;
workspace package artifacts are cleaned where required to prevent stale reuse.

| Gate | Tested source | Actual result |
| --- | --- | --- |
| `cargo test --locked -p layer1-public responses:: --lib -j 2` | `fbce2aa87` | 104 passed, 0 failed, 1 ignored; 504 filtered. The ignored entry is the SDK loopback-server fixture. |
| Strict core/public Clippy, all targets/features, `-D warnings` | `fbce2aa87` | Passed. |
| `cargo test --locked -p cma --bin cma --all-features config::features:: -j 2` | `11c936156` | 14 passed, 0 failed; 2,566 filtered. Includes the restored quota test and independent default-off flags. |
| Actual Mongo response-reader contracts, all features, ignored tests explicitly selected | `11c936156` | 5 passed, 0 failed; 2,575 skipped. Includes post-capture evidence/source mutation. |
| Strict CMA Clippy, all targets/features, `-D warnings` | `11c936156`, `f389f41c8` | Neither final attempt started Cargo: disk preflight refused. Prior source had 22 binary and 20 test-target shared integration diagnostics. No newer pass is claimed. |
| Configuration delta after hierarchy batching | `f389f41c8` | Compilation interrupted by the shared disk floor guard before any test executed. |
| Compatibility correction, config delta and strict CMA | `b9d390634` | Pending safe disk capacity and a shared build slot; new tests are not yet execution evidence. |
| Formatting, whitespace and canonical source-size check | `11c936156` | Passed; 6,152 source files checked. Response-owned changed files are below 500 lines. |
| Mongo selector regression suite | `9b82a99e1`, selector/runner files unchanged afterward | 31 passed, 0 failed; mocked orchestration tests, separate from actual Mongo. Both lists have the same 70 prefixes in the same order. |
| Canonical full-history secret scan | `11c936156` | Passed; 3,828 commits scanned, zero findings. |
| Configuration documentation selection | `11c936156` | Inherited parent-readiness pacing reference still needs the combined integrator's existing `ce9da2336` correction. |

The only source delta from `fbce2aa87` to `11c936156` is
`cma/src/config/features.rs`: the develop quota flag/parser/test restoration.
The core, public and response-reader source bytes are identical. Their passing
results retain their original tested SHA; the corrected CMA binary and Mongo
suite were compiled/executed at the newer head. All recorded frozen-source
checksums remained unchanged. The passing `11c936156` archive SHA-256 is
`edb195ee4348e7c0d9c9806418f4dd6562450a730afa4aae5aeb3447c857eb29`.
The interrupted `f389f41c8` archive remained unchanged at
`e8d99e7355b06ccf4ec09c9dac301948d71df97ba1d996aa91f3696f2e29f414`.

The first Mongo build at `fbce2aa87` failed before executing tests on the missing
configuration field. It is not counted as Mongo evidence. The corrective gate
compiled CMA, required the quota test in the inventory and passed all 14 config
cases before allocating the final Mongo fixture.

Public regressions cover exact replay suffixes, empty and nonempty messages,
interleaved/duplicate/terminal-at-start tools, latest output, historical model,
post-terminal usage and next-turn cutoff, deferred failure, long-turn deep seek,
oversized full results with successful pages, HTTP routes, retained-reader flags,
original-source corruption, share revocation, purge and nonrenewing deadlines.
The real snapshot regression changes evidence and an unread sealed original
after capture: the held reader remains coherent and a fresh reader refuses the
damaged evidence. It passed alongside all four earlier reader contracts.

Mongo fixtures use the real canonical schema and audit rebuild/install path.
Synthetic source rows and seals establish controlled histories; this proves
reader/audit interoperability, not qualification of ordinary S4 publication.
Writer publication and combined authority/migration evidence remain separate gates.

Five samples at each size read a fixed requested result with a fixed eight-row
active unit. The largest fixture exceeds 10,128 entries and 4 MiB. Every sample
consumed three source operations, 13 source rows and 6,408 source BSON bytes.
The profiler required numeric indexed seeks and rejected collection scans,
blocking sorts, writes and transaction-control commands.

| Sealed units | Checkouts / commands | Reply bytes | Documents examined across five samples | p50 ms | p95 / max ms |
| --- | --- | --- | --- | --- | --- |
| 1 | 24 / 24 | 30,690 | 135 | 19.703 | 26.091 |
| 10 | 26 / 26 | 33,623 | 145 | 35.734 | 44.123 |
| 100 | 26 / 26 | 33,624 | 145 | 48.198 | 62.994 |
| 1,000 | 26 / 26 | 33,625 | 145 | 49.527 | 78.708 |

These local measurements exclude public authorization and command reads. The
small sample on the shared machine establishes bounded query/byte work; it is
not a production-latency or complete-route savings claim. Latency is reported
as observed, without claiming it is constant across history sizes.

## Explicit compatibility correction for the combined tree

The merge preview against combined `efc1fd426` confirms a real conflict in
`cma/src/store/agent_log_reads.rs`. Resolve its segmented arm by calling
`segmented::read_compatibility_window`, with the new child module registered.
Do not retain the combined tree's page loop that accumulates an unlimited vector.
This is an explicit replacement of that integration behavior, not a claim that
merging the earlier unchanged reader file would restore the bound.

The helper retains total count and byte limits for the compatibility vector and
always calls the captured range proof, even for empty, future and maximum-value
cursors. It preserves ordinary legacy reads. Indexed response readers and their
public output pages bypass this complete-vector API, so a small requested result
remains available above either lifetime limit. Small direct log pages also remain
available; an oversized complete-vector request is an explicit refusal.

Two actual-Mongo regressions under the already-selected `response_readers`
namespace cover independent total row/byte refusals, successful small prefix/tail
pages in a large history, and empty-range refusal after an uncommitted append.
They need execution before this correction is called qualified. Existing range,
bounds and indexed-response suites are included in the corrective gate.

The other merge-preview conflict is `config/features.rs`: preserve every
independent flag, parser assignment and test from both parents, including quota,
batching and batching's snapshot prerequisite. No whole-file older-side choice
is valid for either conflict.

Adopted-caller boundary: v2 retrieval, output paging and replay, plus legacy
retrieval/streaming, select the indexed response port before legacy fallback.
Direct log page/range/prefix calls now use the bounded helper. The generic
`AgentReads::complete_log` still serves legacy methods; S3 must preserve its
adopted transcript/usage/title routing through the compatible paged reader when
admission/optimization flags close. This response branch does not qualify those
transcript callers or eliminate legacy live streaming's requested-turn traversal.

## Combined integration and rollout gates

1. Merge the published branch normally, preserving the consumed dependency
   contracts and the current integration branch's genuine writer/recovery roots.
   The unused CMA diagnostics must close through real integration, not allowances.
   Preserve the integrator's configuration-documentation fix `ce9da2336`: this
   isolated branch still lacks the parent-readiness pacing key in the canonical
   operations-document selection, although its dedicated recovery document
   describes it. Do not move that unrelated setting into the response contract.
2. Qualify combined strict CMA, affected core/public/adapter tests, canonical
   migration and grants, snapshot/schema/drift, writer publication, recovery,
   transcript consumers and all P1 closure evidence at the exact combined head.
   The canonical selector already includes the reader tests in both lists.
3. Keep adoption, writer and recovery admission default-off. Retain this reader
   after any adoption even if optional optimization/admission flags close; an
   older image without the certified reader is not a rollback candidate.
4. Obtain exact-head review, the lead's merge relay and green develop artifact
   evidence before release-owner deployment and production verification.

The Thursday 08:30 cutoff is NO-GO. The updated shared schedule targets combined
evidence by 13:30, PR ready at 14:00, review/merge by 16:00, then release-owner
deployment Thursday evening. A schedule or default-off state is not a gate pass.

All prior owned Mongo fixtures and volumes were removed and read back as absent.
After disk pressure interrupted the final configuration build, another 1.480 GiB
of owned compiler outputs was removed, in addition to the earlier 2.783 GiB
cleanup. Shared disk subsequently fell below the mandatory floor again; no new
heavy work starts until capacity recovers. The guarded corrective coordinator
waits before archive/build allocation, then uses the existing shared locks.
No production calls, adoption or activation occurred in this work. Final gate
and cleanup disposition will replace this pending checkpoint before completion.
