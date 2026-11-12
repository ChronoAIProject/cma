# Platform spend implementation notes

Running record kept while implementing
[IMPLEMENTATION.md](IMPLEMENTATION.md) against
[the contract](../../CMA_Platform_Spend_Metering.md) on `feat/platform-spend`.

## Out of scope

Things noticed while implementing that this plan does not change.

- `commit_conversation` retries only transient driver errors; a duplicate key
  from the in-transaction insert of a missing period row would surface as
  unavailable. In practice concurrent inserts surface as write conflicts, and
  the evaluator pre-creates the row (rule 6).
- Sandboxes with an Always keep alive preference (`always_on` or historical
  profile availability) are still kept alive by the maintenance transaction
  under a platform stop; the contract does not say whether a platform pause
  overrides an owner's keep-alive. Sandboxes with no live Agents are not
  candidates of the quiet-pause predicate either. Both keep running cloud
  spend under `SpendStop`; decide with #1478.
- The admin spend routes (`GET /api/v1/admin/spend`,
  `POST /api/v1/admin/spend/observations`) still need OpenAPI registration,
  `docs/API_REST.md` rows and public-contract corpus coverage.
- From the final review, deferred: `PlatformWrites` is taken for every
  message reservation regardless of the owner's policy and held across
  admission retry sleeps and unknown-commit reconciliation; revival and
  registration see staleness only after the next evaluation; the message
  checkpoint commit still retries with `yield_now` rather than
  `contention::backoff`; `ClosureReason` has no fallback for #1478's future
  reasons; owners absent from the user directory are treated as never-seen.
  (The period-scoped observation key, the `MeteringUnavailable` alert and
  sign-in under an unreadable state were settled in review round 1.)
- `#1672` (global Sandbox admission, merged into develop during this work) adds
  a `sandbox_capacity_unavailable` 503 that overlaps #1478's `CapacityLimit`
  reason; the two are not reconciled here.

## Deviations from the plan

Each entry: what the plan said, what was done instead, and why.

### Setup

