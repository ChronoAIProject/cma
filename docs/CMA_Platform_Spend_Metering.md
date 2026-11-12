# Platform spend metering and stop contract

Status: **implemented on `feat/platform-spend`, activation off** (#1496;
plan `docs/plans/platform-spend/IMPLEMENTATION.md`, implementation notes
`docs/plans/platform-spend/NOTES.md`). Decisions 19–30 record where the
implementation settled details this contract left open. Related
work: #1496 (this contract), #1489/#1621 (owner and turn reservations), #1631
(request holds), #1603 (typed credit failures), #1478 (global admission and the
60-second stop switch, which extends the closure model defined here), #1566
(runtime gateway finality), #1483/#1546 (M-SUST-01). It extends
[model budget controls](CMA_Model_Budget_Controls.md) and
[request reservations](CMA_Model_Request_Reservations.md) and uses their
vocabulary. No price, allowance, funding amount or production identity appears
here; every value is a private operator input.

## 1. What this contract decides

The owner/turn foundation already answers "may this owner spend more today".
This contract answers "may the platform commit more spend in this period", and
what happens when the answer becomes no. It fixes four things the foundation
left open and that the request-reservation slice asked for:

| Question | Answer in one line |
| --- | --- |
| Freshness | The model ledger is exact at the transaction boundary; cloud spend is an operator observation plus a bounded projection; a source older than its bound closes new commitments but interrupts nothing. |
| Outstanding liability | Every owner reservation and request hold is mirrored into one platform period row in the same transaction; it is released only by authoritative settlement or proven non-dispatch, never by time. |
| Revision fencing | The period row records the settings revision and period it was evaluated under; a closure can be lifted only by a strictly newer revision; period changes on a nonempty row fail closed. |
| Platform stop state | One durable row per period carries a set of typed closures. Liability-bearing admissions read it inside their transaction; zero-liability admissions read it without the settings fence; revival reads it at the service entry; running turns receive it through `SpendSource` behind a short cache; the pause sweep acts on it. #1478 adds its reasons to the same set. |

Everything below is provider-neutral. "Model" means the metered model usage the
foundation already values in private accounting units; "cloud" means the
infrastructure bill the platform cap also covers. Neither a provider's wire
vocabulary nor a cloud vendor's name belongs in Layer 2 types.

The per-owner daily allowance, turn limits and the 2-Sandbox visitor default are
#1621 and #1485; this contract does not change them.

## 2. Sources of spend

Two sources feed one period total in this delivery; a third is designed and
deferred. Each has its own freshness rule.

### 2.1 Model reservations (internal, transactional)

The existing `model_budget_reservations` ledger: owner rows, turn reservations,
request holds and their settlements. This source is **exact**: liability is
reserved before any paid commitment and settled from recorded usage inside a
Mongo transaction. Its freshness is the commit itself. Per-turn staleness of
runtime usage observations is already handled by `usage_timeout_seconds` and
produces a turn stop, not a platform event.

Owner rows reset `spent` at the owner's local midnight, so they cannot be summed
into a period figure. The period row therefore accrues its own `spent` and
`reserved` (section 4) in the same transactions that move the owner rows.

### 2.2 Cloud observations (operator input, laggy)

CMA has no cloud billing feed. The operator posts **observations**: the
platform's cloud spend from `period_start` through an instant
`observed_through`, in the same private units. Observations are append-only and
immutable; a later observation with a later `observed_through` supersedes an
earlier one for the admission figure; both remain as evidence.

Between the latest `observed_through` and now, cloud spend is **projected**
linearly from an operator-configured `run_rate_units_per_day`:

```
cloud_projected(now) = ceil((now − observed_through) × run_rate_units_per_day / 86_400)
cloud(now)           = observed_units + cloud_projected(now)
```

The run rate is the operator's own calculation of node, storage, egress,
backup, monitoring and gateway cost at the planned auto-pause duty cycle
(M-SUST-01). Database cost and the existing-consumer reserve are not in it;
they are outside the disposable cap (section 3). Per-Sandbox allocation
quantities from `sandbox_usage` are **not** added to this figure: they are the
same node cost expressed per pod, and the gate forbids counting a node and its
pods twice. They remain the owner-attribution source for TC-112.

Platform spend metering is enabled only when budgets are enabled and the
platform policy carries a `cloud` section (decision 31). Without one there is
no period row, no closure and no platform check: owner and turn budgets
behave exactly as #1621 defines them.

Freshness: the policy carries `max_observation_age_seconds`. If
`now − observed_through` exceeds it, or no observation exists for the period,
the cloud source is **stale**. A stale source closes new commitments (section
5, `MeteringUnavailable`) and does not interrupt running work. The first
observation of a period may be `0` units at `period_start`.

### 2.3 Model provider observations (reconciliation only) — deferred

**Not part of the #1496 delivery.** The observation `source` accepts `cloud`
only, the period row has no drift field, and `adjustments` stays `0`. The design
is kept here so it can be added without reshaping the row:

The operator may also post observations of the model provider's own reported
spend. They never replace the internal ledger as the admission figure; the
ledger is fresher and reserves before dispatch. They produce a **drift** value
(`provider_observed − ledger_spent_through(observed_through)`), surfaced to the
operator. Drift above a configured tolerance raises an alert and is recorded on
the period row; it does not change phase by itself. If the operator decides the
ledger under-counts, the correction is an explicit `adjustment` observation
that adds to the period's committed total and is audited like any other input.

## 3. The cap, the levels and the admission rule

