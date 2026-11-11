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
committed(now) + L ≤ disposable
```

This is #1496's `spent + reserved + max_metering_lag_risk ≤ usable cap`
(M-SUST-01 `spend_reserved_lag`), with `lag_allowance` as the configured
`max_metering_lag_risk` and `disposable` as the usable cap.

The rule is evaluated **inside the admitting transaction** against the period
row. A liability-bearing admission (`L > 0`) also increments the settings fence,
as the foundation does today; a zero-liability admission reads the settings and
the period row inside its snapshot without the fence (section 4.1). A refusal
aborts the admitting transaction, so the refusing call enters the closure
afterwards with a guarded single-document write (section 5.4); a successful
admission that crosses the warning level enters `SpendWarning` inside its own
transaction. The owner and turn checks of the foundation still apply first; the
platform check never loosens them.

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
- cloud source stale or unconfigured (section 2.2);
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
create an identity CMA will then refuse. The entry check is advisory: when the
admission state cannot be read it proceeds, and the callback — which fails
closed — remains the authority, because an identity can also be created at the
provider directly. Existing owners are never affected by `SpendWarning`.

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
2. The **pause sweep** then pauses every Active Sandbox whose Agents are all
   quiet, using the quiet-pause eligibility predicate (no active turn, no
   pending work, no queued messages, fenced live attachments). It ignores
   `inactivity.auto_pause_enabled`: this pause is a platform action, not an
   inactivity policy. The pause reason shown to the owner is fixed text:
   "Platform spend limit reached. Your work is saved; the Sandbox will be
   available again after the limit is raised."
3. Homes, receipts and history are retained. Retained disk continues to accrue
   in the cloud run rate; that is expected and already inside `disposable`.
4. Nothing resumes automatically. When a closure lifts (section 6) the owner
   sends a new message; revival then goes through ordinary admission.

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

Alerting on `SpendWarning` and `SpendStop`: a structured
`platform_spend_closure` operations event with the reason and settings revision
(no amounts). It is the alert record and fires once, from the writer whose
guarded update actually inserted the closure (section 5.4). Routing it to
on-call is EXT-MONITORING (#1570); a dashboard banner is later UI work.

### 5.4 Writing closures

Closures are entered in two ways, both monotonic:

- **Inside a successful admission:** `SpendWarning`, when the admitted liability
  crosses the warning level. It commits with the admission.
- **Outside any transaction:** every closure entered on a refusal (an admission,
  a zero-liability check) and every closure the evaluator enters. The
  foundation's transaction loops abort on any domain error, so a closure pushed
  inside a refusing transaction would roll back with it. Instead the refusing
  call, after the abort, issues one guarded single-document update:

  ```
  update_one({_id: "platform:<period>", "record.closures.reason": {$ne: <reason>}},
             {$push: {"record.closures": <closure>}, $inc: {"record.revision": 1}})
  ```

  First writer wins and repeats are no-ops. The caller whose update modified the
  row is the only one that emits the alert. This is the pattern the foundation
  already uses for `stop:` rows.

Closures are removed only by the evaluator (section 6).

## 6. Revision fencing

- **Settings revision.** Every level is computed from the settings document the
  writer read, whose revision is recorded on the row and on each closure. A
  settings PATCH that changes the platform policy increments the revision as
  today (CAS, `ConfigurePlatform`, confidential audit).
- **Lifting a closure** is a revision-CAS update of the period row (`$pull` of
  the closure, `$set` of `settings_revision` and `evaluated_at`) made by the
  evaluator after it re-evaluates under a settings revision strictly greater
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
| `platform_budget_exhausted` | HTTP 503, no `Retry-After` | `SpendStop` closure present | "The platform's spend limit for this period has been reached. Completed work is preserved and your Sandbox is paused, not lost. After the limit is raised, send a new message to continue; this turn will not replay automatically." |
| `registration_closed` | HTTP 503 | `SpendWarning` or any stop closure, subject never seen | "New sign-ups are temporarily closed. Existing accounts are unaffected." |
| `budget_unavailable` (existing) | unchanged | metering source stale, unconfigured or period ended; ledger inconsistent | existing text |

`platform_budget_exhausted` is a `BudgetError` variant so it travels the same
guarded-stop, terminal-receipt, AG-UI and Responses paths as the owner and turn
reasons. 503 rather than 402 is deliberate: the owner has not exhausted anything
of theirs. No `Retry-After` is sent because lifting needs an operator revision,
so no honest retry time exists. **Decision pending review**: keep 503, or stay
with the foundation's uniform 402 for all budget refusals.

`registration_closed` has its own response body; it does not reuse the sign-in
failure body, whose text asks the user to start a new sign-in attempt.

## 8. Operator surface

All additive, under the existing authenticated admin routes, `ConfigurePlatform`
authority, revision CAS and confidential audit (operation and revision only; no
amounts).

- `model_budget.platform` gains `cloud: { run_rate_units_per_day, max_observation_age_seconds }`.
  Required together when present; absent means the cloud source is not
  configured, which is `MeteringUnavailable` once `enabled` is true.
- `POST /api/v1/admin/spend/observations`
  `{ source: "cloud", observed_through, units }`
  Appends an immutable observation row `observation:cloud:<observed_through>`
  in the same collection and updates the period row in one transaction. A
  repeated identical post is idempotent; a conflicting one for the same
  `observed_through` is refused. `observed_through` must be ≤ now and ≥ the
  period start. The `model_provider` and `adjustment` sources of section 2.3
  are deferred.
- `GET /api/v1/admin/spend`
  Returns the period row with levels, committed, cloud freshness, closures and
  the latest observation. Admin-only; values are private by construction.
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
2. **Stop**: drive `committed ≥ disposable` with concurrent admissions on two
   replicas. Expect: exactly the admissions that fit are admitted, none
   overshoot; exactly one `SpendStop` is visible after the refusing
   transactions aborted; CreateSandbox, Launch, revive, new turn, first message
   and auxiliary holds are refused with `platform_budget_exhausted`; every
   running turn receives the guarded cancel within 60 s; failed receipts retain
   output; all quiet Sandboxes reach Paused with the platform reason; homes
   retained; approvals and cancels on admitted work still succeed.
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
5. `platform_budget_exhausted` is HTTP 503; `registration_closed` is HTTP 503;
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
13. Closures entered on a refusal or by the evaluator are written outside any
    transaction with a guarded update; first writer wins; precedent `stop:` rows.
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
    admission refuses at `committed + L > disposable` (section 3). A row with
    no liability adopts a changed `period_end` of the same period; a changed
    `period_start` is a different row.
21. Inside a snapshot transaction a concurrent period-row writer surfaces as a
    retried write conflict, so a residual revision-filter miss (section 4.1)
    is treated as an inconsistency and fails closed with `budget_unavailable`
    instead of re-running.
22. The `platform_spend_closure` event fires from the writer whose guarded
    update inserted the closure (refusals and the evaluator) and from a
    successful admission that enters `SpendWarning` in its transaction; a
    retried admission transaction can emit that warning event twice.
23. A refusal that follows the same transaction's insert of a missing period
    row aborts that transaction before the guarded closure write, and every
    refusal pre-creates the row outside it, so the closure is not lost.
24. Only owners with a model budget policy reserve liability; their messages
    and queued turn starts read the platform state. Zero-liability
    commitments read it for every owner.
25. Registration reads the user directory only while a closure exists;
    without a configured directory a closure refuses the callback.
26. The pause sweep keeps an owner's Always keep alive preference and does
    not pause Sandboxes with no live Agents (the quiet-pause predicate's
    existing rule). Whether a platform pause should override keep-alive is
    left to #1478.
27. A failed evaluation is logged, keeps the last known stop answer and is
    retried on the next attempt; admission keeps failing closed on its own.
28. The two admin spend routes are served behind `ConfigurePlatform` and the
    third gate but are not yet in the published OpenAPI document or the
    public-contract corpus.
29. A refusal at `committed + L > disposable` enters `SpendStop` even while
    `committed` is below `disposable`, so the platform stops when the next
    allowance no longer fits (TC-126). Where one owner's allowance exceeds the
    remaining headroom this stops running work early; revisit with #1478.
30. Only owner rows pin a token valuation; the period row sums amounts that
    are already valued in the shared accounting units, so owners with
    different valuations are admitted side by side. Evaluation runs on every
    reconciliation attempt, at most once per 15 seconds, independent of the
    sweep's other phases.
