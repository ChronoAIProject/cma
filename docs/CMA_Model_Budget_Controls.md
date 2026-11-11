# Recorded-usage model budget controls

Related work: #1489 and the engineering mechanism of #1483. Platform-wide
metering, registration and Sandbox stop actions belong to #1496. Typed upstream
insufficient-credit recovery is described in [model credit recovery](CMA_Model_Credit_Recovery.md).

## Scope and enforcement boundary

The follow-up [CMA request reservation contract](CMA_Model_Request_Reservations.md)
adds pre-request holds for titles and contextual suggestions. Runtime model
calls still require the gateway contract described below.

The current adapter enforces admission reservations and requests a safe stop
from recorded model usage. It does **not** prove a strict ceiling on provider
spend. Provider usage arrives after model requests; the configured unobserved
allowance is headroom, not a verified bound on a request or tool loop. Runtime
cancellation cannot revoke a request already accepted by a provider.

Keep the policy disabled for general rollout until a request-level enforcement
contract supplies a trusted owner/turn binding and a conservative bound before
each paid commitment. That boundary must cover retries, compaction, parallel
requests and auxiliary control-plane model calls. These controls alone do not
complete #1489 or the platform cap gates. An isolated test owner can verify the
reservation and terminal-state mechanisms using controlled runtime fixtures.

`layer2-core/src/budget.rs` owns provider-independent policy, checked accounting
and the `SpendSource` stop interface. `cma/src/store/budget/` initially uses CMA's
persisted `UsageUpdated` observations. A future metering adapter can supply
`SpendSource` without changing the conversation recorder or cancellation caller.
The transactional admission/settlement adapter is separate from that read
interface; an authoritative source must integrate at that boundary too.

## Private administration

The existing authorized `/api/v1/admin/settings` GET, PUT and PATCH operations
accept a `model_budget` section. Use the existing settings revision CAS and
`ConfigurePlatform` authority. The owner supplies all policy values privately.
A separate deployment compatibility gate, `CHRONO_MODEL_BUDGET_WRITES_ENABLED`,
defaults to false; it is not a spend-control value or an admin-editable policy.

The section contains an explicit `enabled` switch, optional `platform` policy,
`owners` (explicit owner policies) and an optional `default_policy`. Only disabled
is an implicit activation value. An owner entry overrides the default. An absent
section or `enabled: false` disables new admission reservations. Disabled settings
can retain explicitly configured values for later activation **only after the
compatible-reader save gate is enabled**. Saving values with enforcement disabled
already changes the stored schema. Until the gate opens, keep the section absent
or completely empty and disabled.
Outstanding reservations and recorded stop decisions remain effective.

Each owner policy contains `owner_day_limit`, `turn_limit`, `max_calls_per_turn`,
`day_timezone`, `input_token_units`,
`output_token_units`, `max_unobserved_units` and `usage_timeout_seconds`. Units
must use the same exact private valuation. Integer inputs must be representable
exactly in browser clients. Policy validation rejects zero/overflowing inputs,
an oversized turn allowance, duplicate owners and unusable timeout/headroom.
The daily window follows the configured IANA timezone, including DST; no timezone
is supplied implicitly. Changing a nonempty ledger's timezone requires liability
reconciliation. Audit events contain the settings operation and
revision, not policy values or owner scope.

The platform policy contains `period_cap`, inclusive `period_start`, exclusive
`period_end`, `warning_percent`, `stop_percent`, `protected_reserve` and
`lag_allowance`. The interval uses explicit RFC3339 instants. Every value is
required when the platform policy is supplied. Percentages must be ordered,
and protected reserve plus lag allowance must fit below the stop threshold.
Enabling requires a complete platform policy and at least one applicable owner
policy. Platform metering/actions consume this contract through #1496; request
call-count enforcement belongs to the following broker/gateway integration.
Persisted configuration alone is not evidence those integrations are active.

Legacy full PUT requests that omit this section preserve the current policy.
An explicitly disabled policy disables new reservations. PATCH replaces the complete
section. The explicit settings-reset endpoint resets this section too. Existing
SDK readers retain the additive section on full replacement; direct REST is the
current typed PATCH surface for this section.

Changes to limits affect queued dispatch and active stop checks. A lowered
limit never releases unresolved liability. Valuation changes with outstanding
or already-debited usage fail closed; existing reservations keep their original
valuation. An operator must reconcile that liability before changing units.

## Durable admission and recovery

Admission reserves the whole turn allowance against the owner in the same
Mongo transaction as the durable command or turn. A shared owner-row write
serializes competing admissions. A settings-row write fences policy changes,
including activation. Same-key replay returns the existing receipt before
attempting another reservation. Control commands can still stop admitted work.
Runtime usage and interruption capabilities are required for enabled owners.

