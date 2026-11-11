# Public Agent-log windows and integrity proof

Related: #1320, #1543, #1591. This public-window consumer depends on the trusted admission checkpoint implementation in PR #1633, integrated at provider commit `5567c5c5b`. The shared reader contract has converged. Merge the provider first; deploying the consumer does not prepare or activate histories. No metadata format, writer or preparation path is duplicated here.

## Shared reader interface

The consumer boundary is `store::agent_log_reads::integrity::LogReadIntegrity`:

```rust
async fn read_head(
    &self,
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut ClientSession,
) -> Result<Option<ReadHead>, SandboxStoreError>;

struct ReadHead {
    last_seq: u64,
    trusted: bool,
    workspace_id: WorkspaceId,
    creation_worktree_id: WorktreeId,
}
```

The consumer opens a Mongo snapshot session. The adapter reads the Agent envelope and every proof query through that exact supplied session and snapshot. It returns `None` only when the Agent is absent. It must not start or abort a transaction, write metadata, repair history, or persist quarantine from the public GET. A corruption result can be handed to an explicitly owned audit/quarantine path separately, but cannot become a successful read or generic repair fallback.

`trusted=true` requires the same accepted #1591 proof: supported metadata; current envelope/record head and Agent revision agreement; immutable creation/history identity; exact count and encoded-byte physical bounds; raw tail identity/sequence/hash; no extra tail; no quarantine; prepared and activated cohort. A retained zero head requires an unfiltered indexed existence check, including malformed rows without a valid sequence. Disabled cohort, missing metadata and supported stale/legacy evidence return an untrusted current head. Unsupported-version classification follows #1591's explicit policy and must never grant trust. Ahead/head/hash/identity contradictions and quarantine propagate errors; dependency and cancellation errors are never converted to missing/empty history.

The published provider supplies a shared read-only proof that returns the already-validated Agent and the trust decision:

```rust
async fn read_proven_agent(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut ClientSession,
) -> Result<Option<(Agent, bool)>, SandboxStoreError>;
```

PR #1633 owns this interface in `agent_log_integrity/read.rs`; its two-field `read_head` remains a test wrapper around it. The public reader copies the captured head and immutable creation lineage, then uses the same read-only `scan_collection` on fallback. It validates Created-event lineage without rereading the Agent, repeating the metadata proof or invoking the quarantine/audit wrapper. Errors still refuse the read. This preserves explicit audit/quarantine ownership. No metadata parser, writer or preparation logic is duplicated.

The current adapter still evaluates tail/extra-tail proof before checking whether the Agent is in the activated cohort. Measure disabled/unprepared metadata-present Agents as well as enabled Agents: those may pay the two proof queries and then the complete fallback scan. Preserve corruption/quarantine semantics while resolving this cost; do not silently bypass the proof to manufacture a checkout saving.

Do not use metadata presence or a head integer alone as authorization. Production code on this unmerged branch calls the shared proof through `PreparedIntegrity`; `LegacyIntegrity` is test-only for paired baseline measurements. Activated/prepared cohort trust is still required. Combined gates and checkout crossover evidence are mandatory before the branch can be described as a production improvement.

## Consumer behavior

Pages, ranges and prefixes share the proof snapshot. With trusted evidence, the query uses the existing unique `(agent_id, seq)` index, exact lower/upper sequence bounds, ascending order, a result limit and a batch size matching the requested count. Every returned BSON record still passes `read_entry`, expected Agent identity, exact sequence, creation-event lineage and expected count checks. Missing or corrupt requested rows fail without a partial response. Empty windows still require the head proof and perform no window query. Range/prefix high-water errors and invalid page sizes preserve the existing API behavior.

An untrusted head always runs the existing complete validator with `ScanCaller::PublicWindowPrefix`, including for empty windows. It can therefore still detect corruption beyond the requested page. Trusted windows rely on #1591's audited immutable-prefix model: corruption outside the requested range and validated tail is the explicit audit's responsibility. Full-history callers that request the entire prefix still read that entire prefix; their semantic folds are not removed by this slice. The separate `agent_conversation_store::reads::pending_approvals_snapshot` also uses the same caller label and retains its complete approval fold. Thus a reduction in page scans cannot eliminate every `public_window_prefix` scan or be equated with the whole historical 15% attribution.

Existing complete-scan telemetry remains authoritative for fallbacks. Bounded reads count consumed BSON through `mongo_read_telemetry`, including partial failure. They are not labelled as complete scans. This slice changes neither pool size nor runtime/user-input authority.

The [fleet selector](CMA_Fleet_Log_Activation.md) extends activation with
default-off stable percentage buckets. Selected reads still require matching
durable proof and never perform lazy preparation themselves. An unprepared
read-only Agent keeps the complete fallback until an eligible write establishes
valid metadata. Percentage changes and the zero-percent kill switch require a
same-image API rollout; the explicit ID cohort must be empty in fleet mode.

## Verification and integration gates