- The branch was brought up to date with `develop` (4c65d80e8) by a merge
  commit. Plan references that moved: `sandbox_admission.rs` now calls
  `global::admit` (#1672) immediately before the live-limit check, so the
  platform check sits after both; `requests.rs::now()` takes the store (a test
  clock, `request_budget_clock`); `infra/api/configmap.yaml` gained keys, so
  "after L98" means after `CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED`.
- `cargo-nextest` and `cargo-deny` are not installed on the implementation
  machine. Local runs used the equivalent `cargo test` selections (same crate,
  same module paths, `--ignored` for Mongo tests); `cargo deny check` was not
  run locally (see Blocked).
- The plan's selectors `test(budget_execution_tests::)` and
  `test(request_budget_tests::)` match no test: those files are mounted as
  `execution_tests::budget_execution` and `execution_tests::request_budget`.
  The real module paths were used.

### Task 2

- The existing `budget_tests::configure` fixture saved `enabled: true` with a
  platform policy and no `cloud`. Under Review Focus 1 that refuses every
  commitment, so the fixture now configures an observed open period
  (2026-01-01 to 2100, cloud configured, zero observation at its start), and
  the three whole-collection counts exclude `platform:` rows. The aborted
  transaction test also asserts that the period row rolled back.
- `policy()` was removed; callers use `fenced_snapshot` directly.
- One generic `period::mirror(…, change)` replaces `mirror_settle`,
  `mirror_release` and `mirror_retain`; a turn settlement applies the retained
  delta and the settlement in one row write.
- `save_period` fails closed (`budget_unavailable`) on a revision filter miss
  rather than returning a transient retry: inside a snapshot transaction a
  concurrent writer already surfaces as a write conflict labelled
  `TransientTransactionError`, which every loop retries, and `ApplyError` has
  no retry variant.
- A refusal after the same transaction inserted the period row aborts that
  transaction before the guarded outside write (otherwise the outside write
  waits on the uncommitted insert). Refusals also pre-create the row outside
  (`ensure_period_row`), so a closure entered on a missing row is not lost.
- `start_turn` runs `require_open` only for a queued turn (one reserved with its
  command); a turn reserved in the same transaction already passed
  `admit_liability`. Since review round 1 an owner without a budget policy
  runs it for every turn start, as its message and model requests do.
- The command reservation loop is `agent_commands.rs::reserve_command`, not
  `agent_commands/reserve.rs`; `PlatformWrites` is taken there for messages.
  Other `reserve::apply` callers (Launch first messages, interrupting
  messages) are correct but unscheduled.
- The Mongo tests are mounted under `execution_tests` (as `budget_execution`
  is) because they need its execution harness; the selector
  `platform_period_tests::` still matches.
- `twenty_concurrent_admissions_…` asserts that every call returns `Ok`
  (retry exhaustion returns `Conflict`) instead of capturing the "contention
  exhausted" warning, which only conversation commits emit.
- `a_lost_runtime_mid_turn_…` simulates the loss by letting usage age out and
  requesting a stop with no terminal evidence; `settlement_after_period_end_…`
  rotates the policy to the next period before settling (a test cannot move
  the wall clock past `period_end`).
- `admit_liability` emits the `platform_spend_closure` event for a
  `SpendWarning` entered inside a successful admission; a retried transaction
  can emit it twice. `enter_closures` emits only for closures it inserted.

### Task 3

- The stop cache also carries the `platform_enabled` hint used for
  `PlatformWrites` scheduling (Task 2); a stale stop entry can delay a cancel
  by one five-second TTL and never admits anything. Tests age the entry
  through a test-only helper instead of sleeping five seconds.
- The platform check runs before the per-turn checks for every active turn,
  budgeted owner or not, as the contract's "every active turn" requires.
- The Responses error helper adds `Retry-After: 1` to every 503; the budget
  arm removes it, because a platform stop lifts only by operator revision.
- `first_message/startup.rs` enumerates the budget codes an unattempted
  first-message refusal may carry; without `platform_budget_exhausted` there,
  a Launch first message refused by the platform stop failed validation as
  corrupt. The code was added with a test. The shared-transcript redaction
  list in `agent_reads/visibility.rs` is the same kind of enumeration; it
  gained the code in review round 1.

### Task 4

- The plan's Layer 1/2 mapping list was incomplete for the compiler:
  `SandboxServiceError`, `WorkspaceServiceError`, `AgentServiceError`,
  `SandboxLifecycleCommandError`, `LaunchStoreError` and `LaunchServiceError`
  each gained a `Budget(BudgetError)` variant with its store mapping, and
  `HierarchyProblem`, `InstanceProblem::from_create`, `trigger_problem` and
  `commands/preparation.rs` (a queued command whose revival is refused now
  fails with the typed budget code instead of retrying as a dependency
  outage) map it. `HierarchyProblem::Budget` delegates to the shared
  `InstanceProblem::Budget` response.
- `SandboxService::new` returns `Arc<Self>`, so the ledger is attached once
  through `attach_platform_spend` (a `OnceLock`) instead of a constructor
  argument; the other construction sites are unchanged.
- Revival uses `PlatformAdmissionState::commitment_refusal()`: `SpendStop`
  refuses with `platform_budget_exhausted`, `MeteringUnavailable` alone with
  `budget_unavailable` (contract §5.1), matching Task 1's admission ruling.
- `record_cloud_observation` joins the `PlatformSpendLedger` trait in Task 7,
  where it is implemented, rather than as a stub here.
- `workspace_admission.rs` was at the 500-line limit; its
  `From<TransactionEventError>` conversion moved into
  `workspace_admission/retry.rs` beside the error type, next to the new
  conversion from the budget transaction error.
- The zero-liability checks run after each path's replay and limit checks, so
  exact replays of admitted creations are not refused.
- The Launch Mongo test lives in
  `trigger_definitions/launch_mongo_integration_tests/platform_spend.rs` (its
  fixtures and the CI selector are there); the Sandbox/Workspace/Agent ones in
  `command_ledger_tests/platform_commitment_tests.rs`, inside the CI Mongo
  selector, rather than at `sandbox_mongo_integration_tests/` top level, which
  that selector does not cover.
- `cancel_pause_and_approval_decisions_stay_available_under_spend_stop` covers
  pause through the lifecycle service; cancel and approval decisions go
  through `AgentOperationService`, which never reads the admission state.

### Task 5

- `registration_closed` lives in `auth/records.rs` beside the decision;
  `flow.rs` is close to the 500-line limit and only calls it.
- `admit_registration` first read the user directory only while a closure
  existed. Review round 1 reversed the order (contract decision 25): the
  directory is checked first, so an existing owner signs in without reading
  the admission state, and only a never-seen subject reads it, failing closed.
- The ledger is attached through `AuthState::with_platform_spend` in
  `composition/credentials.rs`, the pattern `with_binding_store` uses.

### Task 6

- `Phase::PlatformSpend` runs once per sweep after home cleanup and before
  the policy phase, so a stop reaches the candidate scan even when automatic
  inactivity pause is disabled; the idle-pause phase then uses
  `InactivityPauseCause::PlatformSpendStop` with a zero threshold.
- The pause trigger is `SpendStop` itself, not `closed_for_commitments()`:
  `MeteringUnavailable` alone never pauses (contract §5.2).
- The inherent evaluator is `MongoSandboxStore::evaluate_period`; the
  reconciler reaches it through a small `PlatformSpendSweep` trait so the
  in-memory sweep tests can supply a fake.
- The `platform_spend_closure` event is emitted by `enter_closures` (the
  writer whose guarded update inserted the closure), so the sweep does not
  emit a second one.
- An evaluation failure is logged and keeps the last known stop answer
  instead of failing the whole sweep; admission keeps failing closed
  independently. (After the final review, evaluation also runs on every
  `run_due` rather than only when the sweep reaches its phase.)
- `is_inactivity_pause_candidate` became a threshold-range check over a new
  `is_quiet_pause_candidate` (unchanged predicate) that the platform pause
  uses with a zero threshold; a platform pause request must carry threshold 0.
- The sweep test sits in `reconciliation_sweep_policy_tests.rs` to reuse its
  disabled-policy fixture; that fixture store records requests without
  eligibility, so "busy ones are left" is covered by the new
  `maintenance_tests.rs` cases.

### Task 7

- The two spend routes are served and tested through the admin router but are
  not registered in the OpenAPI document. Registering them requires rows in
  `docs/API_REST.md`, entries in `public-contract-inventory.json` and replayed
  corpus cases for success and every declared problem; that contract work is
  a follow-up (see Out of scope).
- `PlatformSpendLedger::record_cloud_observation` returns a typed
  `ObservationError` (`Invalid`, `Conflict`, `Unavailable`) so the conflict
  detail "observation conflicts with a recorded value" reaches the 422.
- A future `observed_through` or unsafe `units` is refused in Layer 1 and
  again by the Mongo ledger; the period-start bound is the ledger's.
- Beyond the plan's list: a Mongo test for the observation transaction
  (append once, idempotent repeat, conflict, bounds, period row update) and a
  Layer 1 round trip of the `cloud` settings fields; the SDK operations doc
  names the third flag.

### Final review

A fresh-context review found no Critical issues and four Important ones.
Three were fixed with a failing test first:

- A token-valuation mismatch for one owner entered a platform-wide
  `MeteringUnavailable` that the evaluator (which cannot see valuations)
  lifted and the owner's next try re-entered. It now refuses only that
  admission with `budget_unavailable` and enters no closure; a period change
  under liability still enters `MeteringUnavailable`. (Review round 1 removed
  the period row's valuation altogether; see below.)
- Turn ends reach the period row through the message checkpoint commit
  (`agent_conversation_store/checkpoint.rs`), which did not take
  `PlatformWrites`; it now does, like `commit_conversation`.
- Evaluation ran only when the sweep reached its platform phase, so a failing
  earlier phase held back stops, lifts and staleness. It now runs on every
  `run_due`, at most once per 15 seconds of the sweep clock, and the idle-pause
  phase re-reads the cached result per candidate.

The fourth (a refusal at `committed + L > disposable` enters `SpendStop` while
`committed` is still below `disposable`) is the plan's admission semantics and
TC-126's expectation; it was kept and recorded as contract decision 29, which
the user later reversed (see "Decision 29 ruling" below).

### Review round 1

The parent session's review of the branch; each fix is its own commit with a
test that failed first.

- `develop` merged (no rebase), 174a35dbf.
- Registration: the directory is checked before the admission state; an
  unreadable state signs existing owners in and refuses never-seen subjects
  (decision 25).
- Owners without a budget policy reserve nothing but meet the platform state
  on messages, turn starts (queued or not) and model requests, with the codes
  of decision 19 (decision 24).
- The period row no longer pins a token valuation; it sums the amounts the
  owner rows valued, so owners valued differently share one period
  (decision 30, sections 4, 4.2 and 6).
- Observation rows are keyed
  `observation:cloud:<period_start>:<observed_through>`, so adjacent periods
  each record an observation at their shared boundary (section 8).
- Shared transcripts show the static guidance for `platform_budget_exhausted`.
- Section 5.3 states that the alert also fires for `MeteringUnavailable` (the
  code already did).
- Decisions 26 (keep-alive under a platform pause) and 29 (refusal entering
  `SpendStop` before `committed` reaches `disposable`) were left unchanged for
  the user's ruling.

### Decision 29 ruling (2026-11-12)

The user ruled "stop at 100%"; the parent session relayed it. Decision 26
still waits for a ruling and is unchanged.

- An allowance that does not fit while `committed < disposable` is refused
  with `platform_budget_exhausted` and enters no `SpendStop` (a missing
  `SpendWarning` only, once `committed ≥ warning_level`); running turns
  continue and no Sandbox is paused.
- `SpendStop` is entered only at `committed ≥ disposable`: by the evaluator
  (decision 20, unchanged) or by a refused admission or zero-liability check
  that finds it there. A zero-liability commitment at exactly `disposable` is
  refused, matching the evaluator.
- This departs from the plan's Task 2 refusal semantics and from TC-126's
  original "refusal enters the stop" expectation; contract section 11 item 2
  now drives `committed` to `disposable` by settlement or observation.
- The Mongo test that asserted a refusal-entered stop became
  `an_allowance_that_does_not_fit_is_refused_and_running_work_stops_only_at_disposable`
  (a running turn keeps going after the refusal and receives the cancel only
  after an observation brings `committed` to `disposable`). The two-store
  test now checks that the race enters no stop and that both stores' later
  refusals at `disposable` record exactly one. The aborted-transaction test
  reaches `disposable` through observed cloud spend instead of an oversized
  allowance.
- `platform_budget_exhausted` guidance no longer says the Sandbox is paused;
  the section 5.2 pause reason covers a pause.

## Blocked

Nothing was blocked. Not run locally, because the tool is not installed on
the implementation machine or the gate is CI-only: `cargo nextest` (the
equivalent `cargo test` selections ran instead), `cargo deny check`,
gitleaks, Docker image builds and scans, kubeconform and kube-linter.

## Gates run (final HEAD)

- `cargo fmt --all --check`; `cargo clippy --locked --all-targets
  --all-features -- -D warnings` (workspace): clean.
- `cargo test --locked --workspace` (as `cargo test`, not nextest): cma
  1485, layer2-core 1422, layer1-public 582 (`--test-threads 1`), remaining
  crates 4596 passed. Six failures in crates this branch does not touch
  (four `cmaeg` console tests sharing process-global metric counters, two
  `layer4-runtime` tests sharing a writer lock) pass when run alone; nextest
  runs each test in its own process.
- `cargo test --locked --doc --workspace`: pass (no doctests).
- Mongo (`--ignored`, fresh replica set per chunk): the plan's Task 8
  selection plus Launch `platform_spend` 49/49; command ledger, agent worker
  and agent transaction CI selectors 227/227 after the review fixes.
- `check-file-size.sh`, `check-config-keys.sh`, `check-config-docs.mjs`, the
  Layer 2 vocabulary grep; no `Cargo.toml` changed (layer dependency rule).
- TypeScript SDK: `npm ci`, `lint`, `typecheck`, `format:check`, `test`
  (4327), `build`, `docs:check`.

## Local test environment

- Unit tests run with `RUST_MIN_STACK=33554432`, as CI does; without it an
  unrelated `agent_reads` test overflows the default 2 MiB thread stack.
- `browser_control_cases` share a process-wide file-operation limiter and
  return 429 when libtest runs them in parallel in one process; the Layer 1
  suite was run with `--test-threads 1` (CI's nextest isolates each test in
  its own process).
- Mongo tests ran in chunks of about 50 against a fresh disposable
  `mongo:8.0.13` single-node replica set each (loopback only, data on a
  volume), mirroring `.github/scripts/mongo-contract-tests.sh`; one long
  unpartitioned run had its container OOM-killed partway through.