The persisted platform policy is the one #1621 already stores
(`PlatformBudgetPolicy`): `period_cap`, `period_start`, `period_end`,
`warning_percent`, `stop_percent`, `protected_reserve`, `lag_allowance`. This
contract adds the cloud fields of section 2.2 and reads the existing ones as:

```
warning_level = period_cap × warning_percent / 100
stop_level    = period_cap × stop_percent    / 100
disposable    = stop_level − protected_reserve − lag_allowance      (validated > 0 already)

committed(now) = model.spent + model.reserved + cloud(now) + adjustments
```

- `protected_reserve` is the budget kept for existing consumers whose spend CMA
  does not meter. It is a deduction from what CMA may commit, not a metered
  figure.
- `lag_allowance` absorbs cloud projection error and any usage the model ledger
  has reserved but cannot yet see. It is configured, not derived.

Admission of a new liability `L` (a turn allowance, a request hold, or `0` for
a commitment with no model component such as a Sandbox creation) succeeds only
when:

```
committed(now) < disposable   and   committed(now) + L ≤ disposable
```

This is #1496's `spent + reserved + max_metering_lag_risk ≤ usable cap`
(M-SUST-01 `spend_reserved_lag`), with `lag_allowance` as the configured
`max_metering_lag_risk` and `disposable` as the usable cap.

The rule is evaluated **inside the admitting transaction** against the period
row. A liability-bearing admission (`L > 0`) also increments the settings fence,
as the foundation does today; a zero-liability admission reads the settings and
the period row inside its snapshot without the fence (section 4.1). The owner
and turn checks of the foundation still apply first; the platform check never
loosens them.

An allowance that does not fit is refused on its own; it does not close the
platform. The platform enters `SpendStop` only when `committed(now) ≥
disposable` (decision 29): the evaluator finds it, or an admission or
zero-liability check that is refused at that point. A zero-liability commitment
at exactly `disposable` is therefore refused, as the evaluator's stop already
says. A refusal aborts the admitting transaction, so the refusing call enters
any closure it found afterwards with a guarded single-document write (section
5.4); a successful admission that crosses the warning level enters
`SpendWarning` inside its own transaction.

When the operator raises `period_cap`, `stop_percent` or lowers the reserves in
a new settings revision, levels are recomputed from that revision and a closure
may lift (section 6). Lowering them never releases liability.

## 4. The period row

One document per period in `model_budget_reservations`, key
`platform:<period_start RFC3339>`. No new collection and no TTL; the row
outlives its period as evidence. The evaluator (section 5.3) pre-creates it with
`$setOnInsert` outside any admission transaction; an admission that still finds
it missing inserts it and treats a duplicate key as the existing transient retry.

```rust
pub struct PlatformPeriodLedger {
    pub period_start: u64,
    pub period_end: u64,
    pub settings_revision: u64,          // revision the levels were last computed under
    pub spent: u64,                      // settled model debits, this period
    pub reserved: u64,                   // outstanding model liability, this period
    pub adjustments: u64,                // operator corrections, section 2.3; 0 in this delivery
    pub cloud: Option<CloudObservation>, // latest observation; history lives in observation rows
    pub closures: Vec<Closure>,          // section 5
    pub evaluated_at: u64,               // last persisted evaluation
    pub revision: u64,                   // row CAS counter, incremented by every persisting writer
}

pub struct CloudObservation {
    pub observed_through: u64,
    pub units: u64,
    pub recorded_at: u64,
    pub settings_revision: u64,
}
```

### 4.1 Where the foundation and this row meet

Every existing ledger mutation gains one more write in the **same** transaction:

| Existing transaction | Owner row effect | Period row effect |
| --- | --- | --- |
| `OwnerBudget::reserve_liability` (turn start, command reserve, first message) | `reserved += L` | `reserved += L`; check `committed + L ≤ disposable` first |
| request `admission::reserve` | `reserved += L` | same |
| `OwnerBudget::settle` (turn ended) | `reserved −= R; spent += debit` | `reserved −= R; spent += debit` |
| request `settlement::settle` | `reserved −= R; spent += actual` | same |
| `release_unexecuted` (proven non-dispatch) | `reserved −= R` | `reserved −= R` |
| `retain_observed_liability` (late larger usage) | `reserved += extra` | `reserved += extra` (no admission check: it is already incurred) |

A reservation records the `period_start` it was charged to. Settlement after
`period_end` debits **that** row, not the current one; the two rows never both
hold the same liability. A reservation made before the platform policy existed
records none and never touches a period row. Owner-day rollover is irrelevant to
the period row.

Every write of the row filters on its `revision` and writes `revision + 1`; a
miss aborts and reruns the whole transaction, never only the row write.

**Contention.** Liability-bearing admissions and settlements write the period
row, so they serialize on it across replicas, next to the settings fence they
already increment. Zero-liability admissions (Sandbox, Workspace, Agent, Launch)
only read it inside their snapshot and never write the fence, so they stay off
both hot documents exactly as today. Within one process a `PlatformWrites`
mutex (the shape of the existing `AgentWrites`) queues period-row writers so
they do not abort each other; across replicas Mongo write conflicts remain the
authority, and every budget transaction loop uses the conversation store's
exponential `contention::backoff`. This is the price of an exact platform cap
and is accepted: admission is human-paced and the API runs as one replica with
rolling overlap. If measured contention still requires it, settlement can be
split into per-replica shards that the evaluator folds, without changing the
contract (admission still reads the folded total in-transaction).

### 4.2 Fail-closed conditions

