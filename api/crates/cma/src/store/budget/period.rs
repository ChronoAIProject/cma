//! Exact period admission over its stored accounting layout. Owner and platform
//! liability changes share the caller's transaction (contract section 4).

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::admission::{AdmissionCause, AdmissionCeilings};
use layer2_core::budget::{
    BudgetError, BudgetPolicy, Closure, OperatorAdmissionState, PlatformBudgetPolicy,
    PlatformPeriodLedger, Refusal,
};
use layer2_core::SubjectId;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{ClientSession, Collection};

pub(super) use super::period_storage::read as read_period;
use super::period_storage::PeriodState;
use super::{domain, ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::sandbox_mongo::is_duplicate_key;

/// One settings read: the owner's policy, the platform policy when budgets are
/// enabled, and the settings revision every level is computed under.
pub(super) struct PolicySnapshot {
    pub(super) owner: Option<BudgetPolicy>,
    pub(super) platform: Option<PlatformBudgetPolicy>,
    pub(super) revision: u64,
    pub(super) operator_control: OperatorAdmissionState,
    pub(super) ceilings: Result<AdmissionCeilings, ()>,
}

/// Liability-bearing reservations only: the foundation's fence write plus the
/// settings read, inside the admitting transaction.
pub(super) async fn fenced_snapshot(
    store: &MongoSandboxStore,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<PolicySnapshot, ApplyError> {
    fence_settings(
        &store.budget_settings,
        "model_budget_admission_fence",
        session,
    )
    .await?;
    let document = store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let snapshot = snapshot(&document, Some(owner)).map_err(domain)?;
    store.note_platform_enabled(snapshot.platform.is_some());
    Ok(snapshot)
}

/// Every model liability increase shares this real write, including late usage
/// and cloud corrections. Snapshot reads alone do not prevent cross-counter skew.
pub(super) async fn fence_settings(
    settings: &Collection<Document>,
    counter: &str,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    crate::store::settings_barrier::increment(settings, counter, session)
        .await
        .map_err(|error| match error {
            crate::store::settings_barrier::Error::Driver(error) => ApplyError::Driver(error),
            crate::store::settings_barrier::Error::Unreadable => domain(BudgetError::Unavailable),
        })
}

/// Zero-liability commitments: a plain read inside the session snapshot, with
/// no fence write (concurrency rule 2). A missing settings document is the
/// unconfigured platform, as for the stop path.
pub(super) async fn read_snapshot(
    settings: &Collection<Document>,
    owner: Option<&SubjectId>,
    session: &mut ClientSession,
) -> Result<Option<PolicySnapshot>, ApplyError> {
    settings
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .map(|document| snapshot(&document, owner).map_err(domain))
        .transpose()
}

pub(super) fn snapshot(
    document: &Document,
    owner: Option<&SubjectId>,
) -> Result<PolicySnapshot, BudgetError> {
    let revision = match document.get("revision") {
        None => 0,
        Some(Bson::Int64(value)) => u64::try_from(*value).map_err(|_| BudgetError::Unavailable)?,
        Some(Bson::Int32(value)) => u64::try_from(*value).map_err(|_| BudgetError::Unavailable)?,
        Some(_) => return Err(BudgetError::Unavailable),
    };
    let settings: layer2_core::PlatformSettings = mongodb::bson::deserialize_from_document(
        document
            .get_document("settings")
            .map_err(|_| BudgetError::Unavailable)?
            .clone(),
    )
    .map_err(|_| BudgetError::Unavailable)?;
    settings.model_budget.validate()?;
    let budget = settings.model_budget;
    Ok(PolicySnapshot {
        ceilings: crate::store::admission_ceilings::from_document(document)
            .map(|state| state.ceilings)
            .map_err(|_| ()),
        operator_control: match crate::store::admission_control::from_document(document) {
            Ok(control) => control.operator_stop.into(),
            Err(_) => OperatorAdmissionState::Unreadable,
        },
        owner: owner.and_then(|owner| budget.for_owner(owner).cloned()),
        // Platform spend metering is enabled only with a configured cloud
        // source (decision 31); without one, owner and turn budgets behave as
        // #1621 alone and no period row exists.
        platform: budget
            .platform
            .filter(|platform| budget.enabled && platform.cloud.is_some()),
        revision,
    })
}

pub(in crate::store) use cma_am_spend::period_key;

/// The evaluator pre-creates the row (rule 6). An admission that still finds
/// it missing inserts it; a concurrent insert surfaces as the callers' existing
/// write-conflict or duplicate-key retry.
pub(super) async fn load_or_open(
    store: &MongoSandboxStore,
    platform: &PlatformBudgetPolicy,
    revision: u64,
    session: &mut ClientSession,
) -> Result<(PeriodState, bool), ApplyError> {
    let (start, _) = platform
        .period_bounds()
        .map_err(|_| domain(BudgetError::Unavailable))?;
    let read = if store.platform_counter_shards {
        super::period_storage::read_batched(&store.model_budget, start, session).await?
    } else {
        read_period(&store.model_budget, start, session).await?
    };
    if let Some(ledger) = read {
        return Ok((ledger, false));
    }
    let ledger = PeriodState::from(
        PlatformPeriodLedger::open(platform, revision)
            .map_err(|_| domain(BudgetError::Unavailable))?,
    );
    store
        .model_budget
        .insert_one(ledger.document().map_err(domain)?)
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    Ok((ledger, true))
}

/// Every write filters on the row revision and writes `revision + 1`. Inside a
/// snapshot transaction a concurrent writer already surfaces as a retried write
/// conflict, so a miss here is an inconsistency and fails closed.
pub(super) async fn save_period(
    store: &MongoSandboxStore,
    state: &PeriodState,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let ledger = &state.ledger;
    let mut next = state.clone();
    next.ledger.revision = ledger
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let result = store
        .model_budget
        .replace_one(
            doc! {"_id": period_key(ledger.period_start), "record.revision": ledger.revision as i64},
            next.document().map_err(domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(domain(BudgetError::Unavailable));
    }
    Ok(())
}

/// Mirror an owner reservation, already valued in the shared accounting units,
/// inside its transaction. `None` when no enabled platform policy exists;
/// otherwise the period the liability was charged to.
pub(super) async fn admit_liability(
    store: &MongoSandboxStore,
    snapshot: &PolicySnapshot,
    owner: &SubjectId,
    liability: u64,
    now: u64,
    session: &mut ClientSession,
) -> Result<Option<super::counter_accounting::Charge>, ApplyError> {
    let Some(platform) = &snapshot.platform else {
        return Ok(None);
    };
    let (mut ledger, inserted) = load_or_open(store, platform, snapshot.revision, session).await?;
    let mut admitted = ledger.ledger.clone();
    match admitted.admit(platform, snapshot.revision, liability, now) {
        Ok(entered) => {
            let metadata_changed = admitted.period_end != ledger.ledger.period_end
                || admitted.settings_revision != ledger.ledger.settings_revision
                || !entered.is_empty();
            // Convert only a successful admission, using pre-admission balances.
            // A refusal must retain the committed layout/revision as evidence
            // for its independent closure write after the transaction aborts.
            super::counter_accounting::activate(store, &mut ledger, session).await?;
            admitted.revision = ledger.ledger.revision;
            ledger.ledger = admitted;
            let charge = super::counter_accounting::reserve(
                store,
                &ledger,
                owner,
                liability,
                metadata_changed,
                session,
            )
            .await?;
            entered.iter().for_each(alert);
            Ok(Some(charge))
        }
        Err(refusal) => Err(refuse(
            &store.budget_settings,
            &store.model_budget,
            snapshot.revision,
            (!inserted).then_some(ledger.ledger.revision),
            ledger,
            refusal,
            session,
        )
        .await),
    }
}

/// Zero-liability commitments read the settings and the row in their snapshot
/// and never write either (rule 2); a refusal's closures go outside.
#[cfg(test)]
pub(crate) async fn require_open_with(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    now: u64,
    cause: AdmissionCause,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    require_open_read(settings, rows, now, cause, false, session).await
}

async fn require_open_read(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    now: u64,
    cause: AdmissionCause,
    batched: bool,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let Some(snapshot) = read_snapshot(settings, None, session).await? else {
        return Ok(());
    };
    require_snapshot_spend_open(settings, rows, &snapshot, now, batched, session).await?;
    require_operator_open(&snapshot, cause)
}

/// Reuse the same settings snapshot while preserving spend refusal precedence.
async fn require_snapshot_spend_open(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    snapshot: &PolicySnapshot,
    now: u64,
    batched: bool,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let Some(platform) = &snapshot.platform else {
        return Ok(());
    };
    let revision = snapshot.revision;
    let (start, _) = platform
        .period_bounds()
        .map_err(|_| domain(BudgetError::Unavailable))?;
    let read = if batched {
        super::period_storage::read_batched(rows, start, session).await?
    } else {
        read_period(rows, start, session).await?
    };
    let expected = read.as_ref().map(|state| state.ledger.revision);
    let mut ledger = match read {
        Some(ledger) => ledger,
        None => PeriodState::from(
            PlatformPeriodLedger::open(platform, revision)
                .map_err(|_| domain(BudgetError::Unavailable))?,
        ),
    };
    match ledger.ledger.admit(platform, revision, 0, now) {
        Ok(_) => Ok(()),
        Err(refusal) => {
            Err(refuse(settings, rows, revision, expected, ledger, refusal, session).await)
        }
    }
}

pub(super) fn require_operator_open(
    snapshot: &PolicySnapshot,
    cause: AdmissionCause,
) -> Result<(), ApplyError> {
    if cause == AdmissionCause::Fresh && !snapshot.operator_control.is_open() {
        return Err(ApplyError::Domain(
            layer2_core::sandbox::store::SandboxStoreError::AdmissionPaused,
        ));
    }
    Ok(())
}

/// Wall-clock seconds for zero-liability checks; an unreadable clock is far
/// past any period end, which fails closed as metering unavailable.
pub(in crate::store) fn now_seconds() -> u64 {
    layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap_or(u64::MAX)
}

pub(in crate::store) async fn require_open(
    store: &MongoSandboxStore,
    now: u64,
    cause: AdmissionCause,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    require_open_read(
        &store.budget_settings,
        &store.model_budget,
        now,
        cause,
        store.platform_counter_shards,
        session,
    )
    .await
}

/// Count-bearing admissions use the same protected document for every cap.
pub(in crate::store) async fn require_open_protected(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    protected: &crate::store::settings_barrier::ProtectedSettings,
    now: u64,
    cause: AdmissionCause,
    batched: bool,
    session: &mut ClientSession,
) -> Result<Result<AdmissionCeilings, ()>, ApplyError> {
    let snapshot = snapshot(protected.document(), None).map_err(domain)?;
    require_snapshot_spend_open(settings, rows, &snapshot, now, batched, session).await?;
    require_operator_open(&snapshot, cause)?;
    Ok(snapshot.ceilings)
}

/// The historical owner-policy bypass is only valid with global metering off.
/// An active meter cannot value this commitment without an owner policy; do
/// not grant a provider permit or a turn without its conservative reservation.
pub(super) async fn require_unbudgeted(
    store: &MongoSandboxStore,
    snapshot: &PolicySnapshot,
    now: u64,
    cause: AdmissionCause,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    require_snapshot_spend_open(
        &store.budget_settings,
        &store.model_budget,
        snapshot,
        now,
        store.platform_counter_shards,
        session,
    )
    .await?;
    if snapshot.platform.is_some() {
        return Err(domain(BudgetError::Unavailable));
    }
    require_operator_open(snapshot, cause)
}

/// Release admission locks before independently recording a refusal. The
/// original refusal stands even when changed evidence defers its closure.
async fn refuse(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    revision: u64,
    expected: Option<u64>,
    mut ledger: PeriodState,
    refusal: Refusal,
    session: &mut ClientSession,
) -> ApplyError {
    if let Err(error) = session.abort_guarded().await {
        tracing::warn!(event = "platform_spend_refusal_abort", error = %error,
            "could not confirm admission abort; closure persistence deferred");
        return domain(refusal.reason);
    }
    if !refusal.closures.is_empty() {
        ledger
            .ledger
            .closures
            .extend(refusal.closures.iter().cloned());
        match super::transitions::persist(settings, rows, revision, expected, &ledger).await {
            Ok(true) => refusal.closures.iter().for_each(alert),
            Ok(false) => tracing::debug!(
                event = "platform_spend_closure_stale",
                "changed evidence deferred refusal closure persistence"
            ),
            Err(_) => tracing::warn!(
                event = "platform_spend_closure_deferred",
                "platform spend closure could not be recorded with its refusal"
            ),
        }
    }
    domain(refusal.reason)
}

/// Pre-create the row outside any admission transaction (rule 6).
pub(super) async fn ensure_period_row(
    rows: &Collection<Document>,
    platform: &PlatformBudgetPolicy,
    revision: u64,
) -> Result<(), BudgetError> {
    let ledger = PlatformPeriodLedger::open(platform, revision)?;
    let record = mongodb::bson::serialize_to_bson(&ledger).map_err(|_| BudgetError::Unavailable)?;
    match rows
        .update_one(
            doc! {"_id": period_key(ledger.period_start)},
            doc! {"$setOnInsert": {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": record,
            }},
        )
        .upsert(true)
        .await
    {
        Ok(_) => Ok(()),
        Err(error) if is_duplicate_key(&error) => Ok(()),
        Err(_) => Err(BudgetError::Unavailable),
    }
}

/// The alert record: reason and settings revision, never amounts.
pub(super) fn alert(closure: &Closure) {
    tracing::warn!(
        parent: None,
        event = "platform_spend_closure",
        reason = ?closure.reason,
        settings_revision = closure.settings_revision,
        "platform spend closure entered"
    );
}
