# Buffered history unchanged-route baseline

Local measurement for #1793 using the harness committed with this document.
The request handlers and authorization/binding chain are those at
`58238ad13201a8dee7b7ce67bb11fb7dc7d651a7`; the new tail-proof port has no
caller in this baseline. The page-composition switch is not implemented yet.
MongoDB is 8.0.13, image digest
`sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c`.

Both real-replica-set tests passed. Each uses one sequential request at a time,
20 alternating complete traversals per access-session mode, all three/ten pages,
and separate active/idle heads. Warmups and every measured response compare raw
serialized bytes. Command/checkouts, returned BSON documents/bytes and complete
scan counts/docs/bytes are stable across repeats. Request counts below are per
mode, not pooled across modes. The actual router/services/stores run in a native
debug build on a shared build machine; unrelated suites contend for CPU and the
local Docker host. The test-only raw command observer adds reply decoding and
byte accounting in both modes. These wall times are local observations, not
production estimates or an isolated proof of the switch's latency effect.

## Implementation gate

With access-session OFF, both fixture sizes have the same command counts:

| Request | Authorization snapshots | Authorization commands | All commands/checkouts | Authorization share |
| --- | ---: | ---: | ---: | ---: |
| First snapshot page, both bindings | 8 | 24 | 30 | 80.0% |
| Every later snapshot page | 8 | 24 | 28 | 85.7% |
| Active head | 7 | 21 | 26 | 80.8% |
| Idle head | 4 | 12 | 12 | 100.0% |

The >=50% unchanged-path gate passes on every page at both sizes. Proceed with
the combined buffered operation and retain the final authorization fence.
Access-session ON independently changes those command totals to 22, 20, 19 and 8;
snapshot counts remain 8/8/7/4 and response bytes are identical.

## Observed latency

| Rows | Route | Access session | Requests | p50 ms | p95 ms | Maximum ms |
| ---: | --- | --- | ---: | ---: | ---: | ---: |
| 260 | head_active | false | 20 | 49.263 | 54.378 | 89.777 |
| 260 | head_active | true | 20 | 42.401 | 52.358 | 84.366 |
| 260 | head_idle | false | 20 | 12.849 | 17.445 | 50.772 |
| 260 | head_idle | true | 20 | 10.591 | 13.546 | 13.693 |
| 260 | snapshot | false | 60 | 53.410 | 81.133 | 98.939 |
| 260 | snapshot | true | 60 | 46.870 | 63.511 | 83.669 |
| 1000 | head_active | false | 20 | 88.616 | 145.428 | 180.690 |
| 1000 | head_active | true | 20 | 80.533 | 90.169 | 130.015 |
| 1000 | head_idle | false | 20 | 17.465 | 20.767 | 57.487 |
| 1000 | head_idle | true | 20 | 14.307 | 15.978 | 16.013 |
| 1000 | snapshot | false | 200 | 86.544 | 95.876 | 117.853 |
| 1000 | snapshot | true | 200 | 80.417 | 87.295 | 117.324 |

Every snapshot or active-head request performs one complete history scan:
260 documents / 65,788 encoded BSON bytes, or 1,000 documents / 252,268 bytes.
Idle heads scan none. Access-session mode does not change scan work. Local scan
durations are included below as individual observed values, not percentile data.

## Command and byte evidence

The inferred phase groups use allowlisted command/collection shape, not production
phase spans. Repeated later pages have identical counts/docs/bytes; all pages are
retained here to make the traversal and command ceiling reviewable. Returned BSON
bytes are neither compressed wire bytes nor server documents examined.