Mirroring `OwnerBudget`, any of the following makes admission return
`BudgetError::Unavailable` and enters a `MeteringUnavailable` closure if one is
not present (outside the refusing transaction, section 5.4):

- the policy's `period_start`/`period_end` differ from the row's while the row
  has nonzero `spent`, `reserved` or `adjustments` (a period cannot be edited
  under liability; the operator starts a new period instead);
- `now ≥ period_end` while the policy still names that period (a finished
  period is unavailable, not open; the operator rotates to a new period);
- the settings document is missing, fails validation, or the fence write
  matches no document;
- cloud source stale or never observed in the period (section 2.2; a policy
  with no `cloud` section has metering off, not unavailable);
- arithmetic overflow.

## 5. Closures: the platform stop state

```rust
pub enum ClosureReason {
    SpendWarning,          // committed ≥ warning_level     → new registrations refused
    SpendStop,             // committed ≥ disposable        → new commitments refused, running work stopped and paused
    MeteringUnavailable,   // a required source is stale, missing or inconsistent → new commitments refused
    // Reserved for #1478 on the same type; not produced by this contract:
    // OperatorStop, CapacityLimit(kind)
}

pub struct Closure {
    pub reason: ClosureReason,
    pub since: u64,
    pub settings_revision: u64,   // revision under which it was entered
    pub committed_at_entry: u64,  // evidence, not authority
}
```

The set is the **platform admission state**. It is not a budget-private flag:
#1478 adds operator and capacity reasons to the same set and the same read
path, so the 60-second switch, the registration ceiling and the spend stop are
one mechanism with several reasons.

Admission reads also derive closures from the current policy, clock and period
evidence. A missing period is an unobserved period, never an open meter. Missing
or stale observations, finished periods and crossed spend thresholds therefore
refuse admission even when the evaluator has not run or cannot persist its
result. These reads perform no writes and never lift a persisted closure; only
the evaluator can commit a lift under the revision rules in section 6.

### 5.1 What each closure refuses

| Operation | Open | `SpendWarning` | `SpendStop` / `MeteringUnavailable` |
| --- | --- | --- | --- |
| Sign-in callback of a subject never seen (owner directory insert), visitors included; CMA's sign-up entry `/auth/login?screen=register` | admit | **refuse** `registration_closed` | refuse `registration_closed` |
| Existing owner: new message / turn, first message dispatch, auxiliary request hold | admit | admit | **refuse** `platform_budget_exhausted` / `budget_unavailable` |
| CreateSandbox, Launch, add Workspace / Agent, revive from Paused (owner, command or Launch) | admit | admit | **refuse** (same codes) |
| Control: cancel, stop, pause, approval decisions on an admitted turn, deletion, export, all reads | admit | admit | admit |

"Registration" is the first durable owner record. A public sign-up becomes a
CMA subject only at the sign-in callback, and visitors pass the same callback
(Launch requires an authenticated caller), so the warning action attaches there
and nowhere in the identity provider. CMA's own sign-up entry
(`/auth/login?screen=register`, which redirects to the identity provider's
registration page) is refused too, so the sign-up path does not lead people to
create an identity CMA will then refuse. `GET /auth/signup-options` likewise
reports `register_available: false` while registration is closed, read on
every request rather than from its provider cache, so the landing page hides
account creation. The entry check and the options are advisory: when the
admission state cannot be read they proceed, and the callback — which fails
closed — remains the authority, because an identity can also be created at the
provider directly. Existing owners are never affected: the callback finds them
in the user directory before it reads the admission state (decision 25).

Revival reaches a Paused Sandbox through the owner's lifecycle route, a
message's command, a Launch, an Agent creation that reuses a profile Agent
(including a Launch admitted before the stop), or a `Revive` Sandbox control
(`POST /api/v1/sandboxes/{id}/commands`, also used by bot delegation); each
reads the admission state before it schedules anything. `Pause` and `Retire` controls carry no new commitment and stay
available.

Approval decisions remain allowed under `SpendStop` because the turn they
continue has already reserved its full allowance; refusing them would not reduce
liability and would strand admitted work.

### 5.2 Running work under `SpendStop`

1. `SpendSource::stop_requested` additionally consults the period row (plain
   read behind a short cache, section 5.3). With a `SpendStop` closure present
   it returns `BudgetStop { reason: PlatformExhausted }` for every active turn,
   bound to the exact Agent, runtime generation and turn as today. The existing
   one-second monitor sends the guarded cancel; only the runtime's terminal
   evidence marks the turn failed; the failed receipt keeps its output and
   effects; same-key replay does not re-execute.
