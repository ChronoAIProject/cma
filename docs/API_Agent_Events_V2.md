# Agent event replay v2

This CMA extension carries complete Agent chronology beside the standard
Responses API. Submit input through Responses v2, retain its command/response
receipt, and render conversation history from the Agent event stream. Response
`sequence_number` values and Agent replay cursors identify different streams.

## Transport eligibility

Task experience eligibility is independent of transport selection. Authenticated
`GET /api/v2/ui/task-experience` returns `Cache-Control: private, no-store` and:

```json
{"object":"cma.ui.task_experience","schema_version":1,"account_id":"current-caller","capabilities":{"task_home_admission":false,"suggested_followup_admission":false,"desktop_enable":false,"browser_takeover":false,"replay_presentation":false}}
```

Only the authenticated caller's identity and selected capabilities are disclosed;
cohort ids are never returned. Fence this observation to the current viewer and
refresh on identity/focus changes. Errors, unsupported servers and unknown schemas
do not authorize new entry. Policy eligibility does not prove an environment's
capability. New task creation, answer-scoped follow-ups, desktop enablement and
takeover are enforced by the server after current authorization and exact accepted
recovery. Replay is a presentation switch; authorized history remains readable.
Closing takeover stops new acquire/input/renew, while status, original receipt
recovery and explicit return remain available under current authority. Keep the
mounted return UI and the unchanged native15-second lease. Never clear pending
keys, transport selection, drafts or retained records on eligibility changes.
See the bounded `19-ROLLOUT-POLICY.md` component in the UI optimization plan for
configuration and reader-floor requirements; it is not combined19 acceptance.

`GET /api/v2/ui/transport` requires authentication and returns
`Cache-Control: private, no-store`. Its `enabled` value is the process-start
`CHRONO_V2_UI_TRANSPORT_ENABLED` flag combined with composed Responses,
targeted controls, Agent replay and live-update dependencies. The flag defaults
to false. The response has this shape:

```json
{
  "object": "cma.ui.transport",
  "schema_version": 1,
  "enabled": false,
  "selection": "new_tabs",
  "input": "responses_v2",
  "snapshot_schema": 1,
  "event_schema": 1,
  "control_schema": 1,
  "live_fragments": true,
  "snapshot": "/api/v2/agents/{agent_id}/events/snapshot",
  "events": "/api/v2/agents/{agent_id}/events",
  "controls": "/api/v2/agents/{agent_id}/responses/{response_id}/controls",
  "command_status": "/api/v2/commands/{command_id}"
}
```

Selection applies when a new tab chooses its transport. Existing AG-UI tabs keep
their transport. A v2 tab retains its read and receipt-recovery routes across
flag reversal. The flag supplies no command authorization. An older server,
unknown schema or unavailable discovery route must not cause an uncertain v2
submission to be resent through v1.

The production ConfigMap enables this selector for new Agent tabs. Rollout requires
the matching API/Web release, a compare-and-set ConfigMap update, and an API rollout
so every replica advertises the intended eligibility. Existing tabs retain their
selected transport; no accepted request changes protocol on rollout or reversal.

Creation reserves the actual Agent, Worktree and exact profile Workspace before
runtime preparation finishes. A Starting Agent can open after its owner and full
lineage are verified. A fresh Active parent without a lifecycle operation or
checkpoint continues to allow durable v2 input. While the parent is Provisioning or
Installing, input additionally requires the Agent detail's `bootstrap` object with
the original lifecycle `operation_id`, `sandbox_id` and `workspace_id`. The server
derives this proof from the exact original parent/profile lineage and a reservation
with no runtime effects. A generic Starting status or an operation receipt alone
does not grant this admission.

The untouched reservation has an empty `base_commit` and starts at Agent work step
`wait_for_workspace`. This represents an unresolved base, never a Git revision.
The worker waits for the actual Workspace commit and an Active credentialed parent.
One transaction then verifies and write-fences both parent records, binds that
actual commit, and advances to `add_worktree`. Runtime setup starts after this
publication. Stop, deletion and terminal parent failure handle an untouched
reservation without attempting runtime cleanup for effects that never occurred.
Such a cancelled reservation may retain the empty base through its cleanup states.

