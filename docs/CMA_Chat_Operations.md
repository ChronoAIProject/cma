# CMA Chat operations and release evidence

This is the enablement, evidence, rollback, and incident runbook for CMA Chat,
defined by [issue #741](https://github.com/ChronoAIProject/cma/issues/741).
The API contracts in [API_REST.md](./API_REST.md),
[API_Responses.md](./API_Responses.md), and
[API_Overview.md](./API_Overview.md) remain authoritative where this runbook
omits a product rule. `CLAUDE.md` section 5 remains the continuity authority.

**Runbook owner:**
[`chronoai-shining`](https://github.com/chronoai-shining). The owner keeps the
release ledger and drill attachments current. Runbook ownership does not grant
access to another owner's messages, repository bytes, credentials, checkpoint
objects, or provider resources.

**Evidence status:** deterministic repository tests cover the G5 through G9
control-plane contracts. They do not prove behavior of an unbuilt release
image, a deployed provider, a staging control-plane restart, or a third-party
SDK version. All live G1 through G9 records, the release image digest, provider
versions, official SDK versions, transcripts, timings, and the staging
enable/disable drill are **pending deployment-owner validation**. Nothing in
this document claims that those live checks have run.

Chat ships dark. Do not set `chat.enabled` to `true` in a deployment until the
release ledger for that exact commit and image records all nine gates as live
passes.

## 1. Scope and non-negotiable boundaries

- A repository test pass is **code evidence**. It may support a bounded claim
  about state transitions, fencing, persistence, projection, or validation. It
  is not a staging transcript and cannot be promoted to one.
- A **live pass** names the exact CMA commit, immutable runtime image digest,
  kernel versions, provider and chart versions, deployment, official SDK
  versions where applicable, UTC interval, operator, redacted transcript, and
  cleanup result.
- An omitted version, transcript, or cleanup check makes live evidence
  incomplete. Mark it `PENDING` or `FAIL`; never infer a pass.
- G1 through G4 remain required release gates even though their provider probes
  are outside repository verification. Follow
  [issue #747](https://github.com/ChronoAIProject/cma/issues/747) before Chat is
  enabled.
- G5 through G9 must be repeated against the exact release image. The Linux
  writer-fence probe in G6 cannot be replaced by a non-Linux unit-test run.
- `chat.enabled: false` prevents new Chat hosts and threads. It is an admission
  control, not permission to terminate a runtime, discard a checkpoint, alter
  retained history, or weaken continuity for an existing host.
- Existing hosts pin the settings revision with which they were created.
  Editing settings does not mutate them. Rollout to an existing owner requires
  explicit host retirement through the documented fenced operation.
- Administrative retirement is not an emergency substitute for disabling new
  admission. It terminates the retained threads with a durable revocation
  reason after a verified capture.
- Never manually delete a provider runtime while CMA still records it as the
  authoritative runtime. Never delete or rewrite a workspace or conversation
  checkpoint to clear an incident.
- Never retry an ambiguous provision, pause, revive, rotation, or retirement by
  creating a second provider runtime. Reconcile durable host state and provider
  metadata first.
- No prompt, response text, reasoning text, repository content or path,
  configured command, command output, checkpoint reference or digest, bearer
  token, cookie, authorization header, or provider response body may enter an
  alert, structured log, issue, or unredacted evidence attachment.
- Successful setup output is not an operational log. A failed setup excerpt is
  already bounded to one UTF-8-safe 32 KiB combined stdout/stderr tail, but it
  can still contain owner data and must be redacted before attachment.
- Do not perform release probes against production owner data. Use isolated
  staging owners, repositories, runtimes, and provider metadata, then remove
  only the disposable resources created by the drill.

## 2. Evidence classes and release record

Use these exact status values in the release ledger:

| Status | Meaning |
|---|---|
| `CODE_PASS` | The named deterministic command passed at the recorded commit and platform. |
| `LIVE_PASS` | The named live procedure passed against every recorded release component. |
| `PENDING` | Required evidence has not run or its record is incomplete. |
| `FAIL` | The procedure contradicted an acceptance criterion. Chat stays dark. |
| `BLOCKED` | A named external prerequisite prevented execution. Chat stays dark. |

A live gate is complete only when its record uses this template and links its
redacted attachments:

```yaml
gate: G5
status: PENDING
bounded_claim: ""
utc_started_at: ""
utc_finished_at: ""
operator: ""
deployment: ""
cma_commit_sha: ""
cma_release_identifier: ""
runtime_image_reference: ""
runtime_image_digest: ""
operating_system: ""
kernel_version: ""
codex_version: "@openai/codex@0.147.0"
codex_acp_version: "codex-acp@1.4.0"
provider_name: ""
provider_api_version: ""
provider_chart_version: ""
settings_revision: null
openai_python_version: null
openai_node_version: null
fixture_owner_ids: []
chat_host_ids: []
conversation_ids: []
commands_attachment: ""
redacted_transcript_attachment: ""
structured_log_interval_attachment: ""
provider_inventory_before_attachment: ""
provider_inventory_after_attachment: ""
timing_attachment: ""
cleanup_result: ""
reviewer: ""
reviewed_at: ""
notes: ""
```

Do not replace an empty field with a guess. Hash each attachment and retain the
hash beside the issue link so later edits are visible. Redaction must preserve
event types, sequence numbers, statuses, durations, command indexes, byte
counts, and opaque object identities needed for the claim while removing
content and credentials.

## 3. Reproducible code evidence for G5-G9

Run from a clean checkout of the exact candidate commit. Record the command,
host OS and architecture, Rust toolchain from the repository, exit status, and
complete test summary. The final PR CI run is the release-candidate code
record; local results are supporting evidence.

The commands below deliberately select semantic tests by their complete unique
names. A command that reports zero selected tests is not a pass.

| Gate | Repository evidence | Live status when this runbook was written | Required live remainder |
|---|---|---|---|
| G5 | Layer 2 host lifecycle plus Layer 4 conversation and git-worktree tests | `PENDING` | Two-window dirty-worktree pause/revive on staging |
| G6 | Admission-race, failure-recovery, and Linux descendant-writer tests | `PENDING` | Byte-stable writer-fence capture on the exact Linux release image |
| G7 | Durable pausing/reviving restart and duplicate-runtime reconciliation tests | `PENDING` | Two real control-plane termination drills on staging |
| G8 | Server-side Responses fixtures and end-to-end route contract tests | `PENDING` | Pinned official Python and Node SDK executions, including extension tolerance |
| G9 | Layer 4 setup execution plus provision/revive/reconcile tests | `PENDING` | Elan install, retained no-op, and typed failure on the release image |

`PENDING` in this table refers to the live gate. A linked CI run can establish
`CODE_PASS` without changing it.

### G5: pause and revive round trip

```sh
cd api
cargo test -p layer2-core --lib quiet_active_host_seals_every_durable_conversation_and_complete_git_workspace
cargo test -p layer2-core --lib successful_pause_parks_every_thread_then_fenced_deletes_the_runtime
cargo test -p layer2-core --lib paused_message_restores_every_thread_and_delivers_the_admitted_turn
cargo test -p layer2-core --lib chat_threads_share_one_host_and_pin_each_worktree_and_conversation
cargo test -p layer2-core --lib paused_chat_retains_linear_metadata_through_revival_and_delivery
cargo test -p layer4-runtime --lib the_public_capture_route_selects_by_id_and_preserves_newest_without_one
cargo test -p layer4-runtime --lib several_carries_bind_distinct_slots_before_attach_and_every_late_carry_is_refused
cargo test -p layer4-runtime --lib local_clone_is_idempotent_and_worktrees_round_trip
```

Bounded code claim: one quiet shared host captures each retained conversation
by id plus a complete git workspace, publishes verified checkpoint references,
parks every thread with `idle_timeout`, deletes the fenced runtime only after
publication, restores every slot and the workspace into a new runtime
generation, preserves linear response metadata, and delivers the one admitted
message. The Layer 4 tests prove keyed conversation selection and local git
worktree round trips. They do not prove provider clone egress, real archive
storage, release-image git behavior, or a staging pause timer.

### G6: writer-fence quiescence under full access

```sh
cd api
cargo test -p layer2-core --lib checkpoint_claim_fences_new_prompt_admission_until_capture_settles
cargo test -p layer2-core --lib turn_start_between_scan_and_fence_close_aborts_without_capture
cargo test -p layer2-core --lib capture_failure_reopens_writers_and_leaves_the_host_active
cargo test -p layer2-core --lib failed_writer_resumption_leaves_an_explicitly_recoverable_pausing_host
cargo test -p layer4-runtime --lib a_live_descendant_after_primary_exit_is_still_quiesced_by_checkpoint_authority
```

The final command is compiled and executed only on Linux. Record a zero-test
result on another operating system as `PENDING_LINUX`, not `CODE_PASS`.

Bounded code claim: the durable checkpoint claim closes prompt admission,
rechecks an in-flight turn after acquiring the gate, refuses capture if the
turn won the race, resumes writers after a capture refusal, retains recovery
authority if writer resume is ambiguous, and on Linux includes a surviving
background descendant in the writer fence after its primary process exits.
The tests do not prove the process-tree behavior of an untested release image
or byte stability through a real provider checkpoint.

### G7: restart during pausing and reviving

```sh
cd api
cargo test -p layer2-core --lib restart_replays_the_exact_inactivity_pause_claim
cargo test -p layer2-core --lib restart_finishes_a_published_pause_after_the_old_runtime_is_gone
cargo test -p layer2-core --lib interrupted_pause_recovers_only_from_the_previous_published_checkpoint
cargo test -p layer2-core --lib restart_adopts_one_existing_revive_generation_and_fences_ambiguous_prompt
cargo test -p layer2-core --lib reconciliation_keeps_the_recorded_runtime_and_removes_a_duplicate
```

Bounded code claim: reconciliation resumes the exact durable pause claim,
finishes a published pause without recapturing a deleted runtime, falls back
only to the previously published recovery point after interrupted capture,
adopts one provider generation during revive without redelivering an ambiguous
prompt, and removes a duplicate while retaining recorded authority. These
tests do not prove Kubernetes termination timing, provider discovery, or
staging boot behavior.

### G8: Responses projection and official SDK boundary

```sh
cd api
cargo test -p layer1-public --lib responses_sdk_fixtures_are_server_projections
cargo test -p layer1-public --lib conversation_projection_uses_the_server_owned_chat_title
cargo test -p layer1-public --lib response_stream_folds_live_reasoning_text_tools_and_durable_completion
cargo test -p cma --lib responses_full_turn_is_streamed_retrieved_paginated_and_not_body_owned
cargo test -p cma --lib response_post_revives_a_paused_chat_thread_before_streaming_the_turn
cargo test -p cma --lib responses_refusals_are_named_authenticated_and_bounded
cargo test -p cma --lib responses_conceal_foreign_non_chat_and_invalid_cursors
```

Bounded code claim: the public adapter projects server-owned conversations and
titles, streams standard response lifecycle, reasoning, output, usage, and the
namespaced `response.cma_chat.tool_activity` extension, reconstructs durable
responses and newest-first item pages, revives before opening a paused turn,
and preserves authentication, concealment, and closed-field refusals. These
are server-side fixtures. They are not an execution of `openai-python` or
`openai-node`, and they do not prove that a selected SDK version tolerates an
unknown event or item type. That part of G8 remains live-only.

### G9: setup-command round trip

```sh
cd api
cargo test -p layer4-runtime --lib commands_run_in_repository_order_and_stop_at_the_first_failure
cargo test -p layer4-runtime --lib combined_output_keeps_only_one_utf8_safe_32_kib_tail
cargo test -p layer4-runtime --lib deadline_returns_a_nonzero_step_and_kills_the_command_session
cargo test -p layer4-runtime --lib public_route_uses_a_restored_repository_and_latches_first_conversation
cargo test -p layer4-runtime --lib setup_waits_for_the_startup_gate_and_a_fresh_incarnation_is_admitted
cargo test -p layer2-core --lib nonzero_setup_persists_the_bounded_failure_evidence_and_cleans_runtime
cargo test -p layer2-core --lib restart_reconciliation_adopts_metadata_runtime_and_replays_setup
cargo test -p layer2-core --lib revive_setup_failure_retains_index_and_excerpt_through_queued_admission
```

Bounded code claim: commands run in repository order, in the restored main
checkout, stop at the first nonzero result, retain one UTF-8-safe 32 KiB
combined tail, time out and stop the command session, serialize with startup,
and refuse after a conversation opens. Provision and revive run setup before
conversation attachment; typed failures retain the zero-based index and
bounded excerpt, clean or retain runtime authority according to the transition,
and survive restart reconciliation. These tests do not download Elan, exercise
real egress, or time the no-op recipe on the release image.

## 4. Live evidence procedures

All procedures in this section are pending until a deployment owner executes
them in isolated staging. Use the exact release candidate and image intended
for rollout. Stop on the first contradiction. Do not continue merely to fill
the rest of the ledger.

### 4.1 Shared prerequisites

1. Record the complete version template from section 2 before creating a
   fixture.
2. Confirm G1 through G4 are `LIVE_PASS` on the same image and provider. A
   different digest or provider invalidates the inheritance.
3. Confirm the runtime advertises the versioned workspace-source,
   conversation-slot, and workspace-setup capabilities required by Chat.
4. Confirm `@openai/codex@0.147.0` and `codex-acp@1.4.0` from inside the image;
   do not infer them from a Dockerfile or package lock alone.
5. Confirm the staging operator has `configure_platform`,
   `read_all_sessions`, and `control_any_session`, plus separately authorized
   read-only provider inventory and checkpoint-evidence access.
6. Tag every disposable provider runtime with the gate, candidate commit, UTC
   run id, and staging fixture owner. Capture provider inventory before the
   first request.
7. Use fresh owner identities and conversations. Do not use production owner
   data or a repository containing credentials.
8. Keep Chat dark outside the isolated staging interval. Record every settings
   revision and use revision CAS; never silently retry a settings conflict.
9. Open a bounded structured-log interval before the procedure. Verify the
   interval contains no message, response, reasoning, repository, command, or
   credential body before attaching it.
10. After the gate, disable Chat, reconcile CMA records, list provider fixtures,
    and remove only resources whose recorded run-id tag matches the drill.

### 4.2 G5 staging pause/revive

1. In isolated staging, use a complete Chat declaration with a short permitted
   pause window of 60 seconds. Record that this is a drill-only deviation from
   the intended 900-second setting.
2. As one fixture owner, create two conversations through two independent Chat
   windows or two independently constructed official clients. Record their
   conversation ids and confirm the admin inventory reports one host with
   `thread_count: 2`.
3. In the first conversation, direct the agent to create an uncommitted UTF-8
   marker file under its worktree and report only its SHA-256 and relative
   path. Do not attach the file contents. In the second, execute a distinct
   short turn and record distinct response and tool-call ids.
4. Record each worktree name and branch. Verify they are different, both live
   below `worktrees/`, and neither conversation can see the other's rollout
   identity.
5. Let both conversations become quiet. Do not manufacture an old database
   timestamp. Wait for the configured boundary and the next one-minute sweep.
6. Confirm the host progresses through `pausing` to `paused`, every thread is
   `idle` with `stop_reason: idle_timeout`, each public projection carries the
   server-composed pause notice, `runtime_id` is no longer retained publicly,
   and provider inventory contains no live runtime for that host generation.
7. Send exactly one message to the first paused conversation. Confirm admission
   waits for revive, the host returns to `active` with one higher runtime
   generation, both conversation slots load, and the queued message is
   delivered once.
8. In both conversations, retrieve durable history and verify the pre-pause
   owner and assistant items remain in order. In the first worktree, verify the
   marker SHA-256, worktree registration, branch, HEAD, and `git status` are the
   same logical state as before pause and show no corruption. Verify the second
   worktree is also registered.
9. Confirm there is exactly one live provider runtime for the host and no
   orphan generation. Restore the 900-second candidate setting, disable Chat,
   and perform the shared cleanup.

The G5 bounded live claim may mention only the two recorded conversations, the
one recorded host, the specific dirty marker, and the observed pause/revive
interval. It is not a provider-wide durability claim.

### 4.3 G6 release-image writer fence

Run this gate on Linux inside the exact immutable release image, with Chat's
fixed unrestricted runtime permission. Preserve process ids and byte counts,
not repository content.

1. Create one fixture conversation and use a turn to start a detached
   descendant that appends monotonically numbered fixed-format records to a
   fixture file after its launching shell exits. Confirm from inside the
   sandbox that the descendant remains alive and the byte count advances.
2. Make the host otherwise quiet and allow the normal lifecycle checkpoint to
   begin. Capture the checkpoint request interval, writer count, archive byte
   count, archive SHA-256, and writer resume result through separately
   authorized internal evidence. Do not expose the checkpoint object reference
   in a public attachment.
3. Confirm prompt admission is refused while the checkpoint claim holds and no
   new `TurnStarted` crosses the capture boundary.
4. Verify the archive by reading it through the normal checkpoint store,
   recomputing its SHA-256, and checking the fixture file ends on one complete
   fixed-format record. Read the same immutable object twice and confirm the
   bytes are identical.
5. Let an opportunistic checkpoint resume the writer, or let an inactivity
   pause retain teardown authority. Confirm the recorded writer count includes
   the detached descendant in either disposition.
6. For the pause case, revive through one message and confirm the restored
   fixture file matches the verified archived bytes before any new agent turn
   can modify it. Confirm one live runtime and no duplicate.
7. Disable Chat and perform the shared cleanup.

The gate fails if the descendant remains writable during capture, a partial
record enters the archive, admission crosses the fence, archive verification
changes the bytes, writer disposition is ambiguous, or more than one runtime
survives.

### 4.4 G7 control-plane restart drill

Run two separate fixtures. Use the deployment's normal process supervisor to
terminate only the selected CMA control-plane process or pod. Record the exact
supervisor command in the evidence; this runbook does not assume a Kubernetes
workload or label name.

**Pausing restart**

1. Create two quiet threads on one host and retain a verified previous
   checkpoint.
2. Wait until durable admin state reports `pausing`. Terminate the selected
   control-plane instance once, without deleting the provider runtime or any
   checkpoint object.
3. Let normal boot reconciliation run. Confirm it either completes the exact
   published pause or restores the previous published checkpoint according to
   the durable boundary reached before termination.
4. Confirm both conversations remain, there is at most one authoritative
   provider runtime, and no idle-timeout event is duplicated.

**Reviving restart**

1. Start with a verified paused host. Send one uniquely identifiable prompt
   and wait until durable admin state reports `reviving`.
2. Terminate the selected control-plane instance once. Do not send another
   prompt and do not manually provision a replacement.
3. Let boot reconciliation discover provider metadata and durable prompt
   admission. Confirm it adopts exactly one generation, cleans a duplicate if
   the provider exposed one, loads every conversation, and settles the original
   admission without delivering the prompt twice.
4. Retrieve response history and verify exactly one owner item and at most one
   terminal assistant response belong to the admitted prompt. Confirm one live
   runtime and no lost conversation.

Record the phase boundary actually reached. A result from killing before the
durable transition claim or after it settled does not exercise G7.

### 4.5 G8 official OpenAI SDK conformance

Select explicit approved versions at drill time. The repository intentionally
does not manufacture version numbers. Record the resolver output and lockfile
or package-manager inventory, then run each client against the exact CMA
`/api/v1` base URL with a fresh bearer credential.

The call sequence follows the official OpenAI references for
[creating conversations](https://developers.openai.com/api/reference/resources/conversations/methods/create.md),
[retrieving conversations](https://developers.openai.com/api/reference/resources/conversations/methods/retrieve.md),
[listing conversation items](https://developers.openai.com/api/reference/resources/conversations/subresources/items/methods/list.md),
[creating responses](https://developers.openai.com/api/reference/resources/responses/methods/create.md),
[retrieving responses](https://developers.openai.com/api/reference/resources/responses/methods/retrieve.md),
and [streaming Responses](https://developers.openai.com/api/docs/guides/streaming-responses.md).

```sh
python3 -m venv .g8-python
.g8-python/bin/python -m pip install --require-virtualenv "openai==${OPENAI_PYTHON_VERSION}"
.g8-python/bin/python -m pip show openai
npm install --no-save --ignore-scripts "openai@${OPENAI_NODE_VERSION}"
npm ls openai
```

The Python driver must perform this sequence with the installed official
package:

```python
import os
from openai import OpenAI

client = OpenAI(
    api_key=os.environ["CMA_BEARER_TOKEN"],
    base_url=os.environ["CMA_BASE_URL"].rstrip("/") + "/api/v1",
)

def object_type(value):
    if isinstance(value, dict):
        return value.get("type")
    return getattr(value, "type", None)

conversation = client.conversations.create()
stream = client.responses.create(
    conversation=conversation.id,
    input="Read lean-toolchain with a workspace tool and report its first line.",
    stream=True,
)

event_types = []
response_id = None
for event in stream:
    event_types.append(object_type(event))
    response = (
        event.get("response")
        if isinstance(event, dict)
        else getattr(event, "response", None)
    )
    if isinstance(response, dict) and response.get("id"):
        response_id = response["id"]
    elif response is not None and getattr(response, "id", None):
        response_id = response.id

assert response_id is not None
assert "response.created" in event_types
assert "response.completed" in event_types
assert any(value.startswith("response.reasoning_") for value in event_types)
assert "response.cma_chat.tool_activity" in event_types

retrieved_conversation = client.conversations.retrieve(conversation.id)
retrieved_response = client.responses.retrieve(response_id)
items = client.conversations.items.list(conversation.id, limit=100)
item_types = [object_type(item) for item in items.data]

assert retrieved_conversation.id == conversation.id
assert retrieved_response.id == response_id
assert "message" in item_types
assert "cma_chat.tool_activity" in item_types
print({"events": event_types, "items": item_types})
```

The Node driver must independently create a new conversation and perform the
same sequence:

```javascript
import { OpenAI } from "openai";

const client = new OpenAI({
  apiKey: process.env.CMA_BEARER_TOKEN,
  baseURL: `${process.env.CMA_BASE_URL.replace(/\/$/, "")}/api/v1`,
});

const conversation = await client.conversations.create({});
const stream = await client.responses.create({
  conversation: conversation.id,
  input: "Read lean-toolchain with a workspace tool and report its first line.",
  stream: true,
});

const eventTypes = [];
let responseId = null;
for await (const event of stream) {
  eventTypes.push(event.type);
  if (event.response?.id) responseId = event.response.id;
}

if (!responseId) throw new Error("stream did not expose a response id");
if (!eventTypes.includes("response.created")) throw new Error("created missing");
if (!eventTypes.includes("response.completed")) throw new Error("completed missing");
if (!eventTypes.some((value) => value.startsWith("response.reasoning_"))) {
  throw new Error("reasoning events missing");
}
if (!eventTypes.includes("response.cma_chat.tool_activity")) {
  throw new Error("extension event missing");
}

const retrievedConversation = await client.conversations.retrieve(conversation.id);
const retrievedResponse = await client.responses.retrieve(responseId);
const items = await client.conversations.items.list(conversation.id, { limit: 100 });
const itemTypes = items.data.map((item) => item.type);

if (retrievedConversation.id !== conversation.id) throw new Error("conversation mismatch");
if (retrievedResponse.id !== responseId) throw new Error("response mismatch");
if (!itemTypes.includes("message")) throw new Error("message item missing");
if (!itemTypes.includes("cma_chat.tool_activity")) {
  throw new Error("extension item missing");
}
console.log({ events: eventTypes, items: itemTypes });
```

Iteration through the namespaced event and access to the later standard events
proves tolerance for that unknown event type in the recorded SDK version.
Listing the namespaced item and accessing the remaining page proves tolerance
for that unknown item type. A cast, hand-written JSON client, generated type
patch, ignored parser exception, or direct `curl` request does not satisfy G8.
Redact output text and tool bodies from the attached driver output.

### 4.6 G9 Lean setup-command round trip

The exact fixture recipe is:

```sh
[ -x .toolchain/bin/lake ] || (curl -sSf https://elan.lean-lang.org/elan-init.sh | ELAN_HOME=$PWD/.toolchain sh -s -- -y --default-toolchain "$(cat lean-toolchain)")
```

1. Configure that one command on a fresh staging host. Record the complete
   settings revision while redacting no command text from the private operator
   record; do not copy it to ordinary activity or public telemetry.
2. Create the first Chat thread. Confirm provider provisioning clones the
   repository before setup and setup finishes before the first conversation
   process opens. Record elapsed time, exit code, and the installed `lake`
   version without attaching installer output.
3. Create a small fixture through the conversation and retain its git status
   and SHA-256. Let the host pause, then revive it by one message.
4. Confirm the restored workspace already contains `.toolchain/bin/lake`, the
   same recipe runs before conversation load, completes as a fast no-op, and
   the fixture and toolchain SHA-256 values survive. Record the provision and
   revive setup durations so the no-op claim is numeric and bounded.
5. For the failure branch, use a fresh fixture owner or explicitly retire the
   old test host because settings are pinned per host. Configure the recipe
   followed by a deterministic command that emits a non-secret sentinel and
   exits `17`.
6. Create a thread and confirm provisioning fails with phase
   `workspace_setup`, zero-based `command_index: 1`, the retained sentinel in a
   bounded excerpt, `retryable: true`, and the failed runtime cleaned or marked
   with explicit cleanup debt. Confirm no conversation process opened and no
   third command ran.
7. Repeat the failure during revive and restart the control plane once. Confirm
   the queued admission reports the typed provisioning failure, the same index
   and bounded excerpt survive reconciliation, and no prompt is delivered.
8. Disable Chat, remove the failing declaration, and perform the shared
   cleanup.

G9 fails if the toolchain installs outside the checkpointed workspace, setup
runs after a conversation opens, the revive recipe downloads again despite a
valid retained executable, failure loses its index, output exceeds 32 KiB or
breaks UTF-8, a later command runs, or an untracked runtime survives.

## 5. Dark-first deployment and enablement

### 5.1 Release prerequisites

Before changing settings, require all of the following:

- The candidate commit is reviewed and all repository CI checks are green.
- The deployed CMA binary and runtime image match the recorded commit and
  immutable digest.
- Database indexes and checkpoint storage initialization completed without a
  warning or partial fallback.
- Runtime readiness advertises all three required versioned capabilities.
- The platform proxy and session-owned delegated credential mint are available
  for the pinned Chat model and title generation.
- G1 through G9 are `LIVE_PASS` for the exact image, provider, and selected SDK
  versions. Every record has a bounded claim and redacted attachments.
- Provider inventory contains no fixture runtime left by a gate.
- The operator can read settings and chat-host inventory with the documented
  least-privilege capabilities.
- Alerts in section 7 are installed and routed before admission opens.
- A rollback operator and observation window are named.

### 5.2 Stage the complete dark declaration

Read `GET /api/v1/admin/settings` immediately before every write. Use its
current revision as `expected_revision`; the numbers below are illustrative.
A `409 settings_conflict` requires another read and an explicit operator
decision. Do not automate last-write-wins.

First stage the documented complete defaults while Chat remains dark:

```json
{
  "expected_revision": 13,
  "chat": {
    "enabled": false,
    "workspace": {
      "repository_url": "https://github.com/the-omega-institute/trureturing",
      "branch": "dev"
    },
    "agent": {
      "model": null,
      "reasoning_effort": "high",
      "instructions": "",
      "record_tool_bodies": true
    },
    "message_prefixes": ["$codex-formal-answer"],
    "inactivity": { "pause_after_seconds": 900 },
    "setup": {
      "commands": [
        "set -eu; test -f skills/codex-formal-answer/SKILL.md; mkdir -p \"$HOME/.agents/skills/codex-formal-answer\" \"$HOME/.codex/skills/codex-formal-answer\"; cp -R skills/codex-formal-answer/. \"$HOME/.agents/skills/codex-formal-answer/\"; cp -R skills/codex-formal-answer/. \"$HOME/.codex/skills/codex-formal-answer/\""
      ]
    },
    "host": { "size": null, "max_threads": 10 }
  }
}
```

Verify the write response contains the next revision and
`settings.chat.enabled: false`. Verify AG-UI capabilities report
`cma:chat.enabled: false`, New Chat is absent, direct conversation creation is
refused, no ChatHost is created, and ordinary Agent sessions remain unchanged.

After G9 has passed on the release candidate, replace the complete Chat object
while it is still dark and set `setup.commands` to the exact Lean recipe:

```json
{
  "expected_revision": 14,
  "chat": {
    "enabled": false,
    "workspace": {
      "repository_url": "https://github.com/the-omega-institute/trureturing",
      "branch": "dev"
    },
    "agent": {
      "model": null,
      "reasoning_effort": "high",
      "instructions": "",
      "record_tool_bodies": true
    },
    "message_prefixes": ["$codex-formal-answer"],
    "inactivity": { "pause_after_seconds": 900 },
    "setup": {
      "commands": [
        "set -eu; test -f skills/codex-formal-answer/SKILL.md; mkdir -p \"$HOME/.agents/skills/codex-formal-answer\" \"$HOME/.codex/skills/codex-formal-answer\"; cp -R skills/codex-formal-answer/. \"$HOME/.agents/skills/codex-formal-answer/\"; cp -R skills/codex-formal-answer/. \"$HOME/.codex/skills/codex-formal-answer/\"",
        "[ -x .toolchain/bin/lake ] || (curl -sSf https://elan.lean-lang.org/elan-init.sh | ELAN_HOME=$PWD/.toolchain sh -s -- -y --default-toolchain \"$(cat lean-toolchain)\")"
      ]
    },
    "host": { "size": null, "max_threads": 10 }
  }
}
```

The Chat object is replacement-shaped. Include every field on each patch; an
omitted nested field resolves to its shipped default rather than preserving a
draft assumption.

### 5.3 Enable flip

Open admission with one revision-CAS patch whose Chat object is byte-for-byte
the reviewed dark declaration except for `enabled: true`:

```json
{
  "expected_revision": 15,
  "chat": {
    "enabled": true,
    "workspace": {
      "repository_url": "https://github.com/the-omega-institute/trureturing",
      "branch": "dev"
    },
    "agent": {
      "model": null,
      "reasoning_effort": "high",
      "instructions": "",
      "record_tool_bodies": true
    },
    "message_prefixes": ["$codex-formal-answer"],
    "inactivity": { "pause_after_seconds": 900 },
    "setup": {
      "commands": [
        "set -eu; test -f skills/codex-formal-answer/SKILL.md; mkdir -p \"$HOME/.agents/skills/codex-formal-answer\" \"$HOME/.codex/skills/codex-formal-answer\"; cp -R skills/codex-formal-answer/. \"$HOME/.agents/skills/codex-formal-answer/\"; cp -R skills/codex-formal-answer/. \"$HOME/.codex/skills/codex-formal-answer/\"",
        "[ -x .toolchain/bin/lake ] || (curl -sSf https://elan.lean-lang.org/elan-init.sh | ELAN_HOME=$PWD/.toolchain sh -s -- -y --default-toolchain \"$(cat lean-toolchain)\")"
      ]
    },
    "host": { "size": null, "max_threads": 10 }
  }
}
```

Record the settings response, activity identity, UTC time, and resulting
revision. The activity record must not contain the settings body. Verify AG-UI
reports `cma:chat.enabled: true` before beginning smoke checks.

### 5.4 Immediate smoke checks

Use a fresh rollout owner and retain opaque identifiers only.

1. Open the web application and confirm the `Chat` and `Agent` segments are
   available. Confirm Agent mode and existing Agent sessions are unchanged.
2. Select `Chat`, create one thread, and confirm one host progresses from
   `provisioning` to `active` with `thread_count: 1` and no `last_failure`.
3. Send one short message. Confirm the durable owner message is verbatim and
   does not contain `$codex-formal-answer`; confirm the runtime received the
   prefixed form through authorized test evidence only.
4. Confirm the conversation streams through the Responses projection and
   reaches one durable terminal response. Retrieve it and list conversation
   items using the documented linear predecessor.
5. Wait through the title sweeper. Confirm a bounded server-owned title appears
   in the rail, `SessionView.chat`, and conversation metadata. Confirm no
   request, error, or log includes transcript content.
6. Create a second thread as the same owner. Confirm the same host reports
   `thread_count: 2`, the worktree name and path differ, and history does not
   cross between threads.
7. Confirm a different owner receives a different deterministic host and no
   identifier or history from the first owner.
8. Read the structured events in section 7 and provider inventory. Require zero
   alert candidates, zero duplicate runtimes, and no credential or content
   leakage.

The complete 15-minute pause/revive journey need not be repeated in the
immediate rollout window only if G5 is already a valid live pass on this exact
deployment artifact. Schedule one post-enable observation and retain its
bounded evidence.

## 6. Disable and rollback

### 6.1 Admission disable

The first rollback action is a revision-CAS replacement of the complete Chat
object with only `enabled` changed to `false`. Preserve the reviewed workspace,
agent, prefix, inactivity, setup, and host values so the declaration can be
inspected and deliberately re-enabled later.

After the write:

1. Confirm AG-UI reports `cma:chat.enabled: false` and New Chat disappears.
2. Confirm new Responses conversation creation and new ChatHost creation are
   refused.
3. Confirm no existing host was terminated, recomputed under the new settings,
   or stripped of its workspace or conversation checkpoints.
4. Preserve existing thread histories and status projections. Continue normal
   reconciliation, checkpoint, pause, revive, credential, and cleanup duties
   required to uphold continuity for retained hosts.
5. Record inventory counts by status and watch them until the incident is
   understood. Disabling admission is not permission to delete them.

Explicit `"chat": null` also turns Chat off, but removes the declaration. Use
it only when the operator intends to remove configuration and has retained the
reviewed settings separately. It does not authorize host destruction.

### 6.2 Host retirement

Use `POST /api/v1/admin/chat/hosts/{id}/retire` only for a named host whose
owner impact is understood. Retirement is valid from `active` or `paused`,
fences new input, verifies capture before an active runtime is deleted, and
terminates retained threads with `stop_reason: revoked`. It is idempotent for a
retired host. A `409 invalid_state` requires reconciliation; do not race the
transition or delete its provider runtime manually.

Retirement preserves checkpoint objects and event history. The owner's next
Chat after re-enable creates a higher-generation host under then-current
settings. Record the operator, host id, reason, response, and provider
inventory without recording message or checkpoint content.

### 6.3 Binary rollback

Keep admission disabled before replacing a binary. Confirm the target version
can read every retained ChatHost, Chat origin, event, settings, and checkpoint
envelope written by the candidate. If compatibility is not proven, do not roll
the control plane backward. Continue forward recovery with the current binary
or prepare an explicit reviewed compatibility release. A binary rollback must
never be used to erase an unknown state or bypass a transition fence.

## 7. Alerts and diagnostics

The binary emits structured sweep summaries. Alert on the exact event names
and fields below; do not parse human prose when a field exists.

| Signal | Fields | Recommended action |
|---|---|---|
| `cma_chat_inactivity_alert_candidate` | `refused`, `errors`, `page_budget_exhausted` | Page on any error or repeated refusal; investigate immediately if the page budget is exhausted. |
| `cma_chat_inactivity_pass` | `pages`, `candidates`, `paused`, `skipped`, `refused`, `errors`, `page_budget_exhausted`, `elapsed_milliseconds`, `actionable` | Build rate and latency panels; a refusal must correlate with host inventory. |
| `cma_chat_rotation_alert_candidate` | `errors`, `page_budget_exhausted` | Disable new admission if image-drift recovery cannot converge. |
| `cma_chat_rotation_pass` | `candidates`, `rotated`, `skipped`, `errors`, `page_budget_exhausted`, `elapsed_milliseconds` | Confirm each rotation leaves one authoritative generation. |
| `cma_chat_title_alert_candidate` | `errors`, `page_budget_exhausted` | Titles can fall back safely, but persistent worker or pagination errors require investigation. |
| `cma_chat_title_pass` | `candidates`, `generated`, `fallbacks`, `skipped`, `errors`, `page_budget_exhausted`, `elapsed_milliseconds` | Monitor fallback ratio without recording generated or source text. |
| `chat checkpoint pass completed with refusals` | `candidates`, `completed`, `skipped`, `refused` | Treat repeated refusal on an otherwise quiet active host as recovery-point debt. |
| `chat checkpoint pass could not read its durable candidates` | bounded `error` | Investigate store availability; do not claim current recovery points. |

The admin host inventory is the diagnostic projection. It exposes host id,
opaque owner, status, thread count, runtime version, pause/revive/checkpoint
times, settings revision, and bounded `last_failure`. It intentionally omits
runtime authority, transition claims, configured command text, prompt content,
checkpoint identities and digests, credentials, and the pinned settings body.
Do not bypass that projection in a general support response.

Recommended derived alerts:

- A host remains `pausing`, `reviving`, or `provisioning` longer than the
  ten-minute public provisioning bound.
- An otherwise eligible quiet active host has no successful checkpoint for
  more than the five-minute eligibility interval plus two one-minute sweep
  intervals.
- Any `last_failure` with phase `workspace_setup`, `workspace_source`,
  `credential`, `runtime`, `runtime_lost`, or `cleanup` remains present after
  its expected reconciliation interval.
- A response returns `provisioning_timeout`, or repeated paused admissions do
  not settle to one durable response.
- Provider inventory shows zero or more than one runtime for an active or
  reviving host, or any runtime for a paused host.
- The inactivity, rotation, checkpoint, or title cadence stops emitting for
  more than two expected intervals while the process is otherwise healthy.

### Diagnostic order

1. Disable new Chat admission if failures are repeated, cross-owner, or can
   affect continuity.
2. Record the UTC interval, candidate commit and image, settings revision, host
   id, public inventory projection, and bounded structured events.
3. Determine the durable host status before consulting provider inventory.
4. Compare provider candidates by exact CMA host metadata and runtime
   generation. Do not choose by creation time or delete both.
5. Confirm the last verified workspace and per-conversation checkpoint exists
   and passes stored byte verification through the authorized store path.
6. Let normal boot reconciliation settle a claimed transition. Do not create a
   replacement runtime or resend a queued prompt while the result is
   ambiguous.
7. If `last_failure.phase` is `workspace_setup`, use only its bounded index,
   retryable flag, cleanup state, and redacted excerpt. Do not rerun the command
   in a user worktree merely to obtain more output.
8. Escalate with the evidence template. Keep Chat dark until one authoritative
   host state and provider generation are proven.

## 8. Operator checklists

### Before staging evidence

- [ ] Candidate commit and immutable image digest recorded.
- [ ] Disk, database, checkpoint storage, provider, and proxy capacity checked.
- [ ] G1-G4 live records apply to the same image and provider.
- [ ] Required runtime capabilities observed from the running image.
- [ ] Isolated fixture owners and provider run-id tag allocated.
- [ ] Chat is dark outside the bounded drill interval.
- [ ] Structured log capture and provider inventory capture opened.
- [ ] Cleanup authority is limited to the drill's exact resource tags.

### Before enable

- [ ] Full repository CI is green at the candidate commit.
- [ ] G1-G9 each have a reviewed `LIVE_PASS` record and bounded claim.
- [ ] Official Python and Node package versions and resolver output attached.
- [ ] Staging enable, smoke, disable, pause, revive, and restart drills passed.
- [ ] Dark complete settings object matches the reviewed declaration.
- [ ] AG-UI reports Chat disabled before the flip.
- [ ] Alert rules, dashboards, rollback operator, and observation window ready.
- [ ] Provider inventory contains no drill fixture or duplicate runtime.

### After enable

- [ ] Settings CAS response and new revision retained.
- [ ] AG-UI reports Chat enabled.
- [ ] Two same-owner threads share one host and distinct worktrees.
- [ ] A different owner receives a different host.
- [ ] Responses stream, retrieval, items, and linear predecessor work.
- [ ] Prefix remains invisible in durable owner content.
- [ ] Server title appears or bounded fallback settles without content logs.
- [ ] No sweep alert candidate, duplicate runtime, or privacy leak observed.
- [ ] Post-enable pause/revive observation scheduled or completed.

### After disable or incident recovery

- [ ] AG-UI reports Chat disabled and new creation is refused.
- [ ] Existing hosts and histories remain durable.
- [ ] Inventory by lifecycle status retained.
- [ ] No provider runtime was manually deleted outside a fenced CMA operation.
- [ ] Ambiguous transition reconciled to one durable authority.
- [ ] Disposable resources removed by exact run-id tag.
- [ ] Provider listing is clean and cleanup evidence attached.
- [ ] Re-enable requires a new explicit operator decision and current revision.

## 9. Evidence attachment template

Attach one record per gate to its GitHub issue and cross-link it from
[issue #741](https://github.com/ChronoAIProject/cma/issues/741). Keep secrets and
content out of issue text.

```markdown
## G5 release evidence: <release identifier>

- Status: PENDING
- Bounded claim: <one precise sentence or empty while pending>
- UTC interval: <start> through <finish>
- Operator / reviewer: <ids>
- CMA commit: <40-character SHA>
- CMA release: <identifier>
- Runtime image: <reference>@sha256:<digest>
- OS / kernel: <values>
- Kernel: @openai/codex@0.147.0; codex-acp@1.4.0
- Provider / API / chart: <values>
- Settings revision: <number>
- Official SDKs: Python <version or N/A>; Node <version or N/A>
- Fixture identities: <opaque owner, host, conversation, response ids>
- Commands: <attachment link and SHA-256>
- Redacted transcript: <attachment link and SHA-256>
- Structured logs: <attachment link and SHA-256>
- Provider inventory before/after: <links and SHA-256 values>
- Timings and byte counts: <attachment link and SHA-256>
- Cleanup: <exact tagged resources removed; final listing link>
- Contradictions or limitations: <none, or explicit list>
```

A reviewer changes `PENDING` to `LIVE_PASS` only after every required field and
attachment is present, the bounded claim matches what was observed, privacy
review passes, and final provider inventory proves cleanup. Repository CI links
remain separate `CODE_PASS` evidence and must not be relabeled as a live gate.
