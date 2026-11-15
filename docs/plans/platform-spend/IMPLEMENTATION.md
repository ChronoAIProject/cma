# Platform Spend Metering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement #1496 — period spend metering on top of the owner/turn reservation foundation, the 80 % registration closure, the 100 % commitment stop with safe pause, and the operator surface — without widening into #1478, #1566 or UI work.

**Architecture:** One period row in `model_budget_reservations` is written in the same Mongo transactions that already move owner rows; its `closures` set is the platform admission state. Admission paths read it in-transaction, `SpendSource::stop_requested` returns a new `BudgetError::PlatformExhausted` from it, and the existing 15-second reconciliation sweep gains a phase that evaluates time-driven transitions and drives the pause sweep. Operator input (cloud observations, the `cloud` policy fields) is additive admin API behind a third strict write gate.

**Tech Stack:** Rust 1.96 workspace (`layer2-core`, `layer1-public`, `cma`), MongoDB transactions, axum, serde, `cargo nextest`; TypeScript SDK public-contract fixtures.

**Spec:** `docs/CMA_Platform_Spend_Metering.md`. Section numbers below (§) refer to it.

## Global Constraints

- No price, amount, currency, funding value or production identity anywhere, including tests and fixtures (spec preamble; CLAUDE.md §6 gitleaks).
- Layer rule: `layer1-public → layer2-core → layer3-adapter`; no provider or vendor vocabulary in `layer2-core` (spec §1, CLAUDE.md §3). Types are "model" and "cloud", never a vendor name.
- Every new persisted shape is gated: `CHRONO_PLATFORM_SPEND_WRITES_ENABLED`, strict boolean, false in code and `infra/api/configmap.yaml` (spec §6). With `model_budget.enabled == false` or `model_budget.platform == None`, every new code path is a no-op.
- A new config key must appear in `api/crates/cma/src/config/features.rs`, `infra/api/configmap.yaml` and the key table in `docs/CMA_Operations.md` in the same commit (CLAUDE.md §4; CI `check-config-keys.sh`, `check-config-docs.mjs`).
- Liability is released only by authoritative settlement or proven non-dispatch, never by elapsed time (spec §2.1, §4.1).
- Admission of liability `L` succeeds iff `committed(now) + L ≤ disposable`, evaluated inside the admitting transaction (spec §3).
- `platform_budget_exhausted` and `registration_closed` are HTTP 503 with no `Retry-After`; existing owner/turn codes stay 402 (spec §7, decision 5).
- Source files stay below 500 lines (repository size gate used by #1621/#1631).
- Commits: one logical change each, imperative mood, no `Co-Authored-By` (CLAUDE.md §8).
- Issue formula mapping: `committed + L ≤ disposable` is #1496's `spent + reserved + max_metering_lag_risk ≤ usable cap` (M-SUST-01 `spend_reserved_lag`), with `lag_allowance` as the configured `max_metering_lag_risk` and `disposable` as the usable cap. State it this way in the PR so the gate owners can map it.
- Platform refusals carry no `Retry-After`: lifting needs an operator revision, so no honest retry time exists. Deliberate deviation from the issue's pre-decision remediation note; spec §7.
- Issue linkage: the PR body and individual commits say `Refs #1496` (owner decision 2026-11-11, a deliberate deviation from the issue's `Closes` constraint). Merging does not close the issue; it stays open until the 11-14 drill and 11-21 production check carry evidence, and is closed by hand then.

## Out of scope (do not implement)

- Spec §2.3 model-provider observations, drift and `adjustment` source: the observation `source` enum ships with `cloud` only; `adjustments` stays `0`.
- `OperatorStop`, `CapacityLimit`, count ceilings, ≤ 60 s evidence runs (#1478).
- Gateway final usage / final call count hook (#1566).
- Web/UI: Operations banner, signed-out copy for `registration_closed`, SDK methods for the new admin routes beyond the one availability field.
- Settlement sharding (spec §4.1 fallback).

## Delivered elsewhere (declared, not re-implemented)

The 11-11 deliverable in #1496 lists these; they are owned by other work and this plan relies on them as they are:

- Per-owner daily model allowance, its local-day rollover and TC-127: #1621 (`BudgetPolicy.owner_day_limit`, `OwnerBudget::advance`). The 2-Sandbox default for public visitors: #1485 (`PlatformLimits.visitor_max_sandboxes` 1→2).
- Automatic stop on a **low model-credit balance** at the gateway: the balance is not metered by CMA (#1566 EXT-LLM-CREDIT); typed 402 recovery is #1603. Until #1478's `OperatorStop` lands, an operator closes admission by lowering `period_cap` or raising `protected_reserve` in a new settings revision, which this plan honors immediately (Review Focus 4).
- `CHRONO_USAGE_METRICS_ENABLED`, `CHRONO_USAGE_BILLING_ENABLED` and `CHRONO_NYXID_SANDBOX_BILLING_ENABLED` stay false and untouched; the 11-14 drill asserts it.
- Alert delivery: the `platform_spend_closure` tracing event (Task 6) is the alert record; routing it to on-call is EXT-MONITORING (#1570) receivers.
- The issue's **Affected Files** list is review-evidence pointers (funding, upstream 402, billing docs — #1603/#1566/#1483 territory), not a change list. The first PR lists the files actually changed, per the issue's own instruction.

## Concurrency rules

These follow patterns already in the store; each names its precedent. Every task below is written against them.

1. **A refusal never carries a durable fact.** The transaction loops abort on any `ApplyError::Domain` (`agent_conversation_store.rs:94`, `budget/requests.rs:716`), so a closure pushed inside a refusing transaction is rolled back. Closures entered on refusal are written **outside** the transaction with a guarded single-document `update_one` (`filter: closures.reason $ne <reason>`, `$push`, `$inc record.revision`), first writer wins, idempotent. Precedent: the `stop:` row in `budget/source.rs:45`. Closures entered on a *successful* admission (`SpendWarning`) stay inside that transaction.
2. **Zero-liability paths never write the settings fence.** Sandbox, Workspace, Agent and Launch admission read settings and the period row inside their transaction snapshot without `$inc`; they stay off the global hot document exactly as they are today (per-owner `sandbox_admissions` fence, `visitor_launch_days` per `day:owner`). Only liability-bearing reservations use the fenced read.
3. **Polling paths never open transactions.** `stop_requested` runs every second per live Agent; it reads the period row with a plain `find_one` like `current_budget_policy` and serves a process-local 5-second cache. The cache may answer "closed" (stop/refuse), never "open" for admission.
4. **Periodic writers write only on change.** The evaluator persists when a closure entered or lifted or `evaluated_at` is ≥ 60 s old, with a `revision` CAS; a CAS miss skips to the next sweep. Precedent: the reconciliation sweep's unchanged-candidate skip (docs/investigations/cma-db-perf).
5. **Local writers are scheduled, not raced.** A process-wide `PlatformWrites` async mutex (shape of `agent_write_coordination.rs::AgentWrites`, one key) is held across every transaction that will write the period row: turn start, turn end, request reserve/settle, release. Taken after the existing `agent_writes` lock, before `start_session`, released after commit/abort. Scheduling only; Mongo transactions remain the authority across replicas (one API replica with rolling overlap, CLAUDE.md §1). The shared `contention::backoff` replaces the fixed `5 ms × attempt` sleep in `requests.rs`.
6. **Singletons are created outside admission transactions.** The period row is pre-created by the evaluator with `$setOnInsert` (precedent: `budget/mod.rs::initialize_settings`); an admission that still finds it missing inserts it and treats a duplicate key as the existing transient retry.
7. **A finished period is unavailable, not open.** `now ≥ period_end` with an unrotated policy is `MeteringUnavailable`: new commitments refused, running work untouched.

## Review Focus

1. Policy saved with `enabled: true`, `platform` present, `cloud` absent → every commitment is refused with `budget_unavailable` and a `MeteringUnavailable` closure is recorded; nothing panics. Test in Task 2.
2. Activation on live traffic: a turn reserved before the platform policy existed settles after activation → the period row is not debited and settlement does not fail (`period_start: None` on the record skips the mirror). Test in Task 2.
3. Two store instances cross the stop level in the same second, both refusing transactions abort → exactly one `SpendStop` closure is visible to a third reader, one alert event. Test in Task 2 (two `MongoSandboxStore::open` instances) and Task 6.
4. Operator lowers `period_cap` below current `committed` → the next admission refuses and records `SpendStop`; `reserved` is unchanged. Test in Task 2.
5. Observation with `observed_through` in the future or before `period_start` → 422, period row unchanged. Test in Task 7.

---

## File map

| Path | Responsibility |
| --- | --- |
| `api/crates/layer2-core/src/budget/platform.rs` (modify) | `PlatformBudgetPolicy.cloud`, `CloudSpendPolicy`, `SpendLevels`, `levels()`, `period_bounds()` |
| `api/crates/layer2-core/src/budget/period.rs` (create) | `PlatformPeriodLedger`, `Closure`, `ClosureReason`, `CloudObservation`, pure accounting and transitions |
| `api/crates/layer2-core/src/budget/period_tests.rs` (create) | unit tests for Task 1 |
| `api/crates/layer2-core/src/budget/spend.rs` (create) | `PlatformSpendLedger` trait, `PlatformAdmissionState`, `PlatformSpendState`, `CloudObservationInput`, `PlatformSpendService` |
| `api/crates/layer2-core/src/budget.rs` (modify) | exports; `BudgetError::PlatformExhausted` |
| `api/crates/cma/src/store/budget/period.rs` (create) | Mongo period row: load/init, `PolicySnapshot`, in-transaction admission helper, mirrors |
| `api/crates/cma/src/store/budget/mod.rs`, `requests.rs`, `requests/admission.rs`, `requests/settlement.rs`, `cancellation.rs`, `source.rs` (modify) | mirror writes, admission check, stop path, stop cache |
| `api/crates/cma/src/store/contention.rs` (move from `agent_conversation_store/contention.rs`) | shared `backoff`/`exhausted` for every budget transaction loop |
| `api/crates/cma/src/store/platform_write_coordination.rs` (create) | `PlatformWrites` process-wide scheduling mutex |
| `api/crates/cma/src/store/agent_conversation_store.rs`, `agent_commands/reserve.rs` (modify) | take `PlatformWrites` around period-row-writing transactions |
| `api/crates/cma/src/store/budget/spend_ledger.rs` (create) | `impl PlatformSpendLedger for MongoSandboxStore` |
| `api/crates/cma/src/store/sandbox_admission.rs`, `workspace_admission.rs`, `agent_admission.rs` (modify) | zero-liability commitment check |
| `api/crates/cma/src/store/trigger_definitions/launches.rs`, `launches/admission.rs` (modify) | Launch commitment check |
| `api/crates/layer2-core/src/sandbox/lifecycle_service.rs`, `launch_service.rs` (modify) | revival pre-check |
| `api/crates/cma/src/auth/records.rs`, `auth/flow.rs` (modify) | registration refusal at the sign-in callback and the sign-up entry |
| `api/crates/cma/src/sandbox/reconciliation_sweep.rs`, `reconciliation.rs`, `reconciliation_telemetry.rs` (modify) | `Phase::PlatformSpend`, pause cause |
| `api/crates/layer2-core/src/sandbox/store/maintenance.rs` (modify) | `InactivityPauseCause`, platform notice |
| `api/crates/layer1-public/src/rest/instance_problem.rs` and the five other match sites; `rest/instances_problem.rs`, `rest/trigger_problem.rs` (modify) | 503 for `PlatformExhausted` |
| `api/crates/layer1-public/src/rest/admin/spend.rs` (create), `admin.rs`, `admin/settings/budget.rs`, `admin/settings/body.rs` (modify) | admin routes and body shapes |
| `api/crates/cma/src/config/features.rs`, `config_tests.rs`, `composition.rs`; `api/crates/layer2-core/src/platform_service.rs`; `infra/api/configmap.yaml`; `docs/CMA_Operations.md` (modify) | third write gate |
| `api/crates/layer1-public/tests/fixtures/public-contract/admin.json`; `sdk/typescript/tests/admin-availability.test.ts` and the SDK admin settings type (modify) | availability field |

---

### Task 1: Period accounting types in Layer 2

**Files:**
- Modify: `api/crates/layer2-core/src/budget/platform.rs`
- Create: `api/crates/layer2-core/src/budget/period.rs`, `api/crates/layer2-core/src/budget/period_tests.rs`
- Modify: `api/crates/layer2-core/src/budget.rs` (`mod period; pub use period::*;`, new error variant)
- Modify: `api/crates/layer2-core/src/budget/rollout_tests.rs`

**Interfaces:**
- Produces:
  ```rust
  // platform.rs
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
  pub struct CloudSpendPolicy { pub run_rate_units_per_day: u64, pub max_observation_age_seconds: u64 }
  // on PlatformBudgetPolicy, additive:
  #[serde(default, skip_serializing_if = "Option::is_none")] pub cloud: Option<CloudSpendPolicy>,
  pub struct SpendLevels { pub warning: u64, pub stop: u64, pub disposable: u64 }
  impl PlatformBudgetPolicy {
      pub fn levels(&self) -> Result<SpendLevels, BudgetError>;
      pub fn period_bounds(&self) -> Result<(u64, u64), BudgetError>; // (start, end) seconds
  }
  // period.rs
  #[serde(rename_all = "snake_case")] pub enum ClosureReason { SpendWarning, SpendStop, MeteringUnavailable }
  pub struct Closure { pub reason: ClosureReason, pub since: u64, pub settings_revision: u64, pub committed_at_entry: u64 }
  pub struct CloudObservation { pub observed_through: u64, pub units: u64, pub recorded_at: u64, pub settings_revision: u64 }
  pub struct PlatformPeriodLedger {
      pub period_start: u64, pub period_end: u64, pub settings_revision: u64,
      pub token_units: Option<[u64; 2]>, pub spent: u64, pub reserved: u64, pub adjustments: u64,
      pub cloud: Option<CloudObservation>, pub closures: Vec<Closure>, pub evaluated_at: u64,
      pub revision: u64, // row CAS counter, incremented by every persisting writer
  }
  impl PlatformPeriodLedger {
      pub fn open(policy: &PlatformBudgetPolicy, settings_revision: u64) -> Result<Self, BudgetError>;
      pub fn cloud_committed(&self, policy: &PlatformBudgetPolicy, now: u64) -> Result<u64, BudgetError>; // Err(Unavailable) when cloud policy or observation missing/stale
      pub fn committed(&self, policy: &PlatformBudgetPolicy, now: u64) -> Result<u64, BudgetError>;
      pub fn admit(&mut self, policy: &PlatformBudgetPolicy, settings_revision: u64, token_units: [u64; 2], liability: u64, now: u64) -> Result<Vec<Closure>, Refusal>; // Ok: closures pushed in place (SpendWarning); Err: self untouched, closures to enter outside the transaction
      pub fn settle(&mut self, reserved: u64, debit: u64) -> Result<(), BudgetError>;
      pub fn release(&mut self, reserved: u64) -> Result<(), BudgetError>;
      pub fn retain(&mut self, extra: u64) -> Result<(), BudgetError>;
      pub fn evaluate(&mut self, policy: &PlatformBudgetPolicy, settings_revision: u64, now: u64) -> Result<Evaluation, BudgetError>;
      pub fn record_cloud(&mut self, observation: CloudObservation) -> Result<(), BudgetError>;
      pub fn closed_for_commitments(&self) -> bool; // SpendStop | MeteringUnavailable present
      pub fn closed_for_registration(&self) -> bool; // any closure present
  }
  pub struct Refusal { pub reason: BudgetError, pub closures: Vec<Closure> } // closures: what the caller enters outside the transaction (concurrency rule 1)
  pub struct Evaluation { pub entered: Vec<Closure>, pub lifted: Vec<ClosureReason> }
  impl Evaluation { pub fn changed(&self) -> bool; }
  // budget.rs
  pub enum BudgetError { OwnerExhausted, TurnExhausted, Unavailable, InvalidPolicy, PlatformExhausted }
  // code(): PlatformExhausted => "platform_budget_exhausted"; guidance(): spec §7 text verbatim
  ```
- Semantics the signatures do not pin:
  - `admit`: `Err(Refusal { reason: Unavailable, closures: [MeteringUnavailable if absent] })` on `token_units` mismatch, period mismatch with nonzero totals (spec §4.2), a `cloud_committed` error, or `now ≥ period_end` (rule 7); `Err(Refusal { reason: PlatformExhausted, closures: [] })` when `closed_for_commitments()`; `Err(Refusal { reason: PlatformExhausted, closures: [SpendStop, SpendWarning if absent] })` when `committed + L > disposable`, `since = now`. A refusal leaves `self` untouched (rule 1). On success set `reserved += L`, push `SpendWarning` in place when `committed + L ≥ warning` and absent, and return what was pushed. Entering is monotonic; `admit` never removes closures.
  - `evaluate`: `MeteringUnavailable` enters when `cloud_committed` errs or `now ≥ period_end`, and leaves when neither holds (any revision ≥ its own); `SpendWarning`/`SpendStop` enter by projection; they leave only when `settings_revision > closure.settings_revision` and the level is no longer reached. Updates `settings_revision` and `evaluated_at` in memory; the caller persists only when `changed()` or per rule 4.
  - `cloud_committed`: `observed.units + ceil((now − observed_through) × run_rate / 86_400)`; stale when `now − observed_through > max_observation_age_seconds`.
  - `CloudSpendPolicy::validate`: both fields `> 0` and `≤ (1 << 53) − 1`; `PlatformBudgetPolicy::validate` calls it when present.

- [ ] **Step 1: Write failing tests in `period_tests.rs`**

```rust
fn policy() -> PlatformBudgetPolicy // cap 1_000_000, 80/100, reserve 50_000, lag 50_000, period [1_700_000_000, 1_702_592_000), cloud {run_rate 86_400, max_age 3_600}
#[test] fn levels_follow_the_spec_formula()            // warning 800_000, stop 1_000_000, disposable 900_000
#[test] fn cloud_projection_rounds_up_and_goes_stale() // obs(units 10, through t0) → committed at t0+1 == 11; at t0+3_601 → Err(Unavailable)
#[test] fn admission_mirrors_reserved_and_enters_warning_once() // two admits crossing 800_000 → one SpendWarning
#[test] fn refusal_at_disposable_returns_the_stop_closure_without_mutating_the_ledger() // admit that would exceed → Err(Refusal{reason: PlatformExhausted, closures: [SpendStop, SpendWarning]}); self.closures still empty, reserved unchanged
#[test] fn a_finished_period_is_metering_unavailable() // now ≥ period_end → admit Err(Refusal{Unavailable, [MeteringUnavailable]}); evaluate enters MeteringUnavailable
#[test] fn evaluation_reports_whether_anything_changed() // same inputs twice → second Evaluation.changed() == false
#[test] fn closures_are_monotonic_within_a_revision_and_lift_only_under_a_newer_one()
#[test] fn metering_unavailable_lifts_on_a_fresh_observation_without_a_new_revision()
#[test] fn token_units_and_period_changes_fail_closed_under_liability()
#[test] fn settle_release_and_retain_keep_spent_plus_reserved_consistent()
#[test] fn missing_cloud_policy_is_unavailable_not_zero()
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p layer2-core budget::period_tests`
Expected: compile error (types missing)

- [ ] **Step 3: Implement `CloudSpendPolicy`, `SpendLevels`, `levels()`, `period_bounds()` in `platform.rs`; `period.rs` per the interface; `BudgetError::PlatformExhausted` with code and guidance in `budget.rs`**

Fix every exhaustive `match` on `BudgetError` the compiler reports (`CommandFailureCode::from_budget` / `budget_error` in `layer2-core/src/sandbox/agent_io/work.rs` get a `PlatformBudgetExhausted` variant; see Task 3 for the wire side).

- [ ] **Step 4: Add the rollout test**

In `rollout_tests.rs`: `retaining_cloud_fields_requires_the_platform_spend_reader()` — a `PlatformBudgetPolicy` with `cloud: Some(..)` serializes with the key, deserializes with the current reader, and fails `PreBudgetSettings`; with `cloud: None` the serialized JSON has no `cloud` key.

- [ ] **Step 5: Run the crate tests**

Run: `cargo nextest run --locked -p layer2-core` and `cargo clippy --locked -p layer2-core --all-targets --all-features -- -D warnings`
Expected: PASS, zero warnings

- [ ] **Step 6: Commit**

```bash
git add api/crates/layer2-core
git commit -m "Add platform period ledger types and the platform exhaustion reason"
```

---

### Task 2: Period row in the reservation transactions

**Files:**
- Create: `api/crates/cma/src/store/budget/period.rs`
- Modify: `api/crates/cma/src/store/budget/mod.rs` (`reserve`, `start_turn`, `apply`, `policy`), `requests/admission.rs`, `requests/settlement.rs`, `cancellation.rs`, `requests.rs` (`RequestRecord`)
- Test: `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/platform_period_tests.rs` (create; register in `command_ledger_tests.rs`)
- Modify: `command_ledger_tests/budget_tests.rs::configure` — add `configure_with_platform(store, owner, platform: PlatformBudgetPolicy, revision: u64)`

**Interfaces:**
- Consumes: Task 1 types.
- Produces (crate-private, `cma/src/store/budget/period.rs`):
  ```rust
  pub(super) struct PolicySnapshot { pub owner: Option<BudgetPolicy>, pub platform: Option<PlatformBudgetPolicy>, pub revision: u64 }
  pub(super) async fn fenced_snapshot(store: &MongoSandboxStore, owner: &SubjectId, session: &mut ClientSession) -> Result<PolicySnapshot, ApplyError>; // the existing fence $inc + settings read (today's `policy()` body); liability-bearing reservations only; `policy()` becomes a wrapper returning `.owner`
  pub(super) async fn read_snapshot(store, owner: Option<&SubjectId>, session) -> Result<PolicySnapshot, ApplyError>; // plain find_one of settings inside the session snapshot, no write (rule 2)
  pub(super) fn period_key(period_start: u64) -> String; // "platform:<RFC3339>"
  pub(super) async fn load_or_open(store, platform: &PlatformBudgetPolicy, revision: u64, session) -> Result<PlatformPeriodLedger, ApplyError>; // find_one; when absent, insert_one — a duplicate key is returned as the transient retry (rule 6)
  pub(super) async fn admit_liability(store, snapshot: &PolicySnapshot, token_units: [u64; 2], liability: u64, now: u64, session) -> Result<Option<u64>, ApplyError>; // Ok(None) when platform policy absent/disabled; Ok(Some(period_start)) after a saved admit; on `Refusal` call `enter_closures(store, period_start, refusal.closures)` WITHOUT the session, then return Err(domain(refusal.reason)) — the caller's loop aborts the transaction; the closure survives (rule 1)
  pub(super) async fn enter_closures(store: &MongoSandboxStore, period_start: u64, closures: &[Closure]) -> Result<Vec<ClosureReason>, BudgetError>; // per closure: update_one({_id: key, "record.closures.reason": {$ne: reason}}, {$push: {"record.closures": closure}, $inc: {"record.revision": 1}}) on the raw collection, no session; returns the reasons whose update had modified_count == 1 — the caller that actually inserted a closure is the only one that alerts
  pub(super) async fn ensure_period_row(store, platform: &PlatformBudgetPolicy, revision: u64) -> Result<(), BudgetError>; // $setOnInsert upsert outside any transaction; used by the evaluator (Task 6)
  pub(super) async fn mirror_settle(store, period_start: Option<u64>, reserved: u64, debit: u64, session) -> Result<(), ApplyError>;
  pub(super) async fn mirror_release(store, period_start: Option<u64>, reserved: u64, session) -> Result<(), ApplyError>;
  pub(super) async fn mirror_retain(store, period_start: Option<u64>, extra: u64, session) -> Result<(), ApplyError>;
  pub(super) async fn require_open(store, now: u64, session) -> Result<(), ApplyError>; // zero-liability commitments (Task 4): read_snapshot → if platform present: load_or_open (read), ledger.admit(.., liability 0, ..) on a clone; on Refusal: enter_closures without the session, then Err(domain(reason)); never writes the settings fence (rule 2)
  pub(super) async fn save_period(store, ledger: &PlatformPeriodLedger, session) -> Result<(), ApplyError>; // replace_one filtered on `revision`, writing revision + 1; matched_count == 0 is returned as the transient retry so the WHOLE transaction aborts and reruns from a fresh snapshot
  // platform_write_coordination.rs
  pub(super) struct PlatformWrites(tokio::sync::Mutex<()>); impl PlatformWrites { pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()>; } // one per MongoSandboxStore, field `platform_writes` beside `agent_writes` (rule 5)
  // source.rs
  struct PlatformStopCache { period_key: String, closed: bool, platform_enabled: bool, read_at: Instant } // std::sync::Mutex<Option<..>> on MongoSandboxStore; TTL 5 s (rule 3)
  ```
- Scheduling (rule 5): `commit_conversation` takes `store.platform_writes.lock()` after `agent_writes.lock(..)` and before `start_session` **only when** the cache hint says `platform_enabled` and `mutation.agent_logs` contains `TurnStarted` or a terminal turn log (those are the mutations that write the period row; a `retain` write without the lock is still correct, merely unscheduled). The command reservation loop in `agent_commands/reserve.rs` and `requests.rs::request_transaction` take it unconditionally when `platform_enabled`. `request_transaction` replaces its `sleep(5 ms × attempt)` with `contention::backoff(attempt)` after the module moves to `store/contention.rs`.
- `TurnRecord` and `RequestRecord` gain `#[serde(default, skip_serializing_if = "Option::is_none")] period_start: Option<u64>`.
- Wiring (spec §4.1 table): `reserve()` → `fenced_snapshot` then `admit_liability(.., policy.turn_limit, ..)` after `owner.reserve`, store `period_start` on the `TurnRecord`; `start_turn()` → when `live_policy` is `Some`, also `require_open` (a stop refuses starting a queued turn); `apply()` settlement branch → `mirror_retain` for the `retain_observed_liability` delta and `mirror_settle(record.period_start, reservation.reserved_units, debit)` where `debit` is the same value `OwnerBudget::settle` uses; `requests/admission.rs` → `admit_liability` after `owner.reserve_liability`, record `period_start`; `requests/settlement.rs` → `mirror_settle(record.period_start, record.reserved, actual)`; `cancellation.rs` → `mirror_release(record.period_start, reserved_units)`.
- A `Some(period_start)` on a record always names the row it was charged to; settlement after `period_end` debits that row (spec §4.1).

- [ ] **Step 1: Write the failing Mongo tests** (`#[tokio::test] #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]`, using `database`, `fixture`, `configure_with_platform`)

```rust
async fn turn_reservation_mirrors_into_the_period_row_in_the_same_transaction() // reserve_command → period reserved == turn_limit; settle via apply TurnEnded → reserved 0, spent == debit
async fn request_hold_mirrors_and_settles_against_the_period_row()
async fn proven_non_dispatch_releases_period_liability() // release_unexecuted
async fn admission_refuses_with_platform_exhausted_and_records_the_closure() // disposable sized to admit exactly one turn; second reserve_command → SandboxStoreError::Budget(PlatformExhausted); row has SpendStop; reserved unchanged
async fn a_refused_admission_leaves_one_spend_stop_visible_after_its_transaction_aborted() // refuse via reserve_command; read the row from a second MongoSandboxStore::open → exactly one SpendStop, reserved unchanged (rule 1)
async fn two_stores_crossing_the_stop_level_record_one_closure() // two MongoSandboxStore::open, concurrent reserve_command on two owners; exactly one admitted; closures.len() == 2 (warning + stop) with one SpendStop
async fn twenty_concurrent_admissions_on_one_store_exhaust_no_retry_budget() // 20 owners, tokio::join on reserve_command under a cap that fits all; every call Ok; zero `Conflict`/`Unavailable`; no "contention exhausted" warning captured (rule 5)
async fn zero_liability_admission_never_writes_the_settings_fence() // require_open under an open platform → platform_settings.model_budget_admission_fence unchanged (rule 2)
async fn missing_cloud_policy_refuses_with_budget_unavailable_and_metering_closure()
async fn a_turn_reserved_before_activation_settles_without_touching_the_period_row() // reserve with platform None, then save platform policy, then settle → row absent or untouched
async fn lowering_the_cap_below_committed_refuses_the_next_admission_without_releasing()
async fn settlement_after_period_end_debits_the_original_row()
async fn a_lost_runtime_mid_turn_keeps_period_reserved_until_terminal_evidence() // reserve, start the turn, replace the runtime generation with no TurnEnded → period `reserved` unchanged; later terminal evidence settles the full allowance (spec §11 item 5)
async fn disabled_or_absent_platform_policy_changes_nothing() // no `platform:` document is ever created
```

- [ ] **Step 2: Run to verify they fail**

Run: `CHRONO_TEST_MONGODB_URI=<isolated replica set> cargo nextest run --locked -p cma --run-ignored all --test-threads 1 -E 'test(platform_period_tests::)'`
Expected: FAIL (helpers missing / assertions on absent row)

- [ ] **Step 3: Implement `cma/src/store/budget/period.rs` and wire the six call sites**

Approach notes: `save_period` filters on `revision` and writes `revision + 1`; `matched_count == 0` is surfaced through the callers' existing transient-retry branch so the whole transaction aborts and reruns — never re-issue only the period write. Move `agent_conversation_store/contention.rs` to `store/contention.rs` (`pub(super) → pub(in crate::store)`) before touching `requests.rs`. Compute `now` in `apply()` from `mutation.appended_at` as the owner path already does.

- [ ] **Step 4: Run the new tests and the existing budget suites**

Run: same command with `-E 'test(platform_period_tests::) | test(budget_tests::) | test(budget_execution_tests::) | test(request_budget_tests::)'`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add api/crates/cma/src/store/contention.rs api/crates/cma/src/store/agent_conversation_store.rs api/crates/cma/src/store/agent_conversation_store api/crates/cma/src/store/budget/requests.rs
git commit -m "Share the conversation transaction backoff with budget request transactions"
git add api/crates/cma/src/store/platform_write_coordination.rs api/crates/cma/src/store/sandbox_mongo.rs api/crates/cma/src/store/agent_commands/reserve.rs api/crates/cma/src/store/agent_conversation_store.rs api/crates/cma/src/store/budget api/crates/cma/src/store/sandbox_mongo_integration_tests
git commit -m "Mirror owner reservations into a platform period row and refuse over the disposable cap"
```

---

### Task 3: Stop path and 503 mapping

**Files:**
- Modify: `api/crates/cma/src/store/budget/source.rs` (`current_stop`)
- Modify: `api/crates/layer2-core/src/sandbox/agent_io/work.rs` (`CommandFailureCode::PlatformBudgetExhausted`, `budget_error`, `from_budget`)
- Modify: `api/crates/layer1-public/src/rest/instance_problem.rs:173`, `rest/agent_command_admission.rs:213`, `conversation/commands/errors.rs:40,76-87`, `responses/v2/error.rs:168-173`, `responses/v2/projection.rs:101-106,237-241`, `responses/v2/controls/shapes.rs:120,255-257`
- Test: `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/budget_execution_tests.rs` (add), `api/crates/layer1-public/src/conversation/commands_tests/refusals.rs` (add), `api/crates/layer2-core/src/sandbox/agent_operation_tests/budget_recovery_tests.rs` (add)

**Interfaces:**
- Consumes: Task 2 `period_key` and the period row shape; plain reads only (no `fenced_snapshot`, no `load_or_open`).
- Produces: `fn budget_status(reason: BudgetError) -> StatusCode` in `instance_problem.rs` (`PlatformExhausted => SERVICE_UNAVAILABLE`, else `PAYMENT_REQUIRED`), used by all six sites; `CommandFailureCode::PlatformBudgetExhausted` ↔ `BudgetError::PlatformExhausted`; wire code `platform_budget_exhausted` in `failure_code`; legacy collapse to `ExecutionFailed` in `controls/shapes.rs`.

- [ ] **Step 1: Write failing tests**

```rust
// budget_execution_tests.rs
async fn platform_stop_requests_a_guarded_cancel_for_every_active_turn() // save SpendStop on the row → stop_requested returns Some(BudgetStop{reason: PlatformExhausted}) bound to the active turn; stop row "stop:<agent>:<turn>" persisted; a second call returns the same reason
async fn staleness_alone_does_not_stop_a_running_turn() // MeteringUnavailable only → stop_requested None
async fn stop_reads_are_served_from_the_cache_within_its_ttl() // insert SpendStop after a cached open read → None until the TTL passes, then Some; the platform_settings fence counter never changes (rule 3)
// refusals.rs
fn platform_exhaustion_is_a_503_problem_with_the_spec_guidance()
// budget_recovery_tests.rs
fn terminal_evidence_after_a_platform_stop_fails_the_turn_with_platform_budget_exhausted()
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run --locked -p layer1-public -p layer2-core -E 'test(platform_exhaustion) | test(platform_stop)'` and the Mongo selector for `budget_execution_tests::platform_`
Expected: FAIL

- [ ] **Step 3: Implement**

`current_stop`: before the per-turn checks, consult `PlatformStopCache` (rule 3). On a miss or after 5 s: plain `find_one` of `platform_settings` (already done by `current_budget_policy`) and a plain `find_one` of the period row, **no session, no fence**; refresh the cache with `closed = closures contain SpendStop` and `platform_enabled`. When `closed` → `Ok(Some(BudgetError::PlatformExhausted))`. The existing first-writer `stop:` row semantics are unchanged. The same cache supplies the `platform_enabled` hint that decides whether `PlatformWrites` is taken (Task 2).

- [ ] **Step 4: Run tests**

Expected: PASS; `cargo clippy --locked -p layer1-public -p layer2-core -p cma --all-targets --all-features -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git commit -am "Stop active turns under a platform spend closure and map the reason to HTTP 503"
```

---

### Task 4: Zero-liability commitments

**Files:**
- Modify: `api/crates/cma/src/store/sandbox_admission.rs` (private `apply`, after the live-limit check at L119), `workspace_admission.rs::apply_admission` (L193), `agent_admission.rs::apply_admission` (L187)
- Modify: `api/crates/cma/src/store/trigger_definitions/launches.rs` (`LaunchCollections` at L38 gains `model_budget: Collection<Document>`; its existing `settings` field is the `platform_settings` handle), `launches/admission.rs::admit_once` (after `visitor_quota::admit`)
- Create: `api/crates/cma/src/store/budget/spend_ledger.rs` — `impl PlatformSpendLedger for MongoSandboxStore`
- Create: `api/crates/layer2-core/src/budget/spend.rs` (trait + state types; the service is Task 7)
- Modify: `api/crates/layer2-core/src/sandbox/lifecycle_service.rs::revive` (L159), `::revive_for_command` (L58); `launch_service.rs::revive_for_launch` (L29); their service structs gain `spend: Option<Arc<dyn PlatformSpendLedger>>` set from `cma/src/sandbox/composition.rs`
- Modify: `api/crates/layer2-core/src/trigger_domain/launch_store.rs` (`LaunchStoreError::Budget(BudgetError)`), `launch_service.rs` (`LaunchServiceError::Budget(BudgetError)`) and `api/crates/layer1-public/src/rest/trigger_problem.rs` (L152–165 arms: `Budget(reason)` → status from `budget_status(reason)`, code `reason.code()`, detail `reason.guidance()`); `api/crates/layer1-public/src/rest/instances_problem.rs` (`HierarchyProblem`, L9/L33) maps `SandboxStoreError::Budget(reason)` for Sandbox/Workspace/Agent creation through `InstanceProblem::Budget(reason)`
- Test: `api/crates/cma/src/store/sandbox_mongo_integration_tests/platform_commitment_tests.rs` (create), `api/crates/layer2-core/src/sandbox/lifecycle_service_tests.rs` (add)

**Interfaces:**
- Produces (`layer2-core/src/budget/spend.rs`):
  ```rust
  pub struct PlatformAdmissionState { pub configured: bool, pub closures: Vec<Closure> }
  impl PlatformAdmissionState { pub fn closed_for_commitments(&self) -> bool; pub fn closed_for_registration(&self) -> bool; }
  pub struct PlatformSpendState { pub settings_revision: u64, pub levels: Option<SpendLevels>, pub committed: Option<u64>, pub ledger: Option<PlatformPeriodLedger> }
  pub struct CloudObservationInput { pub observed_through: u64, pub units: u64 }
  #[async_trait] pub trait PlatformSpendLedger: Send + Sync {
      async fn admission_state(&self, now: u64) -> Result<PlatformAdmissionState, BudgetError>; // read-only, no transaction
      async fn spend_state(&self, now: u64) -> Result<PlatformSpendState, BudgetError>;
      async fn record_cloud_observation(&self, input: CloudObservationInput, now: u64) -> Result<PlatformSpendState, BudgetError>; // Task 7 uses it
  }
  ```
- Launch: `require_open` variant that takes the launch store's collections (`platform_settings`, `model_budget`) — implement as `pub(crate) async fn require_open_with(settings: &Collection<Document>, ledger: &Collection<Document>, now, session)` in `cma/src/store/budget/period.rs` and have the `MongoSandboxStore` version call it. Both are read-only inside the session (rule 2); a refusal's closures go through `enter_closures` on the raw `ledger` collection.
- Revival: `revive*` call `spend.admission_state(now)` before scheduling; `closed_for_commitments()` → `SandboxLifecycleCommandError`/`SandboxServiceError` carrying `BudgetError::PlatformExhausted` (reuse the existing `Budget`-shaped variants where present; add one where not). Service-level read, not in-transaction — decision recorded in spec §12 addendum (Task 8).

- [ ] **Step 1: Write failing tests**

```rust
// platform_commitment_tests.rs (Mongo)
async fn create_sandbox_is_refused_under_spend_stop_and_admitted_when_open()
async fn workspace_and_agent_admission_are_refused_under_spend_stop()
async fn launch_admission_is_refused_under_spend_stop_after_visitor_quota()
async fn metering_unavailable_refuses_commitments_with_budget_unavailable()
async fn no_platform_policy_means_no_period_row_and_no_refusal()
// lifecycle tests
async fn revive_is_refused_while_the_platform_is_closed_for_commitments()
async fn cancel_pause_and_approval_decisions_stay_available_under_spend_stop() // existing control paths untouched
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement the four in-transaction checks, the trait impl, the revival pre-checks and the Layer 1 mappings**
- [ ] **Step 4: Run tests + clippy** — Expected: PASS
- [ ] **Step 5: Commit**

```bash
git commit -am "Refuse new Sandbox, Workspace, Agent, Launch and revival commitments under a platform spend closure"
```

---

### Task 5: Registration closure

**Files:**
- Modify: `api/crates/cma/src/auth/records.rs` (new `admit_registration`, `registration_open`), `api/crates/cma/src/auth/flow.rs` — `callback` (call before persisting the signed-in subject, L~330) and `begin_login_with_intent` (L133; the `LoginScreen::Register` branch at L186 redirects to the identity provider's `/register`)
- Modify: `api/crates/cma/src/auth/mod.rs` (`AuthState.records` or a sibling field gains `spend: Option<Arc<dyn PlatformSpendLedger>>`), `api/crates/cma/src/composition.rs` (wire)
- Test: `api/crates/cma/src/auth/records_tests.rs` (create) with the pure decision; flow tests in the existing `auth/flow_registration_tests.rs`, which already covers the register screen and unseen-identity callbacks

**Interfaces:**
- Produces:
  ```rust
  pub(super) enum RegistrationDecision { Admitted, Existing, Closed }
  pub(super) fn registration_decision(state: &PlatformAdmissionState, existing: bool) -> RegistrationDecision; // pure
  impl AuthState {
      pub(super) async fn admit_registration(&self, subject: &SubjectId) -> Result<RegistrationDecision, StoreError>; // authority: records.users.user(subject) + spend.admission_state(now); no spend ledger configured → Admitted; read errors propagate (fail closed)
      pub(super) async fn registration_open(&self) -> bool; // advisory, for the sign-up entry: !closed_for_registration(); no ledger or a read error → true (the callback decides)
  }
  // flow.rs
  fn registration_closed(clear_binding: Option<(&str, bool)>) -> Response; // 503 {"title": "Sign-up is temporarily closed", "detail": <spec §7 registration_closed text>, "code": "registration_closed"}, no login_url; appends the cleared binding cookie when given, as login_failure does
  ```
- Do **not** reuse `login_failure` for this refusal: its third argument is the browser-binding cookie to clear, not a message, and its fixed detail tells the user to start a new sign-in attempt, which is wrong when sign-ups are closed.
- `callback`: `Closed` → `registration_closed(Some((&cookie_name, auth.secure_cookies())))` and return before persisting; a `StoreError` from the check → `store_failure()` (fail closed, same as other persistence errors). `note_sign_in` is unchanged.
- `begin_login_with_intent`: when `screen == Some(LoginScreen::Register)` and `!auth.registration_open().await` → `registration_closed(None)` before an authorize attempt is created (spec §5.1). Plain login (`screen == None`) is never checked: existing owners must still sign in.

- [ ] **Step 1: Write failing tests**

```rust
fn registration_is_closed_for_new_subjects_only_while_a_closure_exists() // (Open, new) Admitted; (SpendWarning, new) Closed; (SpendWarning, existing) Existing; (SpendStop, new) Closed; (unconfigured, new) Admitted
async fn callback_refuses_a_never_seen_subject_with_registration_closed_and_persists_nothing() // body code "registration_closed", no login_url, binding cookie cleared, no directory row, no signed-in subject
async fn callback_admits_an_existing_subject_under_spend_warning()
async fn register_screen_is_refused_while_a_closure_exists_and_plain_login_is_not() // SpendWarning: /auth/login?screen=register → 503 registration_closed, no attempt persisted; /auth/login → redirect as today
async fn register_screen_proceeds_when_the_admission_state_is_unreadable() // ledger read error → redirect to /register as today
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run `cargo nextest run --locked -p cma -E 'test(auth::)'`** — Expected: PASS
- [ ] **Step 5: Commit**

```bash
git commit -am "Refuse first sign-ins and the sign-up entry while a platform spend closure is active"
```

---

### Task 6: Sweep phase: evaluation, alerts and pause

**Files:**
- Modify: `api/crates/cma/src/sandbox/reconciliation_telemetry.rs` (`Phase::PlatformSpend` before `IdlePause`), `reconciliation_sweep.rs` (new phase arm; `IdlePause` arm reads `self.platform_stop`), `reconciliation.rs` (`evaluate_platform_spend(now) -> Result<bool, _>` on the reconciler)
- Modify: `api/crates/layer2-core/src/sandbox/store/maintenance.rs` (`SandboxInactivityPauseRequest.cause: InactivityPauseCause`, `apply_inactivity_pause` notice by cause)
- Modify: `api/crates/cma/src/store/budget/spend_ledger.rs` (add `evaluate(now) -> Result<Option<Evaluation>, BudgetError>` as an inherent method on `MongoSandboxStore`; `None` when no platform policy is enabled. No transaction, no fence: plain settings read → `ensure_period_row` (rule 6) → plain `find_one` of the row → `ledger.evaluate` on the copy → persist only when `evaluation.changed()` or `now − evaluated_at ≥ 60` (rule 4): entered closures via `enter_closures`; lifted closures, `settings_revision` and `evaluated_at` via one `update_one` filtered on `revision` with `$pull` by reason, `$set`, `$inc revision`; `matched_count == 0` → return the evaluation without retrying. The returned `entered` is narrowed to what `enter_closures` actually inserted.)
- Test: `api/crates/cma/src/sandbox/reconciliation_sweep_tests.rs` (existing module; add), `api/crates/layer2-core/src/sandbox/store/maintenance_tests.rs` (create; register from `maintenance.rs` with `#[cfg(test)] #[path = "maintenance_tests.rs"] mod tests;` as `quiet_pause.rs:86` does), Mongo `platform_period_tests.rs` (add)

**Interfaces:**
- Produces:
  ```rust
  // maintenance.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub enum InactivityPauseCause { #[default] Inactivity, PlatformSpendStop }
  // notice for PlatformSpendStop is the spec §5.2 text verbatim; `matching_pause` compares against the notice for the request's cause
  // reconciliation.rs
  pub(super) async fn evaluate_platform_spend(&self, now: SystemTime) -> Result<bool /* closed_for_commitments */, ReconcileError>;
  ```
- Sweep: `Phase::PlatformSpend` runs once per sweep: `evaluate_platform_spend` → for each newly entered closure `tracing::warn!(parent: None, event = "platform_spend_closure", reason = ?closure.reason, settings_revision = closure.settings_revision, "platform spend closure entered")`; sets `self.platform_stop`; then `Phase::IdlePause`. `IdlePause`: when `self.platform_stop`, use `pause_after_seconds: 0, cause: PlatformSpendStop, auto_pause_enabled: true` regardless of `automatic_pause_threshold`; otherwise unchanged. Because every replica sweeps, the alert fires only from the caller whose guarded update actually inserted the closure (`enter_closures` returns an empty list to the others). The sweep never opens a transaction or touches the settings fence for this phase.
- Spec §5.3 single-fire: tested via two reconcilers against one database.

- [ ] **Step 1: Write failing tests**

```rust
// maintenance_tests.rs
fn platform_spend_pause_ignores_the_inactivity_threshold_and_uses_the_platform_notice()
fn platform_spend_pause_is_not_eligible_with_an_active_turn_or_queued_messages()
// platform_period_tests.rs (Mongo)
async fn evaluation_enters_metering_unavailable_when_the_observation_ages_out_and_lifts_on_a_fresh_one()
async fn evaluation_lifts_spend_stop_only_under_a_newer_settings_revision()
async fn two_evaluators_report_one_newly_entered_closure()
async fn the_evaluator_does_not_write_when_nothing_changed() // two evaluate() calls within 60 s, unchanged inputs → row `revision` unchanged after the second (rule 4)
async fn the_evaluator_pre_creates_the_period_row_outside_admission() // fresh activation → one sweep → row exists with spent 0 / reserved 0 before any admission (rule 6)
async fn after_a_lift_an_explicit_new_message_is_admitted_and_nothing_replays() // SpendStop → save a higher cap (new revision) → evaluate lifts → reserve_command for a NEW command is admitted; the refused command's receipt stays failed and is not dispatched (TC-126 step 2)
// reconciliation_sweep_tests.rs
async fn the_platform_spend_phase_drives_a_pause_of_quiet_sandboxes_and_leaves_busy_ones()
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests + clippy** — Expected: PASS
- [ ] **Step 5: Commit**

```bash
git commit -am "Evaluate platform spend closures in the reconciliation sweep and pause quiet Sandboxes under a stop"
```

---

### Task 7: Operator surface and the third write gate

**Files:**
- Modify: `api/crates/cma/src/config/features.rs` (`platform_spend_writes_enabled: bool` ← `"CHRONO_PLATFORM_SPEND_WRITES_ENABLED"`), `config_tests.rs` (extend the key loop at L398–420), `composition.rs` (`.with_platform_spend_writes_enabled(..)`; build `PlatformSpendService`; `AdminState` gains it)
- Modify: `api/crates/layer2-core/src/platform_service.rs` (`with_platform_spend_writes_enabled`, `platform_spend_writes_enabled()` = foundation && platform; in `configure_platform` after the request-models check: `if !self.platform_spend_writes_enabled() && settings.model_budget.platform.as_ref().is_some_and(|p| p.cloud.is_some()) { return Err(CoreError::ModelBudgetWritesUnavailable) }`)
- Create: `api/crates/layer2-core/src/budget/spend_service.rs` — `PlatformSpendService`
- Create: `api/crates/layer1-public/src/rest/admin/spend.rs`; modify `rest/admin.rs::router` (`.merge(spend::routes())`), `admin/settings/budget.rs` (`CloudSpendPolicyBody`, `PlatformBudgetPolicyBody.cloud: Option<..>`, conversions), `admin/settings/body.rs` (`SettingsView.platform_spend_writes_available: bool`)
- Modify: `infra/api/configmap.yaml` (after L98), `docs/CMA_Operations.md` key table (row after `CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED`)
- Modify: `api/crates/layer1-public/tests/fixtures/public-contract/admin.json` (add `"platform_spend_writes_available": false` beside the two existing availability fields at L63–64, 137–138, 276–277, 347–348, 483–484, 549–550), `sdk/typescript/tests/admin-availability.test.ts` and the SDK admin settings type (optional `platform_spend_writes_available?: boolean`)
- Test: `api/crates/layer1-public/src/rest/admin/platform_spend_tests.rs` (create), `api/crates/layer2-core/src/platform_service/budget_rollout_tests.rs` (add), `api/crates/layer2-core/src/budget/spend_service_tests.rs` (create)

**Interfaces:**
- Produces:
  ```rust
  // layer2-core/src/budget/spend_service.rs
  pub struct PlatformSpendService { ledger: Arc<dyn PlatformSpendLedger>, activity: Arc<ActivityService>, writes_enabled: bool }
  impl PlatformSpendService {
      pub fn new(ledger: Arc<dyn PlatformSpendLedger>, activity: Arc<ActivityService>) -> Self;
      pub fn with_writes_enabled(self, enabled: bool) -> Self;
      pub async fn read(&self, caller: &Caller) -> Result<PlatformSpendState, CoreError>;            // caller.require(Capability::ConfigurePlatform)
      pub async fn record_cloud_observation(&self, caller: &Caller, input: CloudObservationInput) -> Result<PlatformSpendState, CoreError>; // ConfigurePlatform; !writes_enabled → ModelBudgetWritesUnavailable; audit ActivityAction::Configure on ActivityTarget::Resource("platform_spend") with DetailKey::Revision = settings_revision; no units in the record
  }
  ```
- REST (`rest/admin/spend.rs`): `GET /api/v1/admin/spend` → `200 { settings_revision, levels?, committed?, ledger? }` as JSON of `PlatformSpendState` (the body type lives in Layer 1); `POST /api/v1/admin/spend/observations` body `{ "source": "cloud", "observed_through": "<RFC3339>", "units": <integer> }` → `200` same body; `422 invalid_request` for unknown `source`, `observed_through` > now or < `period_start`, non-integer or unsafe `units`; `409 settings_conflict` is not used (no `expected_revision` on observations; the ledger row CAS retries internally); `503 model_budget_writes_unavailable` when the gate is closed; `403 not-an-operator`.
- Idempotency (spec §8): an identical repeat (same `observed_through`, same `units`) returns 200; a different `units` for the same `observed_through` → `422 invalid_request` with detail `observation conflicts with a recorded value`. Implement in `record_cloud_observation` of the Mongo ledger by storing `observation:cloud:<observed_through>` rows in `model_budget_reservations` and comparing.
- configmap lines to add:
  ```yaml
  # Cloud spend policy fields and platform observations need a later reader baseline
  # even if both model budget write gates are on.
  CHRONO_PLATFORM_SPEND_WRITES_ENABLED: "false"
  ```
- `docs/CMA_Operations.md` row: `` `CHRONO_PLATFORM_SPEND_WRITES_ENABLED` `` | Strict boolean, false by default in code and ConfigMap. Reader-before-write gate for the `cloud` platform policy fields and `POST /api/v1/admin/spend/observations`; both model budget write gates must also be enabled. Exposed as read-only `platform_spend_writes_available`; does not activate enforcement or supply values. See CMA_Platform_Spend_Metering.md |

- [ ] **Step 1: Write failing tests**

```rust
// config_tests.rs: extend the key loop with "CHRONO_PLATFORM_SPEND_WRITES_ENABLED" and assert features.platform_spend_writes_enabled
// budget_rollout_tests.rs
async fn saving_cloud_fields_is_refused_until_the_platform_spend_gate_opens()
// spend_service_tests.rs
async fn observations_require_configure_platform_and_the_write_gate()
async fn observation_audit_records_revision_but_no_units()
// platform_spend_tests.rs (layer1)
async fn get_spend_returns_state_for_operators_and_403_for_others()
async fn post_observation_validates_time_bounds_and_is_idempotent()
async fn settings_view_reports_platform_spend_writes_available()
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement gate, service, routes, bodies, configmap, docs row, fixtures and SDK field**
- [ ] **Step 4: Verify**

Run: `cargo nextest run --locked -p layer1-public -p layer2-core -p cma -E 'test(platform_spend) | test(budget_rollout) | test(config_tests)'`; `cargo test --doc -p layer1-public`; `bash .github/scripts/check-config-keys.sh`; `node .github/scripts/check-config-docs.mjs`; in `sdk/typescript`: `npm ci && npm run lint && npm run typecheck && npm run format:check && npm test` (the SDK gates #1621 ran; `npm ci`, never `npm install`).
Expected: all PASS

- [ ] **Step 5: Commit** (two commits)

```bash
git add api/crates/cma/src/config api/crates/layer2-core/src/platform_service.rs api/crates/layer1-public/src/rest/admin/settings infra/api/configmap.yaml docs/CMA_Operations.md api/crates/layer1-public/tests/fixtures sdk/typescript
git commit -m "Gate cloud spend policy fields behind CHRONO_PLATFORM_SPEND_WRITES_ENABLED"
git add api/crates/layer2-core/src/budget api/crates/layer1-public/src/rest/admin api/crates/cma/src/composition.rs api/crates/cma/src/store/budget
git commit -m "Add admin platform spend state and cloud observation routes"
```

---

### Task 8: Spec addendum and whole-branch verification

**Files:**
- Modify: `docs/CMA_Platform_Spend_Metering.md` — the concurrency, registration, period-end and deferral revisions (§12 decisions 10–18) were applied before implementation on 2026-11-11. Change the status line to "implemented on `feat/platform-spend`, activation off"; for any section the implementation had to deviate from, update it and record the deviation as a new §12 decision.
- Modify: `docs/CMA_Model_Budget_Controls.md` — one sentence pointing platform metering to the new document.
- Modify: `docs/CMA_Sandbox_Usage.md` — one sentence after the Measurements table: allocation quantities are owner attribution, not the platform cap's cloud source; see `CMA_Platform_Spend_Metering.md` §2.2.

- [ ] **Step 1: Edit the three documents**
- [ ] **Step 2: Run the full backend gates**

```bash
cd api && cargo fmt --all --check && cargo clippy --locked --all-targets --all-features -- -D warnings && cargo nextest run --locked && cargo test --doc --locked && cargo deny check
```
Mongo-ignored tests: `CHRONO_TEST_MONGODB_URI=<isolated replica set> cargo nextest run --locked -p cma --run-ignored all --test-threads 1 -E 'test(platform_period_tests::) | test(platform_commitment_tests::) | test(budget_tests::) | test(budget_execution_tests::) | test(request_budget_tests::)'`
Expected: all PASS; no source file ≥ 500 lines (`git diff --stat develop... ; wc -l` on changed `.rs` files).

- [ ] **Step 3: Write the PR description in the issue's evidence format**

Every evidence item names: candidate image digest (or "source only — not yet built" for the PR stage), source SHA, environment, time window, sample size N, configuration and flags (all four write gates false; the three billing flags false), and where the raw results live (CI run URL, local log path). No amounts, owner identities or credentials. List the files actually changed (the issue's Affected Files are review pointers). Body says `Refs #1496` (Global Constraints).

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "Record platform spend implementation decisions in the contract"
```

- [ ] **Step 5: Hand off — not run in this plan**

- **11-14 staging drill (QA, SRE):** TC-125/TC-126 with synthetic units and an isolated owner, plus the concurrency case — 20 concurrent Launches with first messages, platform policy enabled with a cap that fits them all. Acceptance additions from this plan: zero `"contention exhausted"` warnings in the API log, zero `budget_unavailable` refusals, `platform_settings.model_budget_admission_fence` grows only by the number of liability-bearing admissions, the three billing flags still read false, zero unexpected 402.
- **`/measure` comments:** after the drill, post `/measure M-SUST-01 value=<…> status=<…> evidence=<link>` on #1546 and `/measure M-SUST-05 …` on #1548 (issue DoD). The value fields come from the drill, not from this plan.
- **11-21 production check (SRE, finance):** settings permission/persistence/audit and the control path with controlled metering inputs and a test owner; never spends real funds. Outside this plan.

---

## Self-review notes

- Issue coverage (#1496, 11-11 deliverable): cap as operator setting → #1621 + Task 7; 80 % alert + registration stop → Tasks 5, 6; 100 % stop of new turns/Sandboxes + safe pause → Tasks 2, 3, 4, 6; protected existing-consumer budget → `protected_reserve`, Task 1; per-owner allowance / 2 Sandboxes → declared elsewhere; operator settings, permission, audit → existing + Task 7; `spent+reserved+lag` design → Task 1–2 (formula mapping in Global Constraints); low balance or metering unavailable → `MeteringUnavailable` (Tasks 1–2) + declared elsewhere for balance; accepted work and reserves explicit → Task 4 refusal matrix. TC-125/126 preconditions → Tasks 2, 5, 6 tests; TC-127 → #1621.
- Spec coverage: §2.1/§2.2 → Tasks 1–2; §2.3 deferred (declared); §3–§4 → Task 2; §5.1 → Tasks 2, 4, 5; §5.2 → Tasks 3, 6; §5.3 → Tasks 3, 6; §5.4 → Tasks 2, 6 (`enter_closures`); §6 → Tasks 1, 2, 6, 7; §7 → Task 3; §8 → Task 7; §9 → no task (gateway work is #1566; conservative debit already holds); §10 → types only, no task; §11 → the drill itself is QA's 11-14 step, the automated tests above are its preconditions.
- Names used across tasks: `PlatformPeriodLedger`, `Closure`, `ClosureReason`, `CloudObservation`, `CloudSpendPolicy`, `SpendLevels`, `Refusal`, `Evaluation`, `PlatformSpendLedger`, `PlatformAdmissionState`, `PlatformSpendState`, `CloudObservationInput`, `PlatformSpendService`, `InactivityPauseCause`, `BudgetError::PlatformExhausted`, `CommandFailureCode::PlatformBudgetExhausted`, `budget_status`, `require_open`/`require_open_with`, `admit_liability`, `enter_closures`, `ensure_period_row`, `mirror_settle`/`mirror_release`/`mirror_retain`, `fenced_snapshot`/`read_snapshot`/`PolicySnapshot`, `save_period`, `PlatformWrites`, `PlatformStopCache`, `contention::backoff`.
- Concurrency rules 1–7 each have at least one test: rule 1 → Task 2 `a_refused_admission_leaves_one_spend_stop_visible_after_its_transaction_aborted`; rule 2 → Task 2 `zero_liability_admission_never_writes_the_settings_fence`; rule 3 → Task 3 `stop_reads_are_served_from_the_cache_within_its_ttl`; rule 4 → Task 6 `the_evaluator_does_not_write_when_nothing_changed`; rule 5 → Task 2 `twenty_concurrent_admissions_on_one_store_exhaust_no_retry_budget`; rule 6 → Task 6 `the_evaluator_pre_creates_the_period_row_outside_admission`; rule 7 → Task 1 `a_finished_period_is_metering_unavailable`.