A queued command transfers its reservation to the bound turn. A terminal
command that provably never dispatched releases its reservation. Elapsed time
alone cannot release a possibly executed reservation. Local-day rollover clears
settled daily usage at the configured local midnight while retaining every unresolved reservation.

The initial owner baseline conservatively values all retained cumulative Agent
usage; it is not an authoritative reconstruction of historical daily spend.
Deleted history, unreported usage and external model calls require a stronger
source before claiming complete spend coverage. Terminal usage is not certified
as final by the present provider contract, so settlement debits the full reserved
allowance rather than assuming unobserved usage was free. Late larger usage
increases held liability before another admission.

A recorded threshold or stale usage requests a cancel guarded by the exact live
attachment and persisted turn stop. Only actual runtime terminal evidence marks
the turn failed. The stop request alone cannot mark an Agent ready or release
its reservation. Stop reasons survive restart and policy disablement, including
activation that finds a running turn without a reservation.

Budget failures use `owner_budget_exhausted`, `turn_budget_exhausted` or
`budget_unavailable`, with HTTP 402 guidance. Completed output and the original
failed receipt remain visible. Same-key replay does not repeat work. After
accounting or capacity is restored, an explicit new message starts a new turn.
A database outage prevents durable admission and terminal settlement; the
current cancellation monitor retries unavailable observations. Consequently it
cannot guarantee a provider-spend ceiling during an accounting outage.

## Storage and release handoff

Agent Manager owns `model_budget_reservations`, initialized by its existing
schema startup path and listed by its service-isolation authority command.
Grant it the same scoped runtime/migrator operations as the other Agent Manager
collections before rollout in isolated mode. There is no TTL on unresolved
liability or stop evidence. Do not delete the collection to reset a budget.
Queued budget failure precision is stored in an additive command-document
sidecar; the strict historical command record and command-status wire enum
retain `execution_failed`. HTTP refusals and the Response error preserve the
precise budget type and recovery guidance for existing SDK readers.

The rollout is expand/contract, with a reader stage before the first nonempty
save, including disabled drafts:

1. Ship this compatible reader with `CHRONO_MODEL_BUDGET_WRITES_ENABLED=false`.
   The strict boolean accepts only true/false and invalid values fail startup
   without echoing the value. Empty disabled settings are omitted from persisted
   `PlatformSettings`; ordinary settings saves remain readable by pre-budget
   images. Any nonempty budget save returns HTTP 503
   `model_budget_writes_unavailable`, without changing the settings revision.
2. The release operator verifies that all serving readers and the designated
   one-image rollback target understand this exact settings and reservation
   schema, drains older workers, and records the verified image/digest as the
   rollback baseline. Only then, in a separate fenced release/configuration step,
   may cma-web-hotfix set `CHRONO_MODEL_BUDGET_WRITES_ENABLED=true`. This PR keeps
   the flag false in the ConfigMap and does not enable production writes.
3. Admins may now save privately supplied values, including disabled drafts,
   through the existing revision CAS and confidential audit. Enabling budget
   enforcement is a later, separate decision requiring the request-level proof
   described above and the lead's private activation direction.
4. After saves exist, rollback uses the verified compatible reader. Turning the
   write gate off retains readable settings, reservations and stop evidence;
   it refuses further nonempty settings saves. Setting policy `enabled:false`
   while retaining values does **not** restore pre-budget reader compatibility.
   Never clear policy or delete liability/stop records to justify an older image.

GET `/api/v1/admin/settings` exposes the read-only top-level boolean
`model_budget_writes_available`. It also accompanies settings mutation responses.
The Operations UI must treat false or missing as **saving not yet available**,
keep values out of logs, and handle the typed 503 if a write reaches a closed
reader during rollout. This signal is not persisted policy and cannot be changed
through PUT/PATCH. Existing SDK readers ignore the additive response metadata.

Train target: merge the compatible reader by Wed 2026-11-11 14:00 SGT for the
evening train; establish the rollback baseline before a later Thursday write-gate
step. If the reader first rides Thu morning, retain Thu evening/Fri morning for
the separate step, with verification complete by Fri 2026-11-13 15:00 SGT.
Production changes and verification belong to cma-web-hotfix and the lead relay.
Use an isolated test owner; never change platform-wide values to exercise a
threshold. Funding and actual configured values stay outside GitHub.

Evidence should include source revision, release artifact, policy revision,
scenario result and retained receipt/event evidence, with no policy amounts,
owner identities or credentials. Verify successful admission, owner exhaustion,
concurrent admission, same-key replay, usage threshold, stale usage, real terminal
settlement, explicit continuation and an unaffected second owner. Strict
provider-spend proof remains a separate acceptance requirement.
