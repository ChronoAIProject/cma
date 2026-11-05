# Sandbox maintenance second fix for #1354

The 5 November 2026 deployment of #1355 retained sweep phases but still restarted
home discovery whenever its combined serial work exceeded one five-second
operation. This change retains discovery progress below that boundary. It does
not change the 20-second slice, 30-second outer bound, shared admission,
readiness, persistent schema, or public HTTP API.

## Production evidence

Read-only VictoriaLogs Kubernetes-proxy GETs selected only the production
control-plane cluster, `chrono-bot` namespace, CMA API workload and the fixed
maintenance phase/attempt/completion messages. The retained fields contain no
resource identities, caller spans, errors, secrets or user content.

From **04:34:00 to 04:44:52 UTC (12:34:00–12:44:52 SGT), 5 November 2026**:

- Two yielded attempts, then **14 consecutive home-inventory deadlines**.
- Home-inventory operations took **5,000–5,005 ms**; zero completed sweeps.
- The last sweep age was **608,796 ms** at attempt 16.
- Runtime inventory completed in **419 ms**. Authority read 341 items in two
  operations / 244 ms. Bindings advanced through all 341 items over slices.
- Runtime cleanup processed 46 items / 176 ms; claim inventory completed in
  88 ms; claim cleanup processed 46 items / 138 ms.

These observations identify the bulk home-inventory boundary. They **do not
identify a slow individual request**; #1355 has no child timing telemetry.
The production scale reported by the deploy operator is about 52 home PVCs,
43 running and nine suspended. The evidence/design was posted to
[issue #1354](https://github.com/ChronoAIProject/cma/issues/1354#issuecomment-6649925506).

## Discovery checkpoints and cleanup authority

`InventoryCursor` is an internal Rust provider boundary, not a wire API or
persistent cursor. `advance` returns no inventory until its complete set has
validated. The reconciler retains the cursor under its existing progress lock.

- Routed homes retain completed provider routes and reject conflicting disk
  identities atomically before accepting a route. An invalid route is retried;
  its drained cursor cannot turn an error into an empty successful inventory.
- Persistent homes retain validated list pages and continuation tokens.
- Native claims retain validated pages, snapshot resourceVersion, continuation
  tokens, identity uniqueness and existing page/item bounds. A bad page or
  cancelled request cannot advance any part of its checkpoint.
- Native homes retain the legacy prefix, complete claim list and current item.
  Each claim read is committed before the attachment reads. The existing
  parallel Sandbox/PVC reads each commit their own successful response, so a
  cancelled sibling is retried without repeating the successful read.
- Existing Claim → Sandbox → PVC lineage/storage validation is reused. A
  rejected attachment restarts that item from the claim. Completed items and
  preceding routes remain available across deadlines and yields.

These observations remain discovery only. Cleanup starts after the complete
inventory is returned and repeats the existing pool-first / fresh owned-record
fences and guarded provider effects for each item, including after cancellation.
An in-memory checkpoint is never permission to delete. A permanently failing
child still blocks cleanup; restarting the process discards all discovery state.

Every discovery request has a five-second bound. The fixed event
`sandbox maintenance inventory child completed` records `child` as one of
`route_list`, `claim_read`, `attachment_read`, together with `count=1`,
`elapsed_ms`, and `outcome` (`complete`, `unavailable`, `deadline`, `cancelled`).
Counts and durations can be summed by category/outcome. Cancellation emits a
sample via a drop guard, without inheriting the caller span or logging inputs.

## Audit of the other phases

| Phase / nested path | Evidence and treatment |
| --- | --- |
| Runtime inventory | 419 ms in production at the current population. Its provider routes/page loops have no per-home attachment rereads. Retained as one bounded operation; the measurement is not a guarantee for larger populations or an unavailable provider. |
| Authority, recovery authority, candidate listing | Already page-checkpointed by #1355. Authority processed two pages successfully in production. |
| Binding and runtime cleanup | Already checkpointed per Sandbox/runtime; all observed candidates advanced. Fresh checks and effects remain in the same bounded item. |
| Claim inventory | 88 ms observed, but the complete-list helper contains a page loop. It now retains validated pages using the same checks as native cleanup listing. |
| Claim/home cleanup: Pod-parent finality | Source previously issued one serial member GET for every unrelated Sandbox-owned Pod, potentially 43–52 requests per cleanup item at the reported population. It now reads one **fresh complete member snapshot after the Pod snapshot**, checks name/UID and claim ownership, and discards it at the end of the item. A 52-Pod regression requires exactly two list requests and rejects missing/replaced parents and an unrecorded owned member. No concurrency increase or cached absence authority is introduced. |
| Failed recovery, profiles, runtime updates, idle pause | #1355 retains the selected recovery or Sandbox candidate. These perform per-candidate admission/ownership work, including profile workspace scans and transactional idle checks; their fresh mutation fences remain atomic under the five-second bound. Production has not reached these phases in the observed sweep, so their durations remain unverified and must be checked at rollout. |

## Regression coverage

The focused tests exercise 24 seconds of home inventory across two routes and
multiple items over successive bounded attempts; deadline/corruption mid-route;
complete-prefix retention without missing or duplicate items; fresh ownership
published before cleanup retries; native child timeouts and outer cancellation;
independent retention of parallel attachment reads; rejected lineage recovery;
claim pagination timeout and inconsistent snapshot retry; fixed telemetry
categories/counts/durations/outcomes; and 52-Pod cleanup finality.

## Production verification

The lead merges and deploys. Keep #1354 open until acceptance:

1. Observe the first `sandbox maintenance sweep completed` within **15 minutes
   of rollout**, then subsequent sweeps and an increasing completed count.
2. Record whole-sweep and phase elapsed times, attempt/item counts, and child
   counts/durations grouped by the three fixed categories and outcome. A yield
   alone is not a completed sweep. Investigate a repeatedly failing child.
3. Confirm home cleanup and the later failed-recovery, profiles, runtime-update
   and idle-pause phases are reached. Their production timing is not established
   by the pre-fix logs.
4. Confirm cached readiness remains bounded (two-second dependency checks,
   five-second observation expiry) and continuously available.
5. Compare equal 15-minute pre/post windows: no API 5xx increase, readiness loss,
   or permanently growing uncompleted Sandbox pending age.

Local verification results are recorded in the PR. This work does not deploy,
merge, close the incident or mutate production.
