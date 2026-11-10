# Public Agent-log windows and integrity proof

Related: #1320, #1543, #1591. This is the prepared public-window consumer. Production still uses complete validation until the metadata owner supplies the reader adapter and the combined integration gates pass. No metadata format, writer or preparation path is duplicated here.

## Reader interface required from #1591

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
}
```

The consumer opens a Mongo snapshot session. The adapter reads the Agent envelope and every proof query through that exact supplied session and snapshot. It returns `None` only when the Agent is absent. It must not start or abort a transaction, write metadata, repair history, or persist quarantine from the public GET. A corruption result can be handed to an explicitly owned audit/quarantine path separately, but cannot become a successful read or generic repair fallback.

`trusted=true` requires the same accepted #1591 proof: supported metadata; current envelope/record head and Agent revision agreement; immutable creation/history identity; exact count and encoded-byte physical bounds; raw tail identity/sequence/hash; no extra tail; no quarantine; prepared and activated cohort. A retained zero head requires an unfiltered indexed existence check, including malformed rows without a valid sequence. Disabled cohort, missing metadata and supported stale/legacy evidence return an untrusted current head. Unsupported-version classification follows #1591's explicit policy and must never grant trust. Ahead/head/hash/identity contradictions and quarantine propagate errors; dependency and cancellation errors are never converted to missing/empty history.

The current admission implementation's `trusted_summary` does not yet meet this reader interface: it assumes a writer transaction, calls `abort_transaction`, may write quarantine, and its helper queries/full-audit error path do not explicitly support the reader's snapshot mode. Reusing its parser and proof rules through a read-only snapshot adapter is the requested convergence point. A second parser or silently weaker proof is not acceptable.

Do not use metadata presence or a head integer alone as authorization. The existing default adapter always returns `trusted=false` and retains the previous projected Agent read and complete scan. Wiring a real adapter and confirming rollout/cohort semantics are mandatory before this branch can be described as a production optimization.

## Consumer behavior

Pages, ranges and prefixes share the proof snapshot. With trusted evidence, the query uses the existing unique `(agent_id, seq)` index, exact lower/upper sequence bounds, ascending order, a result limit and a batch size matching the requested count. Every returned BSON record still passes `read_entry`, expected Agent identity, exact sequence and expected count checks. Missing or corrupt requested rows fail without a partial response. Empty windows still require the head proof and perform no window query. Range/prefix high-water errors and invalid page sizes preserve the existing API behavior.

An untrusted head always runs the existing complete validator with `ScanCaller::PublicWindowPrefix`, including for empty windows. It can therefore still detect corruption beyond the requested page. Trusted windows rely on #1591's audited immutable-prefix model: corruption outside the requested range and validated tail is the explicit audit's responsibility. Full-history callers that request the entire prefix still read that entire prefix; their semantic folds are not removed by this slice.

Existing complete-scan telemetry remains authoritative for fallbacks. Bounded reads count consumed BSON through `mongo_read_telemetry`, including partial failure. They are not labelled as complete scans. This slice changes neither pool size nor runtime/user-input authority.

## Verification and integration gates

- Preserve the original legacy prefix regression, including out-of-window gaps, malformed tail identity, extra tail, missing Agent and zero-length prefix behavior.
- Exercise the consumer with an explicitly injected fixture proof at histories of 1,000 and 10,000 entries: equal output, 200 consumed documents for a 200-entry page, indexed `executionStats.totalDocsExamined=200`, empty-page no-log-read behavior, and measured CMAP commands/checkouts.
- The injected proof does not measure the real metadata proof overhead. Record that separately once the adapter lands; aggregate + tail + extra-tail + page must fit the full page budget of at most 250 examined documents. A decrease in decoded documents does not guarantee fewer checkouts on every small history.
- Preserve a single session and snapshot across head/proof/window; a writer advancing after the captured head cannot leak a later row into the page. Refuse requested gaps, malformed rows and proof errors.
- Before activation, run actual #1591 proof tests for missing/stale/mixed writers, unknown version, quarantine, zero-head orphans, tail hash/identity conflict, concurrent writers and rollback/re-upgrade. Demonstrate that the public read issues no metadata or quarantine write and does not perform preparation inside the request.
- Add the actual provider and run targeted formatting, clippy, consumer tests, legacy tests and real-Mongo combined metadata/window tests before opening the implementation PR. The prepared consumer alone does not satisfy #1320 or TC-046.