```text
HISTORY_COMMANDS rows=1000 route=snapshot page=0 access_session=false page_single_fence=false snapshots=8 commands=30 auth_commands=24 returned_docs=1052 returned_bson_bytes=300713 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 48600] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "binding/agent_commands/find": 2, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=1 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 46903] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=2 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 47353] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=3 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 46868] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=4 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 46196] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=5 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 46942] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=6 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 47492] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=7 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 46952] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=8 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 45357] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=9 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 47595] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=0 access_session=true page_single_fence=false snapshots=8 commands=22 auth_commands=16 returned_docs=1052 returned_bson_bytes=300713 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 47746] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "binding/agent_commands/find": 2, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=1 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 47787] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=2 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 50146] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=3 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 48120] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=4 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 48743] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=5 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 48510] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=6 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 47471] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=7 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 49419] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=8 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 49753] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=snapshot page=9 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=1050 returned_bson_bytes=297348 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 48169] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=head_active page=0 access_session=false page_single_fence=false snapshots=7 commands=26 auth_commands=21 returned_docs=1045 returned_bson_bytes=293912 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 49411] groups={"authorization/agents/aggregate": 7, "authorization/none/abortTransaction": 7, "authorization/resource_shares/find": 7, "binding/agent_commands/find": 1, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=head_active page=0 access_session=true page_single_fence=false snapshots=7 commands=19 auth_commands=14 returned_docs=1045 returned_bson_bytes=293912 full_scan_counts_docs_bytes_us=[1, 1000, 252268, 46738] groups={"authorization/agents/aggregate": 7, "authorization/resource_shares/find": 7, "binding/agent_commands/find": 1, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=1000 route=head_idle page=0 access_session=false page_single_fence=false snapshots=4 commands=12 auth_commands=12 returned_docs=24 returned_bson_bytes=20616 full_scan_counts_docs_bytes_us=[0, 0, 0, 0] groups={"authorization/agents/aggregate": 4, "authorization/none/abortTransaction": 4, "authorization/resource_shares/find": 4}
HISTORY_COMMANDS rows=1000 route=head_idle page=0 access_session=true page_single_fence=false snapshots=4 commands=8 auth_commands=8 returned_docs=24 returned_bson_bytes=20616 full_scan_counts_docs_bytes_us=[0, 0, 0, 0] groups={"authorization/agents/aggregate": 4, "authorization/resource_shares/find": 4}
HISTORY_COMMANDS rows=260 route=snapshot page=0 access_session=false page_single_fence=false snapshots=8 commands=30 auth_commands=24 returned_docs=312 returned_bson_bytes=114233 full_scan_counts_docs_bytes_us=[1, 260, 65788, 14104] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "binding/agent_commands/find": 2, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=snapshot page=1 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=310 returned_bson_bytes=110868 full_scan_counts_docs_bytes_us=[1, 260, 65788, 14045] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=snapshot page=2 access_session=false page_single_fence=false snapshots=8 commands=28 auth_commands=24 returned_docs=310 returned_bson_bytes=110868 full_scan_counts_docs_bytes_us=[1, 260, 65788, 14532] groups={"authorization/agents/aggregate": 8, "authorization/none/abortTransaction": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=snapshot page=0 access_session=true page_single_fence=false snapshots=8 commands=22 auth_commands=16 returned_docs=312 returned_bson_bytes=114233 full_scan_counts_docs_bytes_us=[1, 260, 65788, 14361] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "binding/agent_commands/find": 2, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=snapshot page=1 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=310 returned_bson_bytes=110868 full_scan_counts_docs_bytes_us=[1, 260, 65788, 15106] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=snapshot page=2 access_session=true page_single_fence=false snapshots=8 commands=20 auth_commands=16 returned_docs=310 returned_bson_bytes=110868 full_scan_counts_docs_bytes_us=[1, 260, 65788, 13738] groups={"authorization/agents/aggregate": 8, "authorization/resource_shares/find": 8, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=head_active page=0 access_session=false page_single_fence=false snapshots=7 commands=26 auth_commands=21 returned_docs=305 returned_bson_bytes=107432 full_scan_counts_docs_bytes_us=[1, 260, 65788, 14648] groups={"authorization/agents/aggregate": 7, "authorization/none/abortTransaction": 7, "authorization/resource_shares/find": 7, "binding/agent_commands/find": 1, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=head_active page=0 access_session=true page_single_fence=false snapshots=7 commands=19 auth_commands=14 returned_docs=305 returned_bson_bytes=107432 full_scan_counts_docs_bytes_us=[1, 260, 65788, 14995] groups={"authorization/agents/aggregate": 7, "authorization/resource_shares/find": 7, "binding/agent_commands/find": 1, "log/agent_log/find": 1, "log/agent_log/getMore": 1, "log/agents/find": 2}
HISTORY_COMMANDS rows=260 route=head_idle page=0 access_session=false page_single_fence=false snapshots=4 commands=12 auth_commands=12 returned_docs=24 returned_bson_bytes=20616 full_scan_counts_docs_bytes_us=[0, 0, 0, 0] groups={"authorization/agents/aggregate": 4, "authorization/none/abortTransaction": 4, "authorization/resource_shares/find": 4}
HISTORY_COMMANDS rows=260 route=head_idle page=0 access_session=true page_single_fence=false snapshots=4 commands=8 auth_commands=8 returned_docs=24 returned_bson_bytes=20616 full_scan_counts_docs_bytes_us=[0, 0, 0, 0] groups={"authorization/agents/aggregate": 4, "authorization/resource_shares/find": 4}
```