- Preserve the original legacy prefix regression, including out-of-window gaps, malformed tail identity, extra tail, missing Agent and zero-length prefix behavior.
- Exercise the consumer with an explicitly injected fixture proof at histories of 1,000 and 10,000 entries: equal output, 200 consumed documents for a 200-entry page, indexed `executionStats.totalDocsExamined=200`, empty-page no-log-read behavior, and measured CMAP commands/checkouts.
- The current writer proof uses separate tail and extra-tail point reads. If the reader adapter retains both, a nonempty page needs four checkouts (Agent + tail + extra-tail + window), while a complete scan fitting its first batch and one getMore needs three (Agent + find + getMore). Measure this crossover with the real adapter; do not claim a checkout reduction from the injected proof or activate solely on its two-checkout result. Bounded decoding can improve CPU even when that count does not improve.
- The injected proof does not measure the real metadata proof overhead. The prepared-provider tests measure enabled and disabled states separately; aggregate + tail + extra-tail + page must fit the full page budget of at most 250 examined documents. A decrease in decoded documents does not guarantee fewer checkouts on every small history.
- Preserve a single session and snapshot across head/proof/window; a writer advancing after the captured head cannot leak a later row into the page. Refuse requested gaps, malformed rows and proof errors.
- Before activation, run actual #1591 proof tests for missing/stale/mixed writers, unknown version, quarantine, zero-head orphans, tail hash/identity conflict, concurrent writers and rollback/re-upgrade. Demonstrate that the public read issues no metadata or quarantine write and does not perform preparation inside the request.
- Run targeted formatting, clippy, consumer tests, legacy tests and real-Mongo combined metadata/window tests before opening the implementation PR. The prepared consumer alone does not satisfy #1320 or TC-046.

## Measured combined gate

At source `5289b6663`, formatting, targeted cma clippy (tests/all features), six consumer/integrity unit tests, ten public-window Mongo tests, three legacy integrity Mongo tests, three shared-proof Mongo tests and three semantic-audit Mongo tests passed. The run held the sprint heavy-1 lock with at most two Cargo jobs. The owned Mongo 8.0.13 container and volume were removed. Four affected runner contracts passed for source and archive selection; all consumer handwritten source files are below 500 lines.

The paired comparator preserves the provider's projected Agent head read and calls its actual `scan_complete_log_with_snapshot_session`, including the shared proof and driver cursor batches. That reader shape is unchanged between `03fb3984a` and `5567c5c5b`. It checks identical returned rows and measures CMAP checkouts with the pinned Mongo driver. These are observed totals, replacing the earlier source-derived estimates.

For a 200-row page with approximately 4 KiB message records:

| Retained rows | Metadata/cohort state | Provider reader checkouts | Consumer checkouts | Saved |
| --- | --- | ---: | ---: | ---: |
| 1,000 | Prepared and enabled | 6 | 4 | 2 (33.3%) |
| 10,000 | Prepared and enabled | 8 | 4 | 4 (50%) |
| 1,000 | Prepared, cohort disabled | 6 | 5 | 1 |
| 10,000 | Prepared, cohort disabled | 8 | 7 | 1 |
| 1,000 | Unprepared, no metadata | 4 | 3 | 1 |
| 10,000 | Unprepared, no metadata | 6 | 5 | 1 |

The historical pre-metadata complete-reader comparison remains separate: three checkouts at 1,000 rows and five at 10,000. The enabled consumer therefore adds one checkout at 1,000 rows and saves one at 10,000 versus that older shape. Prepared-but-disabled fallback still pays two proof reads versus that older baseline. These rollout costs must remain visible; do not present the post-#1591 savings as a blanket reduction from the production baseline.

The real enabled reader consumes 201 log documents (one proof tail plus 200 window rows), plus its Agent envelope; the extra-tail probe returns no document. The whole-read Mongo profiler regression includes every proof and window command and rejects collection scans. At both 1,000 and 10,000 retained rows, each 200-row tail page, frozen range and bounded prefix examines exactly 202 documents across all collections and uses four checkouts. Empty pages examine two documents and use three proof checkouts with no window query. These satisfy the respective 250- and 20-document budgets. The injected-proof test separately confirms that the window itself examines exactly 200 documents; its two-checkout result is only a consumer fixture.

Single local unoptimized historical/enabled timings were 178.795/22.555 ms at 1,000 rows and 1,842.761/27.556 ms at 10,000 rows. These are not production latency forecasts. CPU/decoding opportunity and checkout savings must be reported separately. Full-prefix semantic consumers still read their entire requested prefix. A regression confirms that an explicit semantic audit revokes trust after same-length middle-history damage and that the public reader refuses without changing the quarantine marker. Production activation and broad #1320 closure remain pending.

The page budget does not establish TC-046's per-Agent head-refresh rate, aggregate observer command budget, zero idle complete scans or paired idle checkout reduction. Poll scheduling, fresh policy checks, complete pending-approval folds and full Responses reconstruction remain separate caller work. An empty enabled read still requires three proof checkouts; deployment verification must measure these broader targets independently.
