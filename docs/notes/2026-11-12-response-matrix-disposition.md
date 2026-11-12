# Response disposition for segmentation P1-4 and P2

Part of #1349 and #1674. This is the response slice for the combined segmentation
PR. It does not close transcript/title/usage findings or authorize activation.
Source base: combined `ce18603ca`, whose code matches `4b148fba5`.
Existing combined executable evidence is pinned to `92ed005a9`; subsequent
composition whitespace and ledger changes do not alter response behavior.
Narrow implementation: `9d59b39a4be20fac4ad53b71a2335b8deda12570` on
`tl/cma-log-segments/reader-foundation`.

## P1-4: response behavior and bounds

The compatibility API returns the entire requested vector or an explicit
`AgentLogCapacityExceeded` refusal. It independently enforces its row and byte
limits, including beyond 10,128 lifetime rows / 4 MiB. Small requested ranges
continue to work in large histories. Empty, future and maximum-value cursors
still validate the captured proof; an empty response cannot hide an unpublished
append. The compatibility API is not an unlimited-history endpoint.

Public legacy and v2 response retrieval select the retained indexed reader before
legacy fallback. Both receive it from production's shared projection service.
Small results above both old lifetime limits read only their original source
references. Oversized complete output returns `response_pagination_required`;
output pages remain accessible and preserve all output items. No successful
truncation or whole-history fallback is used for adopted records.

V2 replay locates a deep numeric cursor by binary search over immutable output
change ordinals. Original points, turn facts and output changes use the same
captured proof and snapshot. Subsequent tool output updates current items without
renumbering immutable replay events. The existing index describes first tool
starts/terminal transitions, not every intermediate legacy tool update.

Legacy live replay therefore uses original-row pages for the selected turn. It
must preserve every intermediate update; replacing this path with the v2 change
index would omit valid legacy events. The new fix limits quiet traversal between
body yields to eight one-row observations. After eight quiet pages it emits an
SSE comment, without a Responses event, durable ID or sequence increment. Its
source cursor advances and is retained across the yield. Backpressure can now
stop sparse traversal before the entire turn is scanned. A later proof or
ownership failure closes the stream before another source page or completion.

The work unit is explicit: bounded source observations between body yields,
bounded original-row/metadata reads per observation, and byte-bounded final
output assembly. The terminal object still follows the existing result-byte
budget and pagination refusal. Total complete legacy replay is proportional to
the requested turn, including its intermediate updates and quiet source rows;
there is no constant-total-work or constant-time-to-next-data-event claim.
V2 replay and indexed retrieval retain their numeric seek behavior. No lifetime
vector is introduced. Writer/recovery and optional optimization flags stay off;
compatible reading of retained adopted records continues when admission closes.

## P2: response matrix

Paths below are relative to `api/crates/`. Public test names use prefix
`responses::v2::tests::indexed_tests::` unless another prefix is shown.
The exact executed baseline names were checked against S2's unit/Mongo inventories.

| Requirement | Executable evidence | Meaning |
| --- | --- | --- |
| Serialized legacy parity | New `legacy_bounds::legacy_serialized_events_match_original_completion_failure_and_interruption` | Serialized event arrays match the unindexed legacy projector for completion, interruption and typed failure; empty messages, intermediate tool output, later output and usage survive. SSE comments do not affect event numbering. |
| V2 serialized object and replay parity | `parity::every_replay_suffix_matches_original_messages_tools_and_terminal_output`, `failure_gap_and_post_terminal_usage_match_the_original_projection`, `exact_deferred_failure_binding_stays_queued`; `history::historical_models_and_usage_stop_at_the_next_original_turn` | Objects and every reconnect suffix match original projection; historical model, deferred failure and post-terminal usage are retained. |
| Legacy retrieval parity | `parity::legacy_retrieval_uses_original_message_ordinals_and_current_usage` | Serialized legacy response preserves original ordinals and current usage. |
| Large sparse history and long turns | New `legacy_bounds::legacy_sparse_long_turn_yields_bounded_pages_without_losing_events_or_writing`; both `large_history::` cases | Over both old limits, legacy source pages advance exactly once per requested row with at most eight between yielded frames; serialized events match the short reference. V2 deep replay probes only the indexed position. |
| Result/page bounds and continuation | Both `pagination::` cases; `http::legacy_http_pages_oversized_output_without_truncating_the_response`, `v2_output_page_http_preserves_authority_and_refuses_a_foreign_cursor` | Complete over-budget results refuse explicitly; every bounded page remains accessible, cursors advance, foreign/future cursors refuse. |
| Invalid proof/source | All three `integrity::` cases; `pagination::missing_proof_never_becomes_empty_output_or_legacy_fallback` | Invalid identities/counters/coordinates/tool references/source facts refuse instead of producing partial success or fallback. |
| Final authority and deadlines | All five `authority::` cases; `http::legacy_indexed_stream_retains_intermediate_tools_and_fences_buffered_frames`; new `legacy_bounds::legacy_quiet_yield_cannot_bypass_lost_proof_or_revoked_authority` | Share revocation, purge and deadline expiry discard pending output; the quiet yield cannot renew authority or bypass proof refusal. |
| Production dispatch/default-off | Both `projections::response_readers_tests::` cases and `authority::adopted_unbound_stream_keeps_reading_with_legacy_optimization_off` | Legacy/v2 production factories share the reader; disabled optimization does not strand adopted records. |
| Retained caller context | Four `responses::v2::event_context_tests::buffered_retained_context_*` cases | Imported retained-context success, concealment, dependency refusal and final binding fence remain in the combined inventory. |
| No-write behavior | New long-turn and quiet-refusal tests plus real Mongo cases below | Controlled public reads leave Agent/source unchanged; actual Mongo command tracing proves the store performs no read-side writes or transaction control. Fixture fault injection and purge are explicit test actions, not reader writes. |