Starting v2 tabs use the existing exact-body/idempotency-key journal and clear
drafts only after durable admission. Execution waits for Ready and remains behind
the creation first-message barrier. The UI continues to show Starting while this
work runs. Existing AG-UI tabs wait for Agent readiness before input. Stale parent
or bootstrap reads, access uncertainty, checkpoints and lifecycle operations hold
input; recovery of an uncertain v2 submission is an explicit action using the
retained body and key, with no automatic resubmission.

Early navigation retains the owner/scope-bound creation body, key, operation ID,
and observed Agent ID. The Agent page continues lifecycle observation and clears
that creation intent only on confirmed success. If a worker retry requires a new
connection check, **Continue Agent setup** verifies the operation's connection
target and resumes the same creation body/key; reload alone never resumes it.

## Current live baseline

`GET /api/v2/agents/{agent_id}/events/head` reads the current durable head and
its active turn without loading the historical log. It requires the same
`ViewHistory` authority as snapshots and tails, revalidates held authorization
immediately before publication, and returns `Cache-Control: private, no-store`.
It accepts no query parameters.

```json
{
  "object": "cma.agent.event_head",
  "schema_version": 1,
  "agent_id": "agt_example",
  "incarnation": "opaque_incarnation_digest",
  "through_cursor": "opaque_agent_cursor",
  "active_turn": null
}
```

For an active turn, `active_turn` is its exact durable `turn_started` snapshot
item, including `cursor`, `sequence`, `appended_at`, safe `kind` and verified
`response_binding`. Legacy turns retain a null binding. The endpoint reads at
most that one historical row and never returns private source metadata. An
invalid active-turn pointer or unverifiable common-command binding fails closed.

Clients can establish a live tail after this `through_cursor` while independently
loading transcript and snapshot history. This baseline does not acknowledge
historical rows. Keep tail events ordered and reconcile them by durable sequence
when history is applied. The returned head and active-turn identity both belong
to the captured aggregate: completion or new work racing the read is delivered
by the tail, even if it commits before the head response is published. The
baseline carries no control authority; observe response controls before a new
control intent.

## Snapshot bootstrap

The Computer operation replay also uses this public query, independently of the
selected conversation input transport. `Prefer: cma-tool-output` includes bounded
recorded output. Its index uses Agent incarnation, recorded turn sequence, tool
identity and durable event cursor. A selected record freezes the reported title,
status, output, timestamp and preceding plan from that turn. It does not project
current files, browser images or current plans into the past.

The Web reader retains at most 50 operation records per window and renders one
selected record. Earlier windows and reloads reread authorized snapshot pages;
historical folds stop at the selected cursor. Live catch-up uses the existing
finite `events?follow=false&after_cursor=...` read from the last applied cursor,
without rescanning the historical prefix or opening another live subscription.
Return Live reads a fresh fixed-head snapshot. Querying these surfaces never
admits or recovers commands, approvals, feedback, suggestions or control.
Missing events, `history_gap`, expired positions and authorization failures must
be explained instead of silently substituting current state. The retained event
contract declares no expiry deadline or screen recording capability.

`GET /api/v2/agents/{agent_id}/events/snapshot` captures a durable high-water
mark and returns a chronological page. `limit` is 1–200, default 100. Follow
`next_page_cursor` with the `page_cursor` query parameter until it is null.
Subsequent pages retain the first page's high-water mark even while new events
are appended. Responses are bounded to 4 MiB and may contain fewer than the
requested number of rows. A row that does not fit is deferred intact.

```json
{
  "object": "cma.agent.event_page",
  "schema_version": 1,
  "agent_id": "agt_example",
  "incarnation": "opaque_incarnation_digest",
  "through_cursor": "opaque_agent_cursor",
  "items": [
    {
      "cursor": "opaque_agent_cursor",
      "sequence": 42,
      "appended_at": "2026-09-16T00:00:00Z",
      "kind": { "type": "turn_started", "turn_sequence": 42 },
      "response_binding": {
        "command_id": "cmd_example",
        "response_id": "resp_example",
        "turn_sequence": 42
      }
    }
  ],
  "next_page_cursor": null
}
```

