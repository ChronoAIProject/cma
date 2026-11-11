# Capacity recovery service and integration boundary

Part of #1351 and #1349. Recovery admission remains default-off and native
confirmation remains typed Unsupported. This source checkpoint is not release
qualification or permission to activate writer/recovery gates.

## Request-scoped composition

The Layer2 `CapacityRecoveryCommands` port provides status, admission and one
accepted-work attempt. Construct it with
`Arc<AgentWorker>::capacity_recovery_commands(Arc<RecoveryService>)`; the actual
store and worker perform fresh personal resource authorization for every operation.
The command router is `agent_capacity_recovery_commands_router`. Shared composition
owns its public re-export, authenticated mounting and default-off process config.

| Route under `/api/v1/agents/{id}/capacity-recovery` | Behavior |
| --- | --- |
| `GET` | Current bounded disposition; no mutation. |
| `POST /status` | Status for the unchanged original request. |
| `POST /admit` | New storage admission, requiring process and durable gates. |
| `POST /resume` | One exact accepted-work attempt, independent of new admission. |

POST bodies are the original `RecoveryRequest`: Agent, creation key/fingerprint,
original failure sequence and requested-at timestamp. Path and body Agent must
match. The timestamp is exactly UTC whole-second `YYYY-MM-DDTHH:MM:SSZ`, with a
valid calendar date. Bodies are limited to4096 bytes and reject unknown fields;
there is no input, settings or context replay field. Responses contain only
bounded disposition/phase, operation identity, attempts and retry/completion time.
They never serialize retained native/runtime/home identifiers or input digests.

All responses, including extractor errors, are private/no-store. Binding and
request-bound-read credentials cannot use the personal command boundary.
Independent credential expiry bounds the entire awaited command. An ambiguous
reply must be reconciled using the unchanged request: HTTP202 `unknown` is not
success or permission to resend input. Known abort is HTTP409; unavailable
activation is HTTP503. New admission being disabled does not cancel accepted work.

## Completed activation and restart

Durable completion precedes runtime activation. A completed retry verifies its
exact result witness and publication, current Agent revision/fence/input/native
identity, current personal policy and parent intent. It then reattaches only the
retained process, rechecks authority and activates the attachment without another
completion publication. A Retiring parent or changed fence refuses.

P1-3 is still open pending exact live-registry AlreadyActivated handling and
restricted background/loss dispatch. The active discovery streams exclude
Completed. `RecoveryService::discover_agent` uses the frozen by-Agent index to
find up to32 original requests for one disconnected Agent, including completed
work. Its limit/batch33 detects overflow; queries use simple collation,
maxTimeMS1000 and a128KiB candidate byte cap. No unbounded completed scan or new
index is introduced. Candidates, empty results and absent carriers grant no
authority, prove no KnownAbort and permit no ordinary reconnect fallback.

Background continuation needs an explicit accepted-work authority port. It must
bind current personal ownership/policy and the original published result without
constructing a Caller from an owner ID or retaining an expired request Caller.
Publication incarnation alone does not bind owner. Exact result-revision/digest
checks and current parent/stop intent remain necessary. The shared registry
presence helper is process-local evidence only, not durable native proof.

## Bounded child consumers

Recovery child enumeration returns a complete set only within512 children and
8MiB BSON across every page; a transaction page itself is bounded to4MiB. It
refuses oversized Agent or Workspace sets before mutation and does not retain a
second complete document vector. Tests assert durable rows are unchanged.

## Qualification still required

Canonical format-2 validators and three nonunique indexes come from the registered
segment migration. Normal storage/restart fixtures use those definitions; only
explicit obsolete/corrupt test records bypass validation. Authenticated runtime
roles, canonical ledger restart/drift, real Mongo crash/unknown-outcome behavior,
exact native confirmation and production composition are separate gates.

Closing admission is rollback for new operations only. Retain compatible readers,
exact accepted settlement and restricted activation retry after closing gates.
Never downgrade a segmented history, reset its sequence, remove retained input,
revive a Retiring parent or use ordinary reconnect as a native-proof fallback.