The three new regressions are in
`layer1-public/src/responses/v2/indexed_tests/legacy_bounds.rs`.
Source page counters are added only to the controlled fixture. Production change
is confined to `layer1-public/src/responses/stream.rs`; persistence is unchanged.

## Existing actual Mongo qualification

All eleven cases below executed successfully at `92ed005a9`, as a subset of
S2's 361 ordinary Mongo contracts. They are not eleven additional passes on the
new commit. The relevant reader/storage files are source-identical on this pin.

Under `store::agent_log_segments::mongo_tests::response_readers::`:

- `compatibility::compatibility_vectors_refuse_total_bounds_while_requested_pages_succeed`
- `compatibility::empty_compatibility_windows_still_validate_the_original_captured_tail`
- `refusal::indexed_response_refuses_missing_foreign_stale_and_malformed_evidence`
- `refusal::indexed_response_refuses_modified_original_points_and_missing_turn`
- `refusal::response_evidence_and_points_keep_the_capture_snapshot_during_changes`
- `scale::long_turn_seeks_numeric_output_ordinals_and_pages_original_rows`
- `scale::retained_response_points_stay_bounded_above_both_history_limits`

Under `store::sandbox_mongo_integration_tests::response_projection_tests::`:

- `bounded_response_source_matches_the_turn_and_reduces_total_checkouts`
- `response_window_budget_model_and_disabled_fallbacks_do_not_write`
- `response_window_refuses_quarantine_stale_capture_and_invalid_turn_without_writes`
- `response_window_retains_the_next_turn_witness_and_unbound_has_no_output`

The scale fixture measures fixed requested output at 1/10/100/1000 sealed units.
Every sample asserts three source operations, thirteen source rows, less than
16 KiB source bytes, fewer than sixty checkouts/commands and less than 256 KiB
wire replies. It rejects writes, transaction control, collection scans and
blocking sorts. Three source operations do not mean three checkouts. These are
local executable query bounds, not production latency measurements. The snapshot
race changes evidence and unread original rows after capture: the held reader
remains coherent and a fresh reader refuses damaged proof.

## Gate record and integration

New gates executed from immutable `9d59b39a4`, archive SHA-256
`0a2d15bbfca9adc8d7aab5515e9f8ad98a9c6993f1ce8952b5aeb2f3a678494d`,
under existing heavy-1 with `lockf -k -t 3600`, two Cargo jobs and one target.
The source checksum remained unchanged throughout.

| Gate | Result |
| --- | --- |
| New `legacy_bounds::` tests | 3 passed, 0 failed, 0 ignored; 627 filtered; 0.94 s. |
| Full public `responses::` tests | 111 passed, 0 failed, 1 existing SDK loopback fixture ignored; 518 filtered; 6.04 s. Includes the three new tests; counts are not additive. |
| Strict public Clippy, all targets/features, `-D warnings` | Passed, zero diagnostics; 1m11s. |
| Formatting, whitespace, canonical source-size | Passed; 6,329 source files. New/changed Rust files are under 500 lines. |
| Canonical full-history secret scan at code pin | Passed; 4,097 commits, no findings. Final evidence-only head is rescanned before push. |

Commands: `cargo test --locked -p layer1-public --lib
responses::v2::tests::indexed_tests::legacy_bounds:: -j 2 -- --nocapture`,
`cargo test --locked -p layer1-public --lib responses:: -j 2`, and
`cargo clippy --locked -p layer1-public --all-targets --all-features -j 2 --
-D warnings`. Existing strict CMA/core/public/adapter and both factory passes
remain tied to `92ed005a9`. No Mongo rerun is claimed for this public streaming
change; its actual existing coverage remains pinned above.

The response portion of P1-4 and P2 is source-qualified for reviewer readback.
Whole combined findings remain open for the separately owned consumer work.
The ledger records this disposition without treating an inactive rollout as a
substitute for a passing gate.

The combined integrator must select all three new `legacy_bounds::` tests plus
the existing response matrix in the final source inventory. Normal merge only;
no standalone PR and no develop merge is authorized by this handoff. Writer and
recovery activation remain closed, with reviewer confirmation and production
acceptance separate from local source qualification.

## Shared route-contract integration follow-up

The shared ledger's 11:55 qualification update records three broader public
contract failures at the combined base: operation count, tenant-isolation
coverage and mounted-route classification omit
`GET /api/v2/responses/{response_id}/output`. These are outside the passing
`responses::` selection above. The existing indexed HTTP response test proves
route behavior and authority but does not substitute for registering it in the
canonical executable public/tenant inventories. S2 owns shared registrations;
this follow-up must pass in the final combined PR. No exclusion or waiver is
claimed by the response source handoff.

## Cleanup

The one owned Cargo target and immutable gate export were removed after the
gates, freeing 1.224 GiB; free disk readback was 63.38 GiB. No Mongo container or
volume was created by this slice. Both shared-lock inodes were verified unchanged.
Local logs, source hashes, result inventories and cleanup evidence are retained
under `notes/2026-11-12-response-matrix-evidence/` in the lead's workspace.