When bootstrapping sequentially from the snapshot, apply every page successfully
before acknowledging `through_cursor` and starting the tail. The independent
head baseline above permits live observation during this history load. The cursor
is an event watermark, not an aggregate revision. Observe the current Agent revision through the response-controls API when preparing a
new control intent.

`kind` uses the safe core `AgentEventKind` vocabulary. Messages retain their
role/content; tool starts retain call ID, title, kind and initial status; updates
retain status and optional title. Turn outcomes, approvals, delivery changes,
usage, mode, plan and safe resource events retain their durable order. A row
withheld by the caller's visibility policy has `kind: null` and still advances
the source sequence and cursor. Native conversation identities, credential
material, raw command origin and approval runtime locators are not projected.

`response_binding` is present only on the exact turn-start row backed by a
verified common Message command. The server verifies command identity, Agent,
current owner/incarnation, durable dispatch turn and response receipt. Genuine
legacy turns have a null binding. A common-command origin whose record cannot
be verified fails the read; it does not become a legacy turn. A null binding
does not authorize an Agent-wide control fallback.

Clients can request `Prefer: cma-message-bindings` on snapshot and event-tail reads.
With that preference, a verified steering user-message row may include
`message_command_id`. The server binds the immediately preceding immutable
steering dispatch fence to its unique command ledger record under current
history authority, and verifies Agent, owner, incarnation and dispatch identity.
The binding survives page boundaries and reconnects. It never derives identity
from message text or the original response's command. Non-ledger legacy steering
stays unbound. Only the first user input of an originally bound turn may use its
`response_binding.command_id` as a legacy fallback.

The preference is optional and unknown preferences are ignored by older servers.
Without it the event shape remains unchanged, so strict older clients retain
compatibility. `message_command_id` is omitted when unavailable and is valid only
on a user message; it never authorizes a command or broadens resource access.

## Tail and live frames

`GET /api/v2/agents/{agent_id}/events?after_cursor=...&follow=true` replays after
the last acknowledged Agent cursor and follows further events. `follow=false`
ends after the captured head. An omitted cursor starts at the beginning of
retained history. `Last-Event-ID` can supply the cursor instead; if both forms
are present they must agree. Duplicate headers/query keys, unknown fields and
invalid values fail explicitly.

| SSE event | Data and acknowledgement |
| --- | --- |
| `cma.agent.event` | One complete snapshot-item projection, plus `agent_id` and `incarnation`. SSE `id` equals `cursor`. Apply the complete item before acknowledging. |
| `cma.agent.live` | `agent_id`, `incarnation`, `turn_sequence`, `after_sequence`, `message_id`, `kind` (`text_delta` or `reasoning_summary_delta`) and `delta`. With `Prefer: cma-live-observed-at`, also `observed_at` (RFC 3339 server live-fragment observation time). No SSE ID or durable cursor. |
| `cma.agent.live_gap` | `agent_id`, `incarnation`, nullable `turn_sequence`, nullable `frames`, and `reason` (`lagged` or `reconnected`). No SSE ID. Reconnection cannot establish a lost-fragment count. |

The live timestamp preference is opt-in so existing strict clients keep their
unchanged event shape. New clients accept its absence during a rolling upgrade.
`observed_at` records when the API observes and prepares that live fragment; it
does not claim the provider's generation time or turn start. A message UI keeps
the first such observation across subsequent fragments, then uses the committed
message's authoritative `appended_at` when it arrives. Without the preference,
or when an older server omits it, clients do not invent an unreported live time.
| `cma.agent.error` | Safe `code` and `message`. No SSE ID. Revoked/expired authorization closes without releasing further resource information. |

