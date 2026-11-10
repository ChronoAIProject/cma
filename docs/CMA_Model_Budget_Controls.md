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
`ConfigurePlatform` authority. No new environment variable, price, allowance or
currency default is introduced. The owner supplies all values privately.

The section contains `owners` (explicit owner policies) and an optional
`default_policy`. An owner entry overrides the default. An absent section, or
an empty owner list without a default, disables new admission reservations.
Outstanding reservations and recorded stop decisions remain effective.

Each policy contains `owner_day_limit`, `turn_limit`, `input_token_units`,
`output_token_units`, `max_unobserved_units` and `usage_timeout_seconds`. Units
must use the same exact private valuation. Integer inputs must be representable
exactly in browser clients. Policy validation rejects zero/overflowing inputs,
an oversized turn allowance, duplicate owners and unusable timeout/headroom.
The daily window is UTC. Audit events contain the settings operation and
revision, not policy values or owner scope.

Legacy full PUT requests that omit this section preserve the current policy.
An explicit empty policy disables new reservations. PATCH replaces the complete
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
alone cannot release a possibly executed reservation. UTC rollover clears
settled daily usage while retaining every unresolved reservation.

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

The default deployment leaves the section absent. Before activation, all serving
and rollback readers must understand the settings section and reservation
schema. Funding and actual configured values stay outside GitHub. Production
verification belongs to the release operator using a test owner; never change a
platform-wide policy to exercise a threshold.

Evidence should include source revision, release artifact, policy revision,
scenario result and retained receipt/event evidence, with no policy amounts,
owner identities or credentials. Verify successful admission, owner exhaustion,
concurrent admission, same-key replay, usage threshold, stale usage, real terminal
settlement, explicit continuation and an unaffected second owner. Strict
provider-spend proof remains a separate acceptance requirement.
