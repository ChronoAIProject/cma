# S2 part 2: output-index dependency for S4/S1

Review target Thu 2026-11-12 10:00 SGT. Default-off. This extends the frozen publication contract, not a second writer. S2 owns consumer/store page readers and Layer1 wire projection. S4 owns atomic publication/bootstrap of these domain facts; S1 owns index/schema registration. No Responses JSON, event names or wire event counts enter Layer2/persistence.

## Required immutable evidence

Per target turn, retain scalar output-change count and counters: output items, assistant messages (including empty), nonempty assistant messages (`!content.is_empty()`, not trim), first tool starts, first terminal tool transitions. Keep per-change evidence, keyed by `(agent_id, incarnation, evidence_generation, turn_sequence, change_ordinal)` with numeric ordinal. Every producing source row has a single immutable change record: source sequence, counters BEFORE and AFTER, output item ordinal, message ordinal when applicable. For tool events, retain original start sequence and most recent output source at the FIRST terminal transition. A start already terminal emits both transitions. Duplicate starts and subsequent terminal updates do not increment the counters. Later tool output still updates separately indexed current-item evidence for final output. All source references are proved in the same captured snapshot; absent evidence is never an empty successful page.

The current projection retains only scalar counters, not a lifetime tools map. Per-tool state is indexed by turn/call identity and read under the publication transaction. Changes are atomically published with source/head/receipt. Bootstrap derives identical facts under its existing bounded source budget. Writer-disabled legacy sources without this index retain existing explicit fallback; segmented sources must refuse if required evidence is absent and never fall back to complete lifetime reads.

## Reader contract and bounds

S2 translates counters to the existing numeric event cursor in Layer1: opening frames 1 (unbound) or2 (bound); output events =5*messages +nonempty_messages +2*tool_starts +2*tool_finishes. Empty messages emit5, nonempty6. No persisted protocol-specific event rank. A numeric reconnect cursor finds its source change by bounded binary search over the numeric ordinal index (maximum53 point probes, reflecting the existing safe-integer sequence ceiling); a normal continuation uses the verified source ordinal directly. Each returned page has <=32 changes and <=4MiB source bytes. Source lookup reuses S1 captured-source points/range proof; no skipped prefix is scanned. Checks reject counter discontinuity, conflicting identity/revision/generation, missing rows, corrupt source and quarantine.

S4/S1: please confirm and publish the smallest normally mergeable schema/signature dependency, or identify an equivalent already-defined index today. The source projection currently has turn/message evidence but no output ordinal/tool transition index, so it cannot yet support long-turn numeric reconnect without prefix reconstruction. Do not claim the current turn record alone solves long-turn paging. S2 can build no-pre-header snapshot and unchanged-head polling independently while this dependency is published.

## Ownership and next checkpoint

S2 may add protocol-neutral output-index structs in a new owned core module and page-reader files. S4 wires those types into its owned projection/writer files. S1 adds the numeric compound index to its owned schema migration. Please relay concrete signature/checkpoint by Wed2026-11-11 21:00 SGT. Complete terminal objects remain byte-bounded; an over-budget object must explicitly direct the client to complete paged output, never silently truncate or fabricate completion. Public compatibility and pagination behavior remain part of S2 review.

No activation, production access, develop merge or successful integration claim. Develop freeze remains in effect.

## Concrete S2 domain symbols

S2 has implemented `layer2_core::sandbox::agent_output_index` with these public protocol-neutral symbols (source checkpoint9a1f05473, focused gates queued):

```rust
struct AgentOutputTotals { items: u64, messages: u64, nonempty_messages: u64,
    tool_starts: u64, tool_finishes: u64 }
struct AgentToolOutputState { call_id: String, turn_sequence: u64,
    item_ordinal: u64, start_sequence: u64,
    output_sequence: Option<u64>, terminal_sequence: Option<u64> }
enum AgentOutputTool<'a> { None, Absent, Stored(&'a AgentToolOutputState) }
struct AgentOutputChange { turn_sequence: u64, ordinal: u64, source_sequence: u64,
    before: AgentOutputTotals, after: AgentOutputTotals, item_ordinal: u64,
    message_ordinal: Option<u64>, tool: Option<AgentToolOutputState> }
struct AgentTurnOutputIndex { agent_id: AgentId, turn_sequence: u64,
    through_sequence: u64, changes: u64, totals: AgentOutputTotals }
struct AgentOutputDelta { change: Option<AgentOutputChange>, tool: Option<AgentToolOutputState> }
impl AgentTurnOutputIndex {
    fn start(entry: &AgentLogEntry) -> Result<Self, String>;
    fn validate(&self) -> Result<(), String>;
    fn observe(&mut self, entry: &AgentLogEntry, tool: AgentOutputTool<'_>)
        -> Result<AgentOutputDelta, String>;
}
```

All durable structs deny unknown fields; Option source references are required nullable fields. `observe` validates Agent identity, consecutive source sequence and prior indexed tool provenance before committing its fixed-size state. The adapter must supply certified same-snapshot tool absence/presence; a query error cannot become Absent. Subsequent tool output is stored in current per-tool state but never overwrites immutable first-terminal change evidence. Duplicate tool starts are ignored, matching the existing public projector. A new TurnStarted ends the old index; create a fresh turn index instead of observing it in the old one. No source text is retained in the counter index.

S4 integration uses existing agent_log_evidence generations and atomically publishes three record kinds: `turn_output` keyed by turn, `output_change` keyed by `turn:ordinal`, and `tool_output` keyed by a length-prefixed turn/call identity. S1 supplies a unique numeric seek index on `(agent_id,incarnation,evidence_generation,kind,turn_sequence,change_ordinal)` for output_change records (partial filter kind=output_change), plus existing unique kind/entity_key point lookup. Numeric top-level turn_sequence/change_ordinal must equal the typed record and fit the existing signed safe-integer ceiling. This schema/index must go through the existing migration runner and reader-floor/gate; no runtime DDL or GET backfill.

A legacy bootstrap needs the same per-tool lookup semantics and bounded staging batches; never accumulate a lifetime map in the active-head record. If supporting a large legacy turn needs more than the existing bootstrap budget, return the typed bounded refusal and preserve the source. Public paged reads remain read-only and source-certified. S2 will publish a normally mergeable isolated module branch so S4 does not import response controllers.

Validation update: all10 focused core/API tests passed at8e1a2dbfe, including5 output-index model tests and5 stream observation tests; core/public all-target/all-feature Clippy with warnings denied passed. The isolated module commit is source-identical to the tested module. These runs used lockf -k -n on heavy-1, two Cargo jobs and one target. No Mongo paging/publication or production gain is claimed by these domain tests.
