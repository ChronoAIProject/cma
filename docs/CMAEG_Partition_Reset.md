# Owner repair of a deleted partition Agent

Issue #1323 adds explicit owner reset. It does not add existing-Agent rebind,
automatic replacement or another partition-rule version. Gateway transport IDs,
channel IDs, URLs, keys and healthy partition mappings are preserved.

## Owner procedure

1. Read the channel and its version. Inspect
   `GET /v1/channels/{channel_id}/partition?partition_key=oracle` (optionally
   `consumer_id`). Keep its exact `agent_id`.
2. With creator authorization and `cma-eg:channels:update`, submit
   `POST /v1/channels/{channel_id}/partition/resets`, one `Idempotency-Key`
   containing 1–128 visible ASCII bytes, and:

   ```json
   {
     "partition_key": "oracle",
     "expected_version": 5,
     "expected_agent_id": "agt_deleted"
   }
   ```

   `consumer_id` is optional and defaults to `default`. Organization owners use
   `X-CMAEG-Owner` with current organization authorization. Ingress HMAC secrets
   and worker keys do not authorize management. Other owners receive 404.
3. A 200 result includes `consumer_id`, unchanged `transport_conversation_id`,
   and `reset`: binding ID/generation, transport `conversation_id`, new
   `incarnation`, `previous_agent_id`, `previous_conversation_id`, reserved
   `successor_agent_id`, and `state: reserved`. The Agent does not yet exist.
   The next ensure/event provisions that exact reservation. Inspection reports
   the reserved mapping with null Agent status until provisioning begins.
4. After a timeout, retry the same body and key. CMA returns the original
   receipt, including after the writer gate closes. Changed input with that key
   returns 409 `partition_reset_key_conflict`. A channel-version change is a
   separate gateway conflict; do not change the body/key to resolve an unknown
   result without inspecting the mapping first. Reset itself does not change
   the channel version. `expected_version` is a read-time precondition, not a
   cross-service transaction or a channel write/CAS.

The gateway projects the provider's allowlisted reset refusal as a 409 problem
with `code: conflict` and the exact named refusal in `detail`. Eligibility
refusals include `partition_reset_existing_agent`,
`partition_reset_affinity_unsupported`, `partition_reset_binding_not_ready`,
`partition_reset_mapping_missing`, `partition_reset_mapping_retired`,
`partition_reset_mapping_changed`, `partition_reset_agent_missing`,
`partition_reset_agent_not_deleted`, `partition_reset_cleanup_pending`, and
`partition_reset_parents_unavailable`. Missing storage/authorization evidence
never proves eligibility. A reset requires a profile-owned conversation-affinity
binding, a terminally Deleted Agent with settled cleanup, a Ready idle binding,
and usable idle Sandbox/Workspace parents. Stopped/failed/live Agents and
borrowed Agents cannot be reset. No resource is retired or deleted by reset.

## Admission, retries and retained work

The cutoff is **CMA command admission**. An admitted predecessor command keeps
its original Agent, response, event context and history. Its permanent external
event identity is `(binding authority, transport conversation, event id)`, which
reset preserves. A POST attempting another command on the successor receives
409 `agent_command_key_conflict`; it does not execute again. Legacy Custom
retries can instead recover the original provider context and return 202 with
no new POST or duplicate result delivery. Durable gateway retries keep their
original admission/execution and delivery receipts. A refusal never authorizes
replaying a completed or uncertain external effect.

Gateway-queued events not yet admitted by CMA select the successor when they
reach CMA after reset. The same event can admit at most one command there;
ordinary queue expiry, authorization and capacity rules still apply. Already
terminally refused/expired gateway events are not automatically reopened.
There is no gateway pending-work preflight and no promise that the gateway
partition was idle during reset.

Reset and command admission write the same CMA binding transaction fence. Reset
also advances the binding revision. A racing request either committed under its
original authority before reset or fails the stale-authority fence and retries;
it cannot commit under an obsolete snapshot. Terminal deletion already refuses
fresh input on the predecessor. Other partitions retain their Agents; an
in-flight snapshot of the same binding may receive the existing retryable
admission conflict and retry with its original key.

Historical mappings, command records, logs, reply targets, media references and
reference expiry are preserved. History/context and media supplements resolve
the original admitted command's Agent. New events bind to the current
incarnation before their media/supplement parent command is admitted. Old media
never moves to a successor. Reset does not recreate deleted home files or merge
old and new Agent transcripts. Background recovery never provisions a head
that has not been materialized by ensure/event admission.

## Binding contract and storage

The management-authenticated provider endpoint is
`POST /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/resets`.
It accepts `expected_agent_id`, `owner`, `organization`, `request_digest`, and
`idempotency_binding_id`, with one 64-character lowercase hex `Idempotency-Key`.
The gateway hashes the tuple `(owner kind, owner subject, channel id, client key)`
and separately hashes the complete typed public body. The channel's default
binding is the stable idempotency scope for all consumers, so changing consumer
with the same key also conflicts. CMA verifies current manager and owner kind/
subject on both target and scope bindings before receipt replay or mutation.
No credential or manager identity from request JSON grants authority.

