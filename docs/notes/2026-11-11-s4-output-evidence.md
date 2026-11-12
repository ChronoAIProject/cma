# S4 output evidence publication checkpoint

Part of #1350 and transactional #1349. Uses the unchanged S2 domain model at
`9327ec32f` and S1 captured-source implementation at `5b8a823fc`. No activation
or review-ready claim; production caller wiring and executable gates remain.

## Durable contract

All rows use the existing strict `agent_log_evidence` envelope and evidence
generation. The shared key function remains `projection::identity`.

| Kind | Entity key | Typed record | Write rule |
| --- | --- | --- | --- |
| turn_output | decimal turn | `{type:turn_output,index:AgentTurnOutputIndex}` | Original-document CAS |
| output_change | turn:ordinal | `{type:output_change,change:AgentOutputChange}` | Insert only |
| tool_output | turn:UTF8-byte-length:call_id | `{type:tool_output,tool:AgentToolOutputState}` | Original-document CAS |

Only output_change carries top-level BSON int64 `turn_sequence` and
`change_ordinal`. Both are required for that kind, equal the typed payload and
fit the signed safe-integer ceiling. Other kinds reject those query fields.
S1 owns the validator additions and unique partial numeric index requested in
#1348 comment-6795481510. Until that migration dependency arrives, the existing
strict validator correctly rejects output-change writes. No runtime DDL.

## Publication and bootstrap

Preparation loads the latest turn index at the captured global through. Missing
or stale evidence refuses. Every consecutive row advances it. Touched tool
state comes from the same transaction or the bounded current batch; lookup
failures never become absence. Original BSON is retained for CAS. Each of the
at most64 source rows adds at most3 output evidence writes. Per-row BSON is
capped at256KiB. Immutable first-terminal changes never become replacements.

Bootstrap uses the identical `projection::output::fold` and typed row encoder.
It retains one scalar turn index and its existing64-row/1MiB staging batch.
Tool state outside that batch is read by exact key from the invisible committed
generation. Each staging write transaction renews the original permit and
rechecks admission; final publication still recaptures the original source.
S1 audit/rebuild can reuse this owned fold/encoder when constructing a new
evidence generation; a rebuilt generation lacking the latest output index
will correctly refuse foreground publication.

## Test sources and gate status

New unit tests check strict numeric seek fields, counter corruption, empty and
whitespace messages, duplicate tool starts, first-terminal/late-output separation
and cross-turn tool refusal. Mongo tests cover publication commit/abort and
missing-index refusal. The bootstrap fixture carries one tool across multiple
staging batches and checks its original start, first-terminal output and later
current output. None of these new tests is claimed executed by this note.

CMA check reached the test sources and found one missing projection import;
that import is fixed. It also reports expected unused code while production
caller/registration work remains. Strict CMA clippy is not green. The next
unit gate is queued under the existing persistent heavy-0 lock, two jobs and
one retained api/target. File-size and whitespace checks passed.