2. The **pause sweep** then pauses every Active Sandbox whose live Agents are
   all quiet, using the quiet-pause eligibility predicate (no active turn, no
   pending work, no queued messages, fenced live attachments), and every
   Active Sandbox with no live Agent. Two inactivity exemptions do not apply
   to this pause (decision 26): a Sandbox whose owner chose Always keep alive
   (`always_on`, or a historical profile's availability) is paused too, and a
   Sandbox without live Agents is a candidate. Every safety condition stays:
   pending or leased Workspace work, or an open owner session on a Sandbox
   without live Agents, leaves it running, and the maintenance transaction's
   revision, `last_seq` and fence guards apply unchanged. The pause never
   changes the owner's preference: `always_on` and the profile's availability
   are left as they are, and once the closure lifts and the owner revives the
   Sandbox, Always keep alive applies again. It ignores
   `inactivity.auto_pause_enabled`: this pause is a platform action, not an
   inactivity policy. The pause reason shown to the owner is fixed text:
   "Platform spend limit reached. Your work is saved; the Sandbox will be
   available again after the limit is raised."
3. Homes, receipts and history are retained. Retained disk continues to accrue
   in the cloud run rate; that is expected and already inside `disposable`.
4. Nothing resumes automatically. When a closure lifts (section 6) the owner
   sends a new message; revival then goes through ordinary admission. While
   `SpendStop` is present the sweep also defers managed runtime updates,
   because their rotation starts a runtime even on a paused Sandbox; they run
   again after the lift.

A failed pause can leave a retained home and runtime eligible for recovery.
Recovery admission checks the platform state in its transaction before starting
any replacement runtime. STOP or unavailable metering refuses that new
commitment; after a lift the same retained home and queued work remain eligible
for recovery. Existing pause completion can still proceed.

`MeteringUnavailable` performs steps 1–2 **only if** `SpendStop` is also
present. Staleness alone refuses new commitments and leaves running work alone.

### 5.3 Propagation and the 60-second budget

- **Admission:** no propagation delay. Liability-bearing admissions read the
  period row and increment the settings fence inside their transaction;
  zero-liability admissions read both inside their snapshot without the fence;
  revival reads the admission state at the service entry, because it carries no
  model liability and the turn admission behind it is exact. A closure written
  by any replica is therefore honored by the next admission on every replica.
  Cached reads may refuse early, never admit.
- **Running turns:** the `SpendSource` monitor polls each second per live Agent.
  It reads the period row with a plain read — no transaction, no fence — behind
  a 5-second process-local cache that can only report "closed". The cancel is
  sent within the cache window plus one poll plus the runtime round-trip.
- **Time-driven transitions and the pause sweep** run as a phase of the existing
  15-second reconciliation sweep on every replica; there is no separate worker
  or lease. The phase pre-creates the row, recomputes `cloud(now)`, enters or
  lifts closures, and persists only when something changed or `evaluated_at` is
  at least 60 seconds old, with a revision CAS; a CAS miss waits for the next
  sweep. It then drives the pause sweep. Within one sweep every action starts
  under 60 seconds; the evidence for TC-125/126 is the `since` instant on the
  closure versus the last admitted receipt and the first pause event.

Alerting on `SpendWarning`, `SpendStop` and `MeteringUnavailable`: a
structured `platform_spend_closure` operations event with the reason and
settings revision (no amounts). `MeteringUnavailable` alerts too, because it
refuses every new commitment until an operator records an observation or
rotates the policy. It is the alert record and fires once, from the writer whose
guarded update actually inserted the closure (section 5.4). Routing it to
on-call is EXT-MONITORING (#1570); a dashboard banner is later UI work.

### 5.4 Writing closures

Closures are entered in two ways:

- **Inside a successful admission:** `SpendWarning`, when the admitted liability
  crosses the warning level. It commits with the admission.
- **In a separate guarded transaction:** every closure derived by a refusing
  admission or by the evaluator. A refusal enters `SpendStop` only when
  `committed` without its own liability has reached `disposable`, a missing
  `SpendWarning` when `committed ≥ warning_level`, and `MeteringUnavailable`
  on a fail-closed condition. An allowance that merely does not fit enters no
  `SpendStop`.

A refusal first aborts the admission transaction, releasing all settings and
period locks. Its closure transaction then conditionally writes the exact
settings revision and the exact period revision that produced the decision.
The evaluator uses the same guard for entries, lifts and periodic refreshes.
A missing period uses an insert, so a concurrent creator also invalidates the
decision. If either revision changed, the whole decision is discarded and a
later evaluation derives a fresh one. Refusals never lift existing closures.
Only a confirmed closure commit emits its entry alerts; an uncertain result is
reported as unavailable and cannot authorize an action.

Closures are removed only by the evaluator (section 6).

## 6. Revision fencing

- **Settings revision.** Every level is computed from the settings document the
  writer read, whose revision is recorded on the row and on each closure. A
  settings PATCH that changes the platform policy increments the revision as
  today (CAS, `ConfigurePlatform`, confidential audit).
- **Lifting a closure** uses the guarded transaction in section 5.4, checking
  both policy and accounting revisions. The evaluator re-evaluates under a
  settings revision strictly greater
  than the closure's `settings_revision` and finds `committed < level`. A
  process that has not seen a newer settings revision cannot lift anything.
  `MeteringUnavailable` lifts when its cause clears (fresh observation, valid
  settings, a rotated period) under any revision ≥ its own, because it records a
  fact, not a decision.
- **Entering is monotonic** within a revision: a row cannot go from `SpendStop`
  to open without a newer revision or a new period. Hysteresis is therefore the
  revision itself; a projection that wobbles across a level cannot flap.
- **Period identity.** `period_start` is the row key. A policy whose period
  differs from the row's is a different period: with liability on the old row,
  the change is refused (section 4.2); with none, the new row starts at zero.
  Old rows are never deleted. A period whose end has passed is
  `MeteringUnavailable` until the operator rotates the policy.
- **Valuation.** Accounting units are one shared unit; an owner policy's
  `input/output_token_units` are that owner's price per token. Owner rows pin
  their own valuation on first use and fail closed when it changes under
  liability. The period row only sums amounts each owner already valued, so
  owners priced differently add up directly and the row pins no valuation.
- **Unknown commit outcomes** never produce a permit (unchanged from #1631).
- **Write shape gate.** The new policy fields and the new row kind are a new
  persisted shape. Following #1621/#1631, a strict default-off deployment flag
  `CHRONO_PLATFORM_SPEND_WRITES_ENABLED` gates the first nonempty save of the
  cloud fields and the first observation; until all serving readers and the
  rollback image understand the shape, saves return 503
  `model_budget_writes_unavailable`. Closing the gate later keeps reads and
  liability; it never restores pre-shape reader compatibility.

## 7. Typed outcomes

Additive to the existing `BudgetError` set:

| Code | Transport | Meaning | Guidance |
| --- | --- | --- | --- |
| `platform_budget_exhausted` | HTTP 402, no `Retry-After` | `SpendStop` closure present, or the commitment does not fit below `disposable` (section 3) | "The platform's spend limit for this period does not allow new work right now. Completed work is preserved. After the limit is raised, send a new message to continue; this turn will not replay automatically." |
| `registration_closed` | HTTP 503 | `SpendWarning` or any stop closure, subject never seen | "New sign-ups are temporarily closed. Existing accounts are unaffected." |
| `budget_unavailable` (existing) | unchanged | metering source stale or never observed, or period ended; ledger inconsistent | existing text |

The `platform_budget_exhausted` guidance holds both when the platform is
stopped and when one allowance does not fit, so it says nothing about a pause;
when a Sandbox is actually paused, the pause reason of section 5.2 explains it.
Shared transcripts show the same static guidance.

`platform_budget_exhausted` is a `BudgetError` variant so it travels the same
guarded-stop, terminal-receipt, AG-UI and Responses paths as the owner and turn
reasons. It is HTTP 402, consistent with the owner and turn budget refusals, so
existing clients treat it as a final outcome: the TypeScript SDK treats every
response of 500 or above as retryable and would otherwise retry at 100%. No
`Retry-After` is sent because lifting needs an operator revision, so no honest
retry time exists. (User decision, 2026-11-12.) `registration_closed` stays
HTTP 503: it appears only on the browser redirect path of sign-in, and the
landing page learns of the closure from `GET /auth/signup-options` first.

`registration_closed` has its own response body; it does not reuse the sign-in
failure body, whose text asks the user to start a new sign-in attempt.

## 8. Operator surface

All additive, under the existing authenticated admin routes, `ConfigurePlatform`
authority, revision CAS and confidential audit (operation and revision only; no
amounts).

- `model_budget.platform` gains `cloud: { run_rate_units_per_day, max_observation_age_seconds }`.
  Required together when present. Absent means platform spend metering is
  off (decision 31): owner and turn budgets still apply, and no period row,
  closure or platform refusal exists. The fields can only be saved once
  `CHRONO_PLATFORM_SPEND_WRITES_ENABLED` is open, so enabling #1621 owner
  budgets never closes the platform by itself.
- `POST /api/v1/admin/spend/observations`
  `{ source: "cloud", observed_through, units }`
  Appends an immutable observation row
  `observation:cloud:<period_start>:<observed_through>` in the same collection
  and updates the period row in one transaction. The key is scoped to its
  period, because adjacent periods share their boundary instant and each may
  record an observation there. A repeated identical post is idempotent; a
  conflicting one for the same period and `observed_through` is refused. `observed_through` must be ≤ now and ≥ the
  period start. The `model_provider` and `adjustment` sources of section 2.3
  are deferred.
- `GET /api/v1/admin/spend`
  Returns `metering_enabled` and, when it is true, the period row with levels,
  committed, cloud freshness, closures and the latest observation.
  `metering_enabled: false` means budgets or the `cloud` section are not
  configured, which is distinct from enabled but unavailable (a
  `MeteringUnavailable` closure). Admin-only; values are private by
  construction.
- There is no direct "reopen" command in this contract. Lifting a spend closure
  is a settings change (new revision) or a new period. The manual stop/resume
  switch is #1478's `OperatorStop`.

The Operations UI treats a missing `cloud` section or a false write capability
as "platform spend controls not yet available", in the same way it treats the
existing budget write gates.

## 9. Interaction with the runtime gateway (#1566)

Runtime model calls bypass CMA's HTTP broker. Their platform liability is still
bounded because every turn reserves its full allowance before it starts and
`SpendStop` interrupts it through the same guarded path. The gateway contract
must additionally (a) refuse new upstream requests for a turn whose Agent has a
`BudgetStop`, and (b) deliver authoritative final usage and final call count so
settlement can replace the conservative full-allowance debit on both the owner
row and the period row in one transaction. Until (b) exists, the period row's
`spent` is conservative by the same rule as the owner row: an unknown final
usage debits the whole reservation. That bias is toward stopping early, never
late.

**Low model-credit balance.** The balance of the model account is not metered
by CMA (#1566 EXT-LLM-CREDIT); typed 402 recovery for admitted turns is #1603.
Until #1478's `OperatorStop` exists, an operator closes admission on a low
balance by lowering `period_cap` or raising `protected_reserve` in a new
settings revision, which the next admission honors.

## 10. What #1478 inherits

- `Closure`, `ClosureReason`, the guarded closure write and the in-transaction
  read of the period row are the admission state. #1478 adds
  `OperatorStop { since, revision, actor }` and
  `CapacityLimit { kind: Registrations | RetainedHomes | ActiveSessions }`.
- The reconciliation-sweep phase and its 15-second tick are the propagation
  mechanism; #1478 proves the ≤ 60 s bound with its own ceilings.
- The refusal matrix of section 5.1 gains columns, not new mechanisms.
- Existing consumer protection stays `protected_reserve` here; #1478's
  NyxID/direct-API quota protection is a count rule on the same state.

## 11. Verification (TC-125, TC-126, TC-067 platform clauses)

All on isolated staging with synthetic units, an isolated test owner, a test
period and controlled observations. No real funds are consumed.

1. **Warning**: post observations until `committed ≥ warning_level`. Expect:
   closure `SpendWarning` with `since`; alert emitted once; a never-seen
   subject receives `registration_closed` at the sign-in callback and at the
   sign-up entry; an existing owner's new turn is admitted; no running turn is
   touched.
2. **Stop**: run concurrent admissions on two replicas until the next
   allowance no longer fits. Expect: exactly the admissions that fit are
   admitted, none overshoot; the one that does not fit is refused with
   `platform_budget_exhausted`, no `SpendStop` is entered and running turns
   continue. Then drive `committed ≥ disposable` by settlement or observation.
   Expect: exactly one `SpendStop` is visible, entered by the evaluator or by a
   refused admission or zero-liability check; CreateSandbox, Launch, revive,
   new turn, first message and auxiliary holds are refused with
   `platform_budget_exhausted`; every running turn receives the guarded cancel
   within 60 s of the closure's `since`; failed receipts retain output; all
   quiet Sandboxes reach Paused with the platform reason; homes retained;
   approvals and cancels on admitted work still succeed.
3. **Lift**: PATCH a higher cap (new revision). Expect: the evaluator lifts
   `SpendStop` and `SpendWarning`; an explicit new message revives and is
   admitted; nothing replays; a process that has not seen the new revision
   cannot lift.
4. **Stale cloud**: let `observed_through` age past the bound. Expect:
   `MeteringUnavailable`; new commitments refused with `budget_unavailable`;
   running turns continue; a fresh observation lifts it without a settings
   change.
5. **Liability retention**: kill a runtime mid-turn; expect `reserved`
   unchanged until terminal evidence; period and owner rows agree.
6. **Cross-period settlement**: reserve before `period_end`, settle after;
   expect the old row debited and the new row untouched.
7. **Finished period**: let `now` pass `period_end` without rotating the policy.
   Expect `MeteringUnavailable`; new commitments refused; running turns
   continue.
8. **Concurrency**: 20 concurrent Launches with first messages under a cap that
   fits them all. Expect: zero `"contention exhausted"` warnings, zero
   `budget_unavailable` refusals, and `platform_settings.model_budget_admission_fence`
   growing only by the number of liability-bearing admissions.
9. **Reader gate**: a predecessor image reads a row with cloud fields present
   → the write gate refused the save; with the gate open the predecessor is
   not a valid rollback target and the runbook says so.

Throughout: `CHRONO_USAGE_METRICS_ENABLED`, `CHRONO_USAGE_BILLING_ENABLED` and
`CHRONO_NYXID_SANDBOX_BILLING_ENABLED` read false, and there is no unexpected
402.

Evidence per the gate rules: image digest, source SHA, environment, time
window, N, flags, raw result location; no amounts, identities or credentials.

## 12. Decisions made in this draft

Marked so they can be overturned without re-reading:

1. Cloud spend is operator observation + run-rate projection; per-Sandbox
   allocation is not added to the platform figure (double-counting rule).
2. Model provider observations would reconcile, not drive admission; they are
   deferred (decision 12).
3. The period row lives in `model_budget_reservations`, not a new collection.
4. The platform check runs inside every liability-bearing reservation
   transaction; their serialization on one row is accepted, with local
   scheduling (decision 16).
5. `platform_budget_exhausted` is HTTP 402, consistent with the owner and turn
   budget refusals, so clients that retry every response of 500 or above treat
   it as final (user decision, 2026-11-12); `registration_closed` is HTTP 503;
   neither carries `Retry-After`.
6. Staleness closes admission only; exhaustion interrupts and pauses.
7. Lifting a spend closure requires a newer settings revision or a new period;
   no reopen button here (that is #1478's operator switch).
8. A third write-shape gate `CHRONO_PLATFORM_SPEND_WRITES_ENABLED`, following
   the two precedents.
9. Approval decisions on admitted turns stay allowed under `SpendStop`.
10. Evaluation and the pause sweep run as a phase of the reconciliation sweep;
    no new worker or lease.
11. Revival checks the admission state at the service entry, not in-transaction.
12. Section 2.3 is deferred: `source` accepts `cloud` only, `adjustments` stays
    `0`, the row has no drift field.
13. Closures derived by a refusal or evaluator commit in a separate transaction
    guarded by both settings and period revisions; stale decisions are discarded.
14. Zero-liability admissions read settings and the period row without the
    settings fence.
15. Stop polling reads the period row without a transaction behind a 5-second
    cache that may stop but never admit.
16. A process-wide `PlatformWrites` mutex schedules local period-row writers,
    and budget request transactions share `contention::backoff`.
17. Registration is refused at the sign-in callback (authority, fail closed)
    and at `/auth/login?screen=register` (advisory, proceeds when the state is
    unreadable).
18. A finished period is `MeteringUnavailable` until the policy is rotated.

Recorded during implementation:

19. A refused commitment carries `platform_budget_exhausted` when `SpendStop`
    is present and `budget_unavailable` when only `MeteringUnavailable` is,
    for admissions, zero-liability commitments and revival alike.
20. The evaluator enters `SpendStop` at `committed ≥ disposable` (section 5);
    admission refuses at `committed + L > disposable` or `committed ≥
    disposable` (section 3, decision 29). A row with no liability adopts a
    changed `period_end` of the same period; a changed `period_start` is a
    different row.
21. Inside a snapshot transaction a concurrent period-row writer surfaces as a
    retried write conflict, so a residual revision-filter miss (section 4.1)
    is treated as an inconsistency and fails closed with `budget_unavailable`
    instead of re-running.
22. The `platform_spend_closure` event fires from the writer whose guarded
    update inserted the closure (refusals and the evaluator) and from a
    successful admission that enters `SpendWarning` in its transaction; a
    retried admission transaction can emit that warning event twice.
23. Every refusal aborts its admission transaction before trying to persist
    closures, releasing its settings and period locks. A missing period is
    inserted only under the same policy guard; a concurrent insert invalidates
    the decision. The evaluator can retry closure derivation later.
24. An enabled platform meter requires an owner budget policy for every new
    model commitment, including queued turn starts and auxiliary requests.
    Missing valuation refuses with `budget_unavailable` before dispatch;
    a persisted `SpendStop` still takes precedence as
    `platform_budget_exhausted`. Only while platform metering is off does the
    historical unbudgeted-owner bypass apply. Zero-liability commitments read
    the state for every owner. This follows the accepted pre-request
    reservation contract on #1496.
25. Registration checks the user directory first: an existing subject signs
    in without reading the platform state, so an unreadable state never locks
    an owner out. Only a never-seen subject reads the state, and a failed read
    refuses its callback (fail closed). Without a configured directory every
    subject is never-seen.
26. Pause both under `SpendStop` (user ruling, 2026-11-12). A platform pause
    overrides an owner's Always keep alive preference (`always_on` or a
    historical profile's availability) and also pauses an Active Sandbox with
    no live Agent; an inactivity pause keeps both exemptions unchanged. Busy
    work is never interrupted: live Agents must be quiet as before, and
    pending or leased Workspace work, or an open owner session on a Sandbox
    without live Agents, leaves the Sandbox running. The preference itself is
    never changed, so it applies again once the owner revives the Sandbox
    after the closure lifts.
27. A failed evaluation is logged, keeps the last known stop answer and is
    retried on the next attempt; admission keeps failing closed on its own.
28. The two admin spend routes are served behind `ConfigurePlatform` and the
    third gate but are not yet in the published OpenAPI document or the
    public-contract corpus.
29. Stop at 100% (user ruling, 2026-11-12). The platform enters `SpendStop`
    only when `committed` itself reaches `disposable`. An allowance that does
    not fit (`committed + L > disposable` while `committed < disposable`) is
    refused alone with `platform_budget_exhausted`: it enters no `SpendStop`
    (only a missing `SpendWarning` once `committed ≥ warning_level`), running
    turns continue and no Sandbox is paused. A refused admission or
    zero-liability check that finds `committed ≥ disposable` still enters
    `SpendStop` outside its transaction, and a zero-liability commitment at
    exactly `disposable` is refused, consistent with the evaluator
    (decision 20).
30. Only owner rows pin a token valuation; the period row sums amounts that
    are already valued in the shared accounting units, so owners with
    different valuations are admitted side by side. Evaluation runs on every
    reconciliation attempt, at most once per 15 seconds, independent of the
    sweep's other phases.
31. Platform spend metering is enabled only by `model_budget.enabled` together
    with a `platform.cloud` section. #1621 already requires a platform policy
    whenever budgets are enabled, and the cloud fields can be saved only after
    the third write gate opens, so treating a missing `cloud` as unavailable
    would close every commitment as soon as owner budgets were switched on.
    Without `cloud` the settings snapshot carries no platform policy: no period
    row is created, no platform check or `PlatformWrites` scheduling applies,
    the evaluator and admission state report nothing, and owner and turn
    budgets behave exactly as in #1621. With `cloud`, every fail-closed rule is
    unchanged: a missing or stale observation is `MeteringUnavailable`.
    Removing `cloud` is a settings revision that turns metering off.

## 13. Data migration record

Follows [CMA data migrations](CMA_Data_Migrations.md). Migration ID:
`2026-11-12-am-platform-spend-period-ledger`; service: Agent Manager; issue
#1496, PR #1741; phase: additive expand with gated writers; no backfill and no
contract step. Source identity is the PR's exact reviewed head; the deployment
digest, operator window and production receipts remain release-owner items.

Key point, verified in code: until `CHRONO_MODEL_BUDGET_WRITES_ENABLED` and
`CHRONO_PLATFORM_SPEND_WRITES_ENABLED` are both open **and** an operator saves
`model_budget.platform.cloud`, none of the new durable shapes below is written.
The settings snapshot (`store/budget/period.rs::snapshot`) carries a platform
policy only for `model_budget.enabled && platform.cloud.is_some()` (decision
31); `PlatformSettingsService::configure_platform` refuses to save `cloud`
while the third gate is closed; `PlatformSpendService::record_cloud_observation`
refuses while it is closed. Every new writer starts from that snapshot: the
period row and `period_start` need the platform policy; a `SpendStop` or
`MeteringUnavailable` closure needs the period row; `platform_exhausted` (stop
rows, command sidecars, first-message codes) and the platform pause need a
closure or a period-row refusal. The new and extended records are strict
(`deny_unknown_fields`) readers, so once written, an image without this
change is no longer a valid rollback target.

| Required field | Scoped disposition |
| --- | --- |
| Store and writers | Agent Manager owns `model_budget_reservations`, `platform_settings`, `agent_commands`, `agents` and `sandboxes`. Writers: command reservation and turn start (`store/budget/mod.rs`), auxiliary request holds and settlement (`store/budget/requests/*`), turn settlement, release and retain through the conversation and message-checkpoint commits (`period::mirror`), refusals (guarded `update_one` outside the aborted transaction plus `ensure_period_row` upsert), the evaluator on every reconciliation attempt (`evaluate_period`), the operator observation route (`POST /api/v1/admin/spend/observations`), the stop poll (`stop:` rows), command rejection (`model_budget_failure` sidecar), Launch first-message startup failure, and the maintenance pause (`paused_notice`). No index build, startup DDL, importer, runtime home, browser asset, key or external-effect writer is added. |
| Durable formats | New: `platform:<period_start>` rows `{_id, document_schema_version: 1, record: PlatformPeriodLedger}` (strict; nested `Closure`, `CloudObservation` and the `ClosureReason` enum `spend_warning`, `spend_stop`, `metering_unavailable`), and immutable `observation:cloud:<period_start>:<observed_through>` rows with a strict `CloudObservation` record. Extended: `PlatformBudgetPolicy.cloud` in `platform_settings` (strict struct; omitted when absent, so #1621 settings bytes are unchanged); optional `period_start` on `TurnRecord` (`turn:`, `command:`) and `RequestRecord` (`request:`), both strict and omitted when absent; the `BudgetError` value `platform_exhausted` in `stop:` rows and in the `agent_commands` top-level `model_budget_failure` sidecar (the strict `record.work.failure.code` keeps the legacy `execution_failed`); the first-message failure code string `platform_budget_exhausted`. `sandboxes.lifecycle.paused_notice` gains a new fixed string with no format change. Not persisted: `InactivityPauseCause` (request only), registration decisions, `metering_enabled`, the stop cache and the alert log. Activity records reuse `Configure` with target `Resource("platform_spend")` and a revision detail. No validator, TTL or encryption change. |
| Index / bounded work | No new or modified index. Every new read and write is an `_id` point operation on one period row, one observation row or one reservation row, inside the existing transactions or as one guarded single-document update. The evaluator reads and writes at most one period row per 15 seconds per replica. The period row is one hot document per period; `PlatformWrites` schedules local writers (concurrency rule 5). |
| Reader floor | The compatible floor for retained data written after metering is enabled is this change (the merged #1741 source). Earlier images reject `cloud` in the strict settings policy, `period_start` in strict reservation records, `platform_exhausted` in stop rows and command sidecars, and `platform_budget_exhausted` as a first-message startup code. They never read `platform:` or `observation:` rows, except the retirement-archive census below. Tests: Layer 2 period-row serialization and strictness tests, Mongo contracts on disposable `mongo:8.0.13` replica sets for every writer, and the restored #1621 budget tests passing unchanged with metering off. No predecessor executable has been run against new data; no prior image is certified. |
| Writer gate | `CHRONO_PLATFORM_SPEND_WRITES_ENABLED` (default `false` in code and `infra/api/configmap.yaml`), effective only with `CHRONO_MODEL_BUDGET_WRITES_ENABLED`, gates saving `cloud` and recording observations. Metering then runs only while budgets are enabled and `cloud` is present. Closing the gate stops new `cloud` saves and observations but does not stop an already configured meter; turning metering off is a settings revision that removes `cloud` (allowed with the gate closed) or disables budgets. Finite recovery writes continue after that: settlement of retained reservations whose `period_start` is set still updates their period row through `mirror`; stop rows and sidecars already written remain. Reads, exact replays and admitted-operation settlement stay available throughout. One API replica; all serving processes must converge on the flag before `cloud` is saved. |
| Backfill and recovery | No backfill or transformation. Period rows are created lazily (`$setOnInsert` upsert or in-transaction insert) and observations are operator posts. Reservations made before metering carry no `period_start` and never touch a period row. Partial failures: closure writes are guarded single-document updates (first writer wins, repeats are no-ops); period-row saves use a revision CAS inside the transaction; the evaluator re-derives transitions; an observation is one transaction with an idempotent identical repeat and a refused conflicting one. |
| Backup / dry-run | No deletion, rewrite, index or validator contraction, so no destructive-step backup is required. The recovery unit is `model_budget_reservations`, `platform_settings`, `agent_commands`, `agents` and `sandboxes` together. Disposable local Mongo contracts are the dry run; no production backup, restore rehearsal or dry run is claimed. |
| Rollback / completion | Before `cloud` is first saved, rollback to the predecessor image is unaffected by this change; the release owner confirms with read-only queries that no `platform:` or `observation:cloud:` row, no settings `cloud`, and no `platform_exhausted` stop row or sidecar exists. Saving `cloud` is the point of irreversibility for earlier images: afterwards recovery is a forward fix or this image, and removing `cloud` does not restore predecessor eligibility because retained fields stay; never delete or unset them to fit an older reader. Prerequisite before enabling metering: the retirement-archive census (`store/retirement_archive/census_budget.rs`, #1726/#1716) accepts only `owner:`, `command:`, `turn:`, `request:` and `request-turn:` keys and refuses capture when any other key exists, so `platform:` and `observation:cloud:` rows (and #1621's existing `stop:` rows) would refuse every capture until a reviewed census extension lands. That extension is [ChronoAIProject/cma#1754](https://github.com/ChronoAIProject/cma/pull/1754) (migration `2026-11-12-am-retirement-budget-shapes`); metering may be enabled only after it is merged and deployed, and its merge is the condition of record. Pending release-owner receipts: deployment digest, OFF rollout, the 11-14 drill, the 11-21 production check and sign-off. No production activation or migration is claimed by this PR. |
