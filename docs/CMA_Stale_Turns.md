# Stale turns: prevention evidence and owner disposition

## Scope and symptom

A turn can remain `working` / `in_progress` after its tools finish, with no final
assistant answer or turn terminal. Tool completion, usage updates, and
`work_completed/publish_connection` are not turn completion. Agent `updated_at`
can advance for lifecycle work without turn progress.

For [#1405](https://github.com/ChronoAIProject/cma/issues/1405), observer overlap is
a demonstrated mechanism capable of this symptom; it is **not a proved cause of
that individual incident**. The historical report identified eight turns across
four users. Its eight-row manifest is unavailable. Discovery below nominates
current candidates; it cannot reconstruct that exact historical cohort by itself.

The decision is prevention plus operational disposition using existing owner
operations. **There is no repair tool or automatic sweep in this change.** A
future runtime capability is tracked separately: **[NEW ISSUE — to be filed by
the reviewer]**. Keep the historical assessments on `fix/stale-turn-repair` for
reference; this document is the consolidated operational guide.

## Prevention evidence

[#1428](https://github.com/ChronoAIProject/cma/issues/1428) and merged
[#1429](https://github.com/ChronoAIProject/cma/pull/1429) carry exact prompt identity
through terminal observations and allow a replacement observer to apply terminal
meaning after an earlier quiet receipt without rewinding the durable cursor.
They do not reconstruct terminal evidence already lost from a runtime.

The test-only regression, originally `12728f367` and cherry-picked with `-x`, is
`api/crates/layer2-core/src/sandbox/store/memory/agent_completed_tools_tests.rs:5`,
`completed_tools_then_reconnect_preserve_one_identified_terminal_in_1000_trials`.
Across 1,000 deterministic trials and eight shapes it verifies:

- Two completed tools, usage, and actual fenced PublishConnection completion
  leave the turn active until its identified terminal arrives.
- Both observer orders produce exactly one terminal; success/failure and
  final-text/no-final-text variants preserve the observed outcome and answer.
- Usage is unchanged; each tool start/completion occurs once in durable history.
- No answer is invented from tool completion, including acknowledgement/tools-only
  normal completion. No model or external tool is executed.

This is in-memory propagation evidence for prevention, not a production latency,
external-side-effect, faithful repair, or completed public Response qualification.
The test's timing is observation-to-publication in a local fixture. No candidate
image or deployment acceptance is implied. Do not extrapolate it to mixed-image
observer overlap; #1429's historical rollout limitation remains separate.

## Why faithful online repair is deferred

Three findings rule out treating existing history export/reconnect/stop as repair
proof. Source citations are pinned to the retained evidence commits so later
refactoring cannot silently change their meaning.

1. **A history-only terminal can contaminate B.** Publishing A's terminal without
   an output boundary, then admitting B, allowed buffered A text to produce
   `[Late A suffix., New B answer.]`. Unowned text is suppressed only when no turn
   is active: `layer2-core/src/sandbox/conversation_fold.rs:59` at
   [4aa66cc50](https://github.com/ChronoAIProject/cma/blob/4aa66cc50/api/crates/layer2-core/src/sandbox/conversation_fold.rs#L59).
2. **Reopen completion is not durable drain.** The adapter accepted B at ACK=0
   while A's retained frame remained unconsumed; the API drain returned with
   three queued receipts and zero commits. Its opener-first return is at
   `cma/src/sandbox/agent_worker/connection_output.rs:67` at
   [4aa66cc50](https://github.com/ChronoAIProject/cma/blob/4aa66cc50/api/crates/cma/src/sandbox/agent_worker/connection_output.rs#L67).
   A fresh-process `session/load` also replays history: a further probe accepted
   B with historical A pending at cursor 2. Same-session load is at
   `layer3-adapter/src/hosted/acp_handshake.rs:196` at
   [504addd52](https://github.com/ChronoAIProject/cma/blob/504addd52/api/crates/layer3-adapter/src/hosted/acp_handshake.rs#L196).
3. **Close is not a reversible repair pause.** The real runtime accepted Close,
   kept home history readable and left the old journal accessible to a retained
   attachment, then rejected Open with `mutation_authority_retired`, even with
   a higher fence. Cleanup permanently dominates create authority at
   `layer4-runtime/src/mutation_authority.rs:192` at
   [504addd52](https://github.com/ChronoAIProject/cma/blob/504addd52/api/crates/layer4-runtime/src/mutation_authority.rs#L192).
   Attachments retain the Bridge at `layer4-runtime/src/bridge.rs:216` at that
   [revision](https://github.com/ChronoAIProject/cma/blob/504addd52/api/crates/layer4-runtime/src/bridge.rs#L216).

The assessments, exact probe commands and source maps remain at
[4aa66cc50](https://github.com/ChronoAIProject/cma/blob/4aa66cc50/docs/notes/2026-11-12-stale-turn-api-drain-assessment.md)
and [504addd52](https://github.com/ChronoAIProject/cma/blob/504addd52/docs/notes/2026-11-12-stale-turn-stopped-producer-assessment.md).
A last-handshake-response ACK alone is also not proved to order all later A
notifications. Scheduling PublishConnection does not supply a durable observer
epoch. Pinned codex-acp 1.4.0 processes notification drains, cancellation and
session-failure metadata after a native task marker; `task_complete` alone is not
faithful terminal proof, and `end_turn` may carry failure metadata.

## Read-only operator discovery

Run this only through an already authorized **read-only** Mongo connection to the
CMA database. Do not acquire credentials through this guide, run serving startup,
initialize schema/indexes, or write database records. No production access was
used to prepare this change.

Default threshold: **30 minutes without recorded turn activity**. Age nominates a
case for owner review, never proves native completion or authorizes stop/delete.
Legitimate long reasoning may have no new durable output. Inspect owner-visible
command/Response state before requesting a disposition.

The following `mongosh` script scans **20 Agent documents per invocation**, emits
at most 20 candidate/review rows, and reads at most 2,048 projected log rows per
eligible Agent. Set `afterId` to the prior page's `next_after_id`; continue manually
until `has_more` is false. The `_id_` index bounds the Agent scan before filtering
Working status; the existing unique `agent_log_sequence` index is
`{agent_id:1, seq:1}` (`api/crates/cma/src/store/agent_log.rs:32`). This avoids an
unindexed global status/age query. Cost is at most 21 Agent page reads, 20 exact
Agent rechecks and 40,960 projected log rows per page, each query capped at two
seconds. Projection limits returned data, not underlying Mongo document fetch
bytes. Timeouts, missing indexes, unknown schema, malformed data, oversized turn
spans, gaps and concurrent changes are review holds. No `createIndex` is needed.

```javascript
// Use the already selected CMA database. Paste/run once per page.
(() => {
  const afterId = ""; // Replace only with the previous next_after_id.
  const PAGE = 20, LOG_CAP = 2048, MAX_MS = 2000, THRESHOLD_MS = 30 * 60 * 1000;
  const observedAt = new Date(), cutoff = observedAt.getTime() - THRESHOLD_MS;
  const projection = {
    _id: 1, document_schema_version: 1, status: 1, sandbox_id: 1,
    "record.id": 1, "record.status": 1, "record.revision": 1,
    "record.conversation.active_turn": 1, "record.conversation.last_seq": 1
  };
  const integer = v => {
    const n = v == null ? NaN : Number(v.toString());
    return Number.isSafeInteger(n) ? n : NaN;
  };
  const page = db.agents.find(afterId ? {_id: {$gt: afterId}} : {}, projection)
    .hint("_id_").sort({_id: 1}).limit(PAGE + 1).maxTimeMS(MAX_MS).toArray();
  const lifecycle = new Set(["work_step_completed", "work_step_failed"]);
  const activity = new Set([
    "turn_started", "turn_metadata_set", "owner_message", "agent_message",
    "tool_call_started", "tool_call_updated", "tool_output_reported",
    "approval_requested", "approval_resolved", "operation_delivery_changed",
    "usage", "usage_updated", "runtime_metadata_changed", "mode_changed",
    "plan_set", "config_options_set", "available_commands_set", "status_changed",
    "first_message_changed", "title_set", "skill_installation_changed"
  ]);
  const terminal = new Set(["turn_completed", "turn_failed", "turn_interrupted"]);
  let emitted = 0;
  for (const a of page.slice(0, PAGE)) {
    const c = a.record?.conversation, turn = integer(c?.active_turn);
    const last = integer(c?.last_seq);
    const emit = (result, reason, extra = {}) => {
      ++emitted;
      print(EJSON.stringify({agent_id: a._id, sandbox_id: a.sandbox_id,
        turn_sequence: Number.isFinite(turn) ? turn : null,
        result, reason, observed_at: observedAt.toISOString(), ...extra}));
    };
    if (integer(a.document_schema_version) !== 1 || a.record?.id !== a._id ||
        !(integer(a.record?.revision) > 0) || typeof a.sandbox_id !== "string" ||
        a.status !== a.record?.status) { emit("review", "schema_or_identity"); continue; }
    if (a.status !== "working") continue;
    if (!(turn > 0 && last >= turn) || !Number.isSafeInteger(last)) {
      emit("review", "invalid_turn_range"); continue;
    }
    if (last - turn + 1 > LOG_CAP) { emit("review", "turn_scan_cap"); continue; }
    const logs = db.agent_log.find({agent_id: a._id, seq: {$gte: turn, $lte: last}}, {
      _id: 0, document_schema_version: 1, agent_id: 1, seq: 1,
      "record.agent_id": 1, "record.seq": 1, "record.appended_at": 1,
      "record.kind.type": 1, "record.kind.turn_sequence": 1
    }).hint("agent_log_sequence").sort({seq: 1}).limit(LOG_CAP)
      .maxTimeMS(MAX_MS).toArray();
    let reason = null, latest = -Infinity;
    if (logs.length !== last - turn + 1) reason = "log_gap";
    for (let i = 0; !reason && i < logs.length; ++i) {
      const l = logs[i], k = l.record?.kind, t = Date.parse(l.record?.appended_at);
      if (integer(l.document_schema_version) !== 1 || l.agent_id !== a._id ||
          l.record?.agent_id !== a._id || integer(l.seq) !== turn + i ||
          integer(l.record?.seq) !== turn + i || !Number.isFinite(t) ||
          t > observedAt.getTime()) { reason = "log_shape_or_time"; break; }
      if (i === 0 && (k?.type !== "turn_started" || integer(k.turn_sequence) !== turn)) {
        reason = "missing_turn_start"; break;
      }
      if (terminal.has(k?.type)) { reason = "terminal_already_present"; break; }
      if (k?.turn_sequence != null && integer(k.turn_sequence) !== turn) {
        reason = "different_turn"; break;
      }
      if (k?.type === "turn_started" && i !== 0) { reason = "later_turn"; break; }
      if (lifecycle.has(k?.type)) continue;
      if (!activity.has(k?.type)) { reason = "unclassified_event"; break; }
      latest = Math.max(latest, t);
    }
    const reread = db.agents.find({_id: a._id}, projection).hint("_id_")
      .limit(1).maxTimeMS(MAX_MS).toArray()[0];
    if (!reread || EJSON.stringify(reread) !== EJSON.stringify(a)) reason = "agent_changed";
    if (reason) { emit("review", reason); continue; }
    if (latest <= cutoff) emit("candidate", "recorded_activity_older_than_threshold", {
      last_turn_activity_at: new Date(latest).toISOString(),
      inactive_seconds: Math.floor((observedAt.getTime() - latest) / 1000),
      observed_revision: integer(a.record.revision), observed_cursor: last
    });
  }
  print(EJSON.stringify({scanned: Math.min(PAGE, page.length), emitted,
    next_after_id: page[Math.min(PAGE, page.length) - 1]?._id ?? afterId,
    has_more: page.length > PAGE, threshold_seconds: THRESHOLD_MS / 1000}));
})();
```

Only IDs, statuses/reason codes, counts, sequence numbers and timestamps leave the
query. No prompt, answer, tool output, profile, token, title or failure detail is
projected. Non-lifecycle metadata is conservatively counted as activity. No
candidate from a partial/error run should be acted on without a fresh complete
read. Repeat discovery immediately before review; concurrent production activity
and API-only buffered text make this a nomination, not a consistent native proof.
Do not automatically loop through all pages or infer that an empty page means
there are no candidates elsewhere. Operators must record scan coverage.

## Owner decisions and exact effects

For each exact Agent/turn, identify its command/Response through existing owner
reads, confirm whether it has already settled, and record the owner's decision
before acting. Do not resend the prompt, replay tools, patch Mongo, or claim a
native final answer was published into CMA. Do not reuse the abandoned repair
branch as an operational command. A 202 response is admission, not completion.

| Decision | Existing operation and effect |
|---|---|
| Owner stop | `POST /api/v1/agents/{id}/stop`, JSON `{"reason":"owner"}`, with owner authentication and a unique `Idempotency-Key`. Asynchronously fences interaction, closes the process, then publishes Agent `Stopped` with `stop_reason=owner`, `stopped_at`, cleared requested-stop intent and settled lifecycle work. It retains conversation identity, generation, **active_turn**, recorded messages/usage, home and worktree. It emits `Stopped`, not `TurnInterrupted`/`TurnCompleted`. An unfinished command becomes **Failed / RuntimeLost** on subsequent command observation; Response v2 becomes `failed` with `error.code=runtime_lost`. Existing terminal evidence wins if completion raced the stop. Verify actual results; this is disposition, not faithful native terminal replay. |
| Owner delete | `DELETE /api/v1/agents/{id}` with owner authentication and its own `Idempotency-Key`. It is accepted only for `Stopped` or `Failed`, without pending stop intent; **stop a Working Agent and verify first**. Cleanup ensures process absence, removes the Agent home (including native history) and worktree, then publishes `Deleted`/`deleted_at` and a `Deleted` event. It retains durable Agent/log records rather than synthesizing a turn terminal; transition does not clear a retained active-turn field. It does not change a failed command into success. Capture command/Response results and any approved export before deletion. Verify `Deleted` and removed worktree; absence from a list alone is insufficient. |
| Keep waiting | No mutation. Preserve current Agent/command/Response state; owner chooses a next review time. Age alone is insufficient to stop legitimate work. Record any later natural completion or later owner decision. This is an explicit open disposition, not a repaired or successfully completed turn. |

Current source for these effects:
`api/crates/cma/src/sandbox/agent_worker/cleanup_publication.rs:59`,
`api/crates/layer2-core/src/sandbox/agent.rs:247` and `:352`,
`api/crates/layer2-core/src/sandbox/agent_io/evidence.rs:330`,
`api/crates/layer1-public/src/responses/v2/projection.rs:88` and `:248`, and
`api/crates/cma/src/sandbox/agent_worker/cleanup_retired.rs:97`.
Stop/delete are not a temporary pause/reopen procedure. Do not promise continued
input into the stopped Agent. The stop operation may interrupt work that is still
native-active, which is why the owner chooses the disposition explicitly.

## Optional owner-only native answer export

An existing read-only path is available:
`GET /api/v1/agents/{id}/history`, returning `application/x-ndjson` with
`Cache-Control: no-store`. It is restricted to the personal owner, not a shared
reader; authority and target are rechecked after runtime I/O. See
`api/crates/layer1-public/src/rest/agent_reads.rs:143` and
`api/crates/layer2-core/src/sandbox/agent_reads/history.rs:23`.

With the owner's explicit authorization, download to an owner-controlled private
location **before delete**, hash the file and inspect the exact native session/
turn. Export reads home files while the Sandbox runtime is reachable, independently
of the Agent bridge; it can work after stop. A missing runtime, missing exporter,
changed authority, malformed/oversized history, or unavailable home can refuse it.
Do not restart a Sandbox, seek new credentials, or bypass authorization to obtain
it. Use no new endpoint or process execution path.

If the native history contains a final answer, the owner may retain an exact
excerpt with its session/turn association. Acknowledge uncertainty when association
or phase is ambiguous. The export does not prove the original public terminal,
repair a command/Response, or publish an answer into CMA. Keep exported bytes and
customer content out of issue comments and disposition rows; record only the
artifact's hash and restricted evidence reference, byte count, IDs and outcome.

## Disposition ledger and verification

Maintain one row per historical turn, keyed by Agent ID + turn sequence. Never
invent the missing seven identities or attach new candidates to historical rows
without evidence. Record unmapped rows as `unidentified`; finding fewer current
candidates does not establish eight dispositions. Do not double-count one turn
across multiple command reads/retries.

| Field | Required value |
|---|---|
| Row and identity | Historical row number; Agent, Sandbox, command, Response IDs; turn sequence; identity-evidence reference. Unknown values remain `unknown`. |
| Before | Observation UTC; deployed source/image; Agent status/revision/cursor/active_turn; command/Response status/error code; last turn activity; preserved-answer/usage evidence hash without content. |
| Owner decision | Approval reference and UTC for stop/delete, or keep-waiting decision and next review UTC. Approval must cover the exact Agent and destructive deletion separately. |
| Action | `owner_stop`, `owner_delete`, `keep_waiting`, `settled_otherwise`, `already_deleted`, or `unidentified`; operation UTC, idempotency receipt/reference; no resend. |
| After | Verified UTC; Agent status/active_turn/work state; command/Response status/error; actual terminal event count/type; usage comparison; no released queued-input observation; cleanup result where applicable. Do not require an invented turn terminal for stop/delete. |
| Native export | `not_requested`, `unavailable`, or `exported`; exact session/turn association, byte count and SHA256, restricted evidence reference; never answer text. |
| Disposition | Confirmed result or open reason; operator reference; owner acknowledgment; follow-up date/issue. |

Production operators run discovery, map identities and collect before/after reads;
owners authorize operations or waiting and any export. Owners/operators use the
existing authenticated public operations, wait for their actual completion, and
record public effects. Any error, changed identity or unconfirmed cleanup remains
open in the ledger. No production disposition was performed during this work.

## Future runtime capability

The new issue must qualify a versioned, capability-detected contract rather than
infer completion from elapsed time, equal history exports or a task marker:

- Exact Agent/home/native-session/prompt-turn, runtime generation/incarnation and
  original typed terminal evidence, including cancellation/error metadata and
  normal completion without final text; hold unsupported or ambiguous histories.
- A completion-correlated output boundary proving all A output is accounted for,
  including future notification/error/goal activity. Preserve ACK-after-commit and
  retained data; never ACK or discard unproved output to manufacture a boundary.
- Durable per-attachment writer fencing across API replicas and uncertain commits,
  with per-Agent input gating through the normal fenced terminal transaction.
- Supported reopen after a repair close on the same Agent/home/native session,
  with explicit process/journal finality; no global downtime or identity replacement.
- Durable drain before dispatch, covering both retained output and fresh
  session/load history. B's transcript must contain only `New B answer.`.
- Independent-store Mongo races, old/fresh observers, controls/parent changes,
  unknown commits, no-final-text completed Response, deterministic receipts,
  unchanged usage, no model/prompt resend and no unrelated queued-input release.
- Operator-only dry-run/read-only discovery and explicit owner-approved targets;
  background automation remains separate in #1427. No retrospective capability or
  faithful outcome may be claimed for legacy turns lacking the required evidence.