The server subscribes to live updates before reading the initial head, catches
up durable history first and validates runtime generation/active turn internally.
A future-anchored fragment waits for its durable sequence. A committed assistant
message suppresses superseded text fragments. Consecutive fragments whose
message, kind, turn, runtime generation and `after_sequence` agree may be
coalesced, and each resulting `delta` is capped at 16 KiB (the original
live-fragment byte bound). A live batch carries at most 128 source fragments.
Each subscriber's in-memory lane retains 256 source fragments (previously 32),
so the same short burst can now arrive without a lag gap. These bounds count
source fragments before coalescing; clients concatenate `delta` values and do
not infer source-fragment counts from the number of live SSE frames. The frame
shape is unchanged. The shared lag contract deliberately overflows the lane by
eight fragments and alternates message/reasoning kinds, preserving its exact
eight-fragment gap and first retained delta across coalescing.
Live authorization and head validation happen once per batch; when the durable
head advances, still-valid buffered fragments wait behind the
durable catch-up and then resume in their original order. Resuming rechecks the
Agent identity, runtime generation, active turn, committed assistant message
and durable head against the retained fragment metadata. Durable completion or
a superseding assistant message covers obsolete fragments, which are dropped
without a gap. A runtime-generation change in the same active turn counts the
invalidated fragments and produces an explicit `lagged` gap. Broadcast lag also
produces a `lagged` gap, after fragments received before that lag and before any
later fragments. Reasoning summaries remain ephemeral; snapshots, response
retrieval and retained receipts cannot reconstruct missed reasoning.

Every request authorizes the actual caller. Durable and gap frames revalidate
current policy and exact resource lineage after waiting under consumer
backpressure; each live batch revalidates before releasing its first fragment.
Stream membership evidence does not renew through polling. Dependency IO is
bounded to 15 seconds and to any earlier membership deadline.

## Cursors and failures

`Prefer: cma-tool-output` enables the optional `output: {text, truncated}` field
on `tool_updated`, and the `tool_output_reported` event with `call_id` and
`output` only. It composes with the existing preference tokens. Output-only ACP
updates do not invent a status. Values are bounded reported text snapshots,
not deltas to concatenate, terminal input, stdout/stderr channels or exit codes.
The core retains at most 16 KiB of valid UTF-8 per output record. It extracts
ACP text content or the adapter's explicitly reported formatted output;
metadata-only transport deltas do not create one durable record per byte chunk.

Without this preference, output fields are omitted and output-only entries have
`kind: null`, preserving their cursor positions. Existing history authorization,
Agent incarnation and turn boundaries apply. Old logs without output remain
valid. Once new output records are written, older core binaries that reject the
new durable variant/fields cannot read them: downgrade requires a compatible
reader or restoring pre-upgrade data. There is no automatic rollback migration.

Cursors are opaque incarnation-bound coordinates, never authorization.
Current encoding uses `ag1.<digest>.<sequence>`; snapshot page tokens use the
distinct `ags1.<digest>.<through>.<after>` family. The digest binds Agent ID and
creation timestamp with domain separation. Consumers must retain the complete
opaque cursor separately from numeric event chronology and Responses sequences.

The current store retains the complete immutable log prefix. There is no
claimed retention expiry. Missing/gapped history or orphan rows are unavailable
history, not successful truncation. HTTP errors use the existing RFC problem
document envelope:

| Status | Code |
| --- | --- |
| 400 | `invalid_agent_events_query`, `invalid_agent_cursor`, `agent_cursor_beyond_head` |
| 401 | `not_authenticated` |
| 404 | `agent_not_found` (missing and inaccessible resources share this result) |
| 409 | `agent_incarnation_changed` |
| 503 | `agent_history_unavailable`, `agent_read_unavailable` |

Clients reconnect only the read stream. Retain the exact key/body before a
Responses or control POST, settle admission only from its durable receipt and
recover uncertainty using the same request identity. Observation abort does not
cancel a turn. A control receipt records delivery progress; the durable turn
outcome determines whether cancellation occurred.

See [Responses](API_Responses.md) for input and response retrieval and the
response-controls contract for source-bound cancel, steer and approval delivery.
