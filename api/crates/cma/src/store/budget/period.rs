//! The platform period row: one document per period in the reservation
//! collection, moved in the same transactions as the owner rows (contract §4).

use layer2_core::budget::{
    BudgetError, BudgetPolicy, Closure, ClosureReason, PlatformBudgetPolicy, PlatformPeriodLedger,
    Refusal,
};
use layer2_core::SubjectId;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{ClientSession, Collection};

use super::{decode, domain, ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::sandbox_mongo::is_duplicate_key;

/// One settings read: the owner's policy, the platform policy when budgets are
/// enabled, and the settings revision every level is computed under.
pub(super) struct PolicySnapshot {
    pub(super) owner: Option<BudgetPolicy>,
    pub(super) platform: Option<PlatformBudgetPolicy>,
    pub(super) revision: u64,
}

/// Liability-bearing reservations only: the foundation's fence write plus the
/// settings read, inside the admitting transaction.
pub(super) async fn fenced_snapshot(
    store: &MongoSandboxStore,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<PolicySnapshot, ApplyError> {
    // A real write also fences activation against previously disabled admissions.
    // The additive fence is outside the strict settings record.
    let result = store
        .budget_settings
        .update_one(
            doc! {"_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION},
            doc! {"$inc": {"model_budget_admission_fence": 1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(domain(BudgetError::Unavailable));
    }
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

pub(in crate::store) fn period_key(period_start: u64) -> String {
    let start = std::time::UNIX_EPOCH + std::time::Duration::from_secs(period_start);
    format!("platform:{}", layer2_core::rfc3339_at(start))
}

fn period_document(ledger: &PlatformPeriodLedger) -> Result<Document, BudgetError> {
    let record = mongodb::bson::serialize_to_bson(ledger).map_err(|_| BudgetError::Unavailable)?;
    Ok(doc! {
        "_id": period_key(ledger.period_start),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "record": record,
    })
}

async fn read_period(
    rows: &Collection<Document>,
    period_start: u64,
    session: &mut ClientSession,
) -> Result<Option<PlatformPeriodLedger>, ApplyError> {
    rows.find_one(doc! {"_id": period_key(period_start)})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(decode)
        .transpose()
}

/// The evaluator pre-creates the row (rule 6). An admission that still finds
/// it missing inserts it; a concurrent insert surfaces as the callers' existing
/// write-conflict or duplicate-key retry.
pub(super) async fn load_or_open(
    store: &MongoSandboxStore,
    platform: &PlatformBudgetPolicy,
    revision: u64,
    session: &mut ClientSession,
) -> Result<(PlatformPeriodLedger, bool), ApplyError> {
    let (start, _) = platform
        .period_bounds()
        .map_err(|_| domain(BudgetError::Unavailable))?;
    if let Some(ledger) = read_period(&store.model_budget, start, session).await? {
        return Ok((ledger, false));
    }
    let ledger = PlatformPeriodLedger::open(platform, revision)
        .map_err(|_| domain(BudgetError::Unavailable))?;
    store
        .model_budget
        .insert_one(period_document(&ledger).map_err(domain)?)
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
    ledger: &PlatformPeriodLedger,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let mut next = ledger.clone();
    next.revision = ledger
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let result = store
        .model_budget
        .replace_one(
            doc! {"_id": period_key(ledger.period_start), "record.revision": ledger.revision as i64},
            period_document(&next).map_err(domain)?,
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
    liability: u64,
    now: u64,
    session: &mut ClientSession,
) -> Result<Option<u64>, ApplyError> {
    let Some(platform) = &snapshot.platform else {
        return Ok(None);
    };
    let (mut ledger, inserted) = load_or_open(store, platform, snapshot.revision, session).await?;
    match ledger.admit(platform, snapshot.revision, liability, now) {
        Ok(entered) => {
            save_period(store, &ledger, session).await?;
            entered.iter().for_each(alert);
            Ok(Some(ledger.period_start))
        }
        Err(refusal) => {
            if inserted {
                // Our uncommitted insert would block the guarded write below.
                let _ = session.abort_transaction().await;
            }
            Err(refuse(
                &store.model_budget,
                platform,
                snapshot.revision,
                ledger.period_start,
                refusal,
            )
            .await)
        }
    }
}

/// Zero-liability commitments read the settings and the row in their snapshot
/// and never write either (rule 2); a refusal's closures go outside.
pub(crate) async fn require_open_with(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let Some(PolicySnapshot {
        platform: Some(platform),
        revision,
        ..
    }) = read_snapshot(settings, None, session).await?
    else {
        return Ok(());
    };
    let (start, _) = platform
        .period_bounds()
        .map_err(|_| domain(BudgetError::Unavailable))?;
    let mut ledger = match read_period(rows, start, session).await? {
        Some(ledger) => ledger,
        None => PlatformPeriodLedger::open(&platform, revision)
            .map_err(|_| domain(BudgetError::Unavailable))?,
    };
    match ledger.admit(&platform, revision, 0, now) {
        Ok(_) => Ok(()),
        Err(refusal) => Err(refuse(rows, &platform, revision, start, refusal).await),
    }
}

/// Wall-clock seconds for zero-liability checks; an unreadable clock is far
/// past any period end, which fails closed as metering unavailable.
pub(in crate::store) fn now_seconds() -> u64 {
    layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap_or(u64::MAX)
}

pub(in crate::store) async fn require_open(
    store: &MongoSandboxStore,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    require_open_with(&store.budget_settings, &store.model_budget, now, session).await
}

/// Enter a refusal's closures after (or regardless of) the abort, then return
/// the refusal reason for the caller's loop to abort on (rule 1). Entering is
/// best effort: the refusal stands and the evaluator re-derives the closure.
async fn refuse(
    rows: &Collection<Document>,
    platform: &PlatformBudgetPolicy,
    revision: u64,
    period_start: u64,
    refusal: Refusal,
) -> ApplyError {
    if !refusal.closures.is_empty() {
        let entered = match ensure_period_row(rows, platform, revision).await {
            Ok(()) => enter_closures(rows, period_start, &refusal.closures).await,
            Err(error) => Err(error),
        };
        if entered.is_err() {
            tracing::warn!(
                event = "platform_spend_closure_deferred",
                "platform spend closure could not be recorded with its refusal"
            );
        }
    }
    domain(refusal.reason)
}

/// Guarded single-document writes outside any transaction: first writer wins
/// and repeats are no-ops. Returns the reasons this caller actually inserted;
/// only that caller emits the alert record.
pub(super) async fn enter_closures(
    rows: &Collection<Document>,
    period_start: u64,
    closures: &[Closure],
) -> Result<Vec<ClosureReason>, BudgetError> {
    let key = period_key(period_start);
    let mut inserted = Vec::new();
    for closure in closures {
        let reason = mongodb::bson::serialize_to_bson(&closure.reason)
            .map_err(|_| BudgetError::Unavailable)?;
        let value =
            mongodb::bson::serialize_to_bson(closure).map_err(|_| BudgetError::Unavailable)?;
        let result = rows
            .update_one(
                doc! {"_id": &key, "record.closures.reason": {"$ne": reason}},
                doc! {"$push": {"record.closures": value}, "$inc": {"record.revision": 1_i64}},
            )
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        if result.modified_count == 1 {
            alert(closure);
            inserted.push(closure.reason);
        }
    }
    Ok(inserted)
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

/// Apply a settlement, release or retention to the row a reservation was
/// charged to, in the caller's transaction. A reservation made before the
/// platform policy existed carries no period and never touches a row.
pub(super) async fn mirror(
    store: &MongoSandboxStore,
    period_start: Option<u64>,
    session: &mut ClientSession,
    change: impl FnOnce(&mut PlatformPeriodLedger) -> Result<(), BudgetError>,
) -> Result<(), ApplyError> {
    let Some(start) = period_start else {
        return Ok(());
    };
    let mut ledger = read_period(&store.model_budget, start, session)
        .await?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    change(&mut ledger).map_err(domain)?;
    save_period(store, &ledger, session).await
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