CMA stores `conversation_resets_v1` outside the strict `ProviderBinding` payload
in its existing `provider_bindings` envelope. It contains heads keyed by the
hash of `(generation, transport conversation id)` and immutable reset receipts.
Absent means incarnation zero. Successor internal conversation IDs and Agent/
Worktree IDs are reserved once, without inserting a provisionable conversation.
Ensure later inserts the reservation using existing CAS and provisioning.
Existing `BindingConversation` records are unchanged. Point reads use the
existing binding `_id` index; there is no new collection, index, TTL or startup
DDL. Ordinary `$set` binding updates preserve the extra envelope field. Reset
also increments the binding's existing admission fence and revision, and writes
`binding_reset_fence` outside the strict Agent, Sandbox and Workspace payloads
in the same transaction. Those counters force write conflicts with concurrent
lifecycle changes; their values are not execution authority, and subsequent
ordinary whole-envelope replacements may omit them. Strict domain payloads
remain unchanged; the retained incarnation head imposes the reader floor.
Storage is bounded to 256 heads and 512 receipts per binding; exhaustion returns
`partition_reset_capacity` without discarding history. Existing heads can still
advance within the receipt bound. Receipts are not collected to make old keys
eligible again.

## Migration 2026-11-16-am-binding-conversation-reset-v1

| Required field | Release record |
| --- | --- |
| Identity, phase, owners | #1323; `2026-11-16-am-binding-conversation-reset-v1`; expand/readers then gated writers; Agent Manager owns storage, Event Gateway owns the proxy; deployment operator remains responsible for activation evidence |
| Source/artifact identity | Implementation commit series on `fix/1323-partition-reset`; pin final reviewed source SHA and candidate/recovery image digests in activation evidence before enabling |
| Writers | Explicit CMA reset transaction; lazy conversation materialization through ensure/Responses; existing provisioning/recovery workers consume only materialized records |
| Formats | Optional `conversation_resets_v1` binding-envelope state and `binding_reset_fence` envelope counters on Agent/Sandbox/Workspace rows; original strict payloads, conversation records, channel seals, event IDs, logs, credentials and expiry preserved; no new DDL or backfill |
| Reader floor | Every serving/recovery CMA image must understand heads, use them for new alias resolution and retain original Agent mapping for history. Predecessors can decode the unchanged strict payload but ignore the head: they are semantically incompatible after the first reset. Drain them before activation. Gateway proxy requires this additive contract; no new gateway durable format |
| Writer gate | `CHRONO_BINDING_CONVERSATION_RESET_ENABLED=false` by default. Closed: fresh reset returns `partition_reset_disabled`; exact recorded reset replay, head reads, lazy materialization and admitted work settlement remain available |
| Backup/rehearsal | Operator must record protected Agent Manager catalog/command/history backup and credential/home custody coverage, test restored heads/receipts plus lazy materialization, and preserve all post-backup accepted writes |
| Rollback/forward recovery | Before first reset, predecessor remains eligible subject to other migrations. Afterwards use a compatible forward-repair image; closing the flag does not lower the reader floor. Never unset heads, erase receipts, clear commands or replace retained homes |
| Verification | Local memory/Mongo races, receipt replay, gate closure, legacy absence, unchanged strict payload, preservation under ordinary writes, and provider/SDK contracts are automated. Real provider qualification remains operator-owned and unexecuted by this change |

Activation evidence must identify source SHA, candidate image digest, environment,
time window, N, configuration/gates and raw-result location. Include the provider
matrix case **owner rebind — reset to fresh Agent**, with repair within five
minutes, healthy partition continuity and zero duplicate execution/delivery.
This case does not claim support for selecting an existing Agent. Qualification
must include restart/lost-response retry and the predecessor-reader exclusion.

## What can delete a gateway-created Agent

Owner Agent deletion after stopping/failure, owner Workspace removal, and owned
binding/conversation retirement cleanup can delete it. Channel deletion unbinds
its provider and initiates owned-resource cleanup. Recovery can finish retained
previous-binding retirement; ordinary metadata-only binding updates do not do
that. Borrowed existing Agents survive binding retirement. A retired Sandbox's
retention purge can subsequently remove Agent records/logs, producing a missing
Agent rather than a retained Deleted tombstone. Inactivity and platform-spend
maintenance pause Sandboxes; they are not Agent deletion sweeps. No direct
account-closure Agent-deletion hook was found at the reviewed baseline.

Use Agent lifecycle logs (`Deleted`/`AgentDeleted`), stop cause, Workspace/Sandbox
lifecycle and binding retirement history to establish a specific cause. Code
paths alone do not establish who deleted the Agent reported in #1323.
