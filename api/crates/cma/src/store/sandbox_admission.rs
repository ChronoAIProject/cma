//! Serialized Mongo admission for sandbox limits and live names.

use std::time::Duration;

use futures_util::TryStreamExt;
use layer2_core::ids::SubjectId;
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionReplay, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxLimitClass, SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent, SandboxEventKind, SandboxOrigin};
use mongodb::bson::{doc, Bson};

use super::sandbox_events::{event_document, read_event};
use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::sandboxes::{admitted_sandbox_document, read_sandbox, read_sandbox_admission_replay};

mod canary;
mod catalogue;
mod global;
mod names;
mod public_owner;
mod retry;
mod runtime_image_origin;

const REPLAY_CANDIDATE_LIMIT: i64 = 2;
pub(super) enum AdmissionError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    Retry(mongodb::error::Error),
    Duplicate,
    PreparedLeaseBusy,
}

impl From<super::agent_conversation_store::ApplyError> for AdmissionError {
    fn from(error: super::agent_conversation_store::ApplyError) -> Self {
        match error {
            super::agent_conversation_store::ApplyError::Domain(error) => Self::Domain(error),
            super::agent_conversation_store::ApplyError::Driver(error) => Self::Driver(error),
        }
    }
}

enum AdmissionReceipt {
    Created {
        sandbox: Sandbox,
        event: SandboxEvent,
        prepared_outcome: &'static str,
    },
    Existing(Sandbox),
}

impl AdmissionReceipt {
    fn into_result(self) -> SandboxAdmissionResult {
        match self {
            Self::Created { sandbox, .. } => SandboxAdmissionResult::Created(sandbox),
            Self::Existing(sandbox) => SandboxAdmissionResult::Existing(sandbox),
        }
    }
}

#[async_trait::async_trait]
impl SandboxAdmissionStore for MongoSandboxStore {
    async fn sandbox_admission_replay(
        &self,
        owner: &SubjectId,
        origin: &SandboxOrigin,
    ) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError> {
        durable_origin_replay(self, owner, origin).await
    }

    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        let result = self.commit_sandbox_admission(admission).await?;
        // Includes durable replays and positively reconciled ambiguous commits.
        // A failed or cancelled admission leaves periodic discovery to recover.
        self.sandbox_admitted.notify_one();
        Ok(result)
    }

    async fn admit_catalogue_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        let result = self.commit_catalogue_sandbox_admission(admission).await?;
        self.sandbox_admitted.notify_one();
        Ok(result)
    }
}

fn admission_retry_delay(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    Duration::from_millis(1_u64.checked_shl(exponent).unwrap_or(u64::MAX).min(32))
}

async fn apply<F>(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    session: &mut mongodb::ClientSession,
    observe: &F,
    wait_for_prepared_lease: bool,
    retrying: bool,
    catalogue: bool,
) -> Result<AdmissionReceipt, AdmissionError>
where
    F: Fn() -> String + Sync,
{
    if let Some(fence) = &admission.creation_fence {
        if store.creation_writer_epoch.as_deref() != Some(fence.epoch.as_str()) {
            return Err(AdmissionError::Domain(SandboxStoreError::Unavailable(
                "Sandbox creation epoch is unsupported".into(),
            )));
        }
    }
    let resource_policy = store
        .bindings
        .admit_sandbox(admission, session)
        .await
        .map_err(AdmissionError::Domain)?;
    let owner = admission.sandbox.owner.as_str();
    store
        .sandbox_admissions
        .update_one(
            doc! { "_id": owner },
            doc! {
                "$inc": { "fence": 1_i64 },
                "$setOnInsert": { "document_schema_version": DOCUMENT_SCHEMA_VERSION },
            },
        )
        .upsert(true)
        .session(&mut *session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                AdmissionError::Retry(error)
            } else {
                AdmissionError::Driver(error)
            }
        })?;
    let database = store.client.database(&store.sandboxes.namespace().db);
    if let Some(sandbox) = replay_in_transaction(store, admission, session).await? {
        return Ok(AdmissionReceipt::Existing(sandbox));
    }
    if super::account_deletions::erasing_in_session(&database, &admission.sandbox.owner, session)
        .await
        .map_err(AdmissionError::Driver)?
    {
        return Err(AdmissionError::Domain(
            SandboxStoreError::AccountDeletionInProgress,
        ));
    }
    let image_admission = canary::verify(
        &database,
        admission,
        store.runtime_image_origin_enabled,
        session,
    )
    .await?;
    if catalogue {
        catalogue::admit(&database, &admission.sandbox, session).await?;
    }
    if retrying
        && admission.admission_cause() == layer2_core::admission::AdmissionCause::Fresh
        && super::launch_home_capacity::precheck_fresh_home(
            &store.budget_settings,
            &store.sandbox_admissions,
            &store.sandboxes,
            store.global_sandbox_limit,
            &admission.sandbox.owner,
            session,
        )
        .await
        .map_err(AdmissionError::Driver)?
        .is_some()
    {
        return Err(AdmissionError::Domain(
            SandboxStoreError::GlobalCapacityReached,
        ));
    }
    let protected =
        super::settings_barrier::acquire(&store.budget_settings, store.initial_settings, session)
            .await
            .map_err(|error| match error {
                super::settings_barrier::Error::Driver(error) if is_duplicate_key(&error) => {
                    AdmissionError::Retry(error)
                }
                super::settings_barrier::Error::Driver(error) => AdmissionError::Driver(error),
                super::settings_barrier::Error::Unreadable => AdmissionError::Domain(
                    SandboxStoreError::Budget(layer2_core::budget::BudgetError::Unavailable),
                ),
            })?;
    let mut live_limit = admission.live_limit.min(
        admission
            .limit_class
            .current_limit(protected.settings())
            .map_err(AdmissionError::Domain)?,
    );
    if admission.limit_class == SandboxLimitClass::Owner {
        live_limit = live_limit.min(
            store
                .public_owner_sandbox_limit(&admission.sandbox.owner, Some(&mut *session))
                .await
                .map_err(AdmissionError::Driver)?
                .unwrap_or(live_limit),
        );
    }
    let live = store
        .sandboxes
        .count_documents(doc! { "owner": owner, "retired_at": Bson::Null })
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    if live >= u64::from(live_limit) {
        return Err(AdmissionError::Domain(SandboxStoreError::LimitReached {
            limit: live_limit,
            live: u16::try_from(live).unwrap_or(u16::MAX),
        }));
    }
    let ceilings = super::budget::require_open_protected(
        &store.budget_settings,
        &store.model_budget,
        &protected,
        super::budget::now_seconds(),
        admission.admission_cause(),
        store.platform_counter_shards,
        session,
    )
    .await?;
    global::admit(store, admission, &ceilings, session).await?;
    let mut sandbox = admission.sandbox.clone();
    image_admission
        .record_origin(store, &mut sandbox, session)
        .await?;
    sandbox.name = names::choose_name(store, session, owner, &admission.name).await?;
    let prepared_outcome = if let canary::ImageAdmission::Canary(lease) = image_admission {
        canary::bind(&database, &lease, &sandbox.id, session).await?;
        "miss_runtime_canary_cold_only"
    } else if admission.binding.is_none() {
        use super::native_prepared::ClaimOutcome;
        // The transaction snapshot already exists. Observe now, so a health
        // completion visible in this snapshot cannot postdate our lease clock.
        let observed_at = observe();
        match super::native_prepared::claim_ready(store, &mut sandbox, &observed_at, session)
            .await?
        {
            ClaimOutcome::Claimed => "hit",
            ClaimOutcome::Busy if wait_for_prepared_lease => {
                return Err(AdmissionError::PreparedLeaseBusy);
            }
            outcome => {
                super::warm_sandboxes::claim_ready(
                    store,
                    &mut sandbox,
                    &admission.event_at,
                    session,
                )
                .await?;
                match outcome {
                    ClaimOutcome::Busy => "miss_health_lease_timeout",
                    ClaimOutcome::Miss(reason) => reason,
                    ClaimOutcome::Claimed => unreachable!("claimed above"),
                }
            }
        }
    } else {
        "miss_bound_creation"
    };
    let event = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: admission.event_at.clone(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandboxes
        .insert_one(
            admitted_sandbox_document(
                &sandbox,
                1,
                &admission.name,
                &admission.admission_fingerprint,
            )
            .map_err(AdmissionError::Domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(classify_insert)?;
    store
        .sandbox_events
        .insert_one(event_document(&event).map_err(AdmissionError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_insert)?;
    super::resource_access::insert_admission_policy(store, resource_policy.as_ref(), session)
        .await?;
    super::sandbox_usage::lifecycle_event(store, session, &event, None)
        .await
        .map_err(|error| match error {
            super::sandbox_events::TransactionEventError::Domain(error) => {
                AdmissionError::Domain(error)
            }
            super::sandbox_events::TransactionEventError::Driver(error) => {
                AdmissionError::Driver(error)
            }
        })?;
    if !super::creation_outcomes::publish_sandbox(
        &database,
        admission,
        sandbox.id.as_str(),
        session,
    )
    .await
    .map_err(AdmissionError::Driver)?
    {
        return Err(AdmissionError::Domain(SandboxStoreError::Unavailable(
            "Sandbox creation evidence fence is no longer held".into(),
        )));
    }
    Ok(AdmissionReceipt::Created {
        sandbox,
        event,
        prepared_outcome,
    })
}

async fn replay_in_transaction(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    session: &mut mongodb::ClientSession,
) -> Result<Option<Sandbox>, AdmissionError> {
    let Some(filter) = admission_filter(admission) else {
        return Ok(None);
    };
    let mut cursor = store
        .sandboxes
        .find(crate::store::work_schema::query(filter))
        .limit(REPLAY_CANDIDATE_LIMIT)
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(AdmissionError::Driver)?;
    unique_origin_replay(documents)
        .map_err(AdmissionError::Domain)?
        .map(|replay| classify_replay_record(admission, replay).map_err(AdmissionError::Domain))
        .transpose()
}

async fn durable_replay(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
) -> Result<Option<Sandbox>, SandboxStoreError> {
    let Some(_) = admission.origin_key() else {
        return Ok(None);
    };
    durable_origin_replay(store, &admission.sandbox.owner, &admission.sandbox.origin)
        .await?
        .map(|replay| classify_replay_record(admission, replay))
        .transpose()
}

async fn durable_origin_replay(
    store: &MongoSandboxStore,
    owner: &SubjectId,
    origin: &SandboxOrigin,
) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError> {
    let mut cursor = store
        .sandboxes
        .find(crate::store::work_schema::query(
            origin_filter(owner.as_str(), origin).ok_or(SandboxStoreError::InvalidTransaction(
                "manual origin has no replay identity",
            ))?,
        ))
        .limit(REPLAY_CANDIDATE_LIMIT)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let mut documents = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        documents.push(document);
    }
    unique_origin_replay(documents)
}

fn admission_filter(admission: &SandboxAdmission) -> Option<mongodb::bson::Document> {
    origin_filter(admission.sandbox.owner.as_str(), &admission.sandbox.origin)
}

pub(super) fn origin_filter(
    owner: &str,
    origin: &SandboxOrigin,
) -> Option<mongodb::bson::Document> {
    let (kind, id) = origin.replay_key()?;
    let (query_field, record_field) = match origin {
        SandboxOrigin::Trigger { .. } => ("origin_launch_id", "record.origin.launch_id"),
        SandboxOrigin::Lifecycle { .. } => ("origin_operation_id", "record.origin.operation_id"),
        SandboxOrigin::Manual => return None,
    };
    Some(doc! {
        "$or": [
            { "owner": owner, "origin_type": kind, query_field: id },
            { "record.owner": owner, "record.origin.type": kind, record_field: id },
        ],
    })
}

fn unique_origin_replay(
    mut documents: Vec<mongodb::bson::Document>,
) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError> {
    if documents.len() > 1 {
        return Err(SandboxStoreError::Corrupt(
            "multiple sandboxes claim one admission origin".into(),
        ));
    }
    documents
        .pop()
        .as_ref()
        .map(read_sandbox_admission_replay)
        .transpose()
}

fn classify_replay_record(
    admission: &SandboxAdmission,
    replay: SandboxAdmissionReplay,
) -> Result<Sandbox, SandboxStoreError> {
    if replay.admission_fingerprint == admission.admission_fingerprint {
        Ok(replay.sandbox)
    } else {
        Err(SandboxStoreError::SandboxOriginConflict)
    }
}

fn classify_insert(error: mongodb::error::Error) -> AdmissionError {
    if is_duplicate_key(&error) {
        AdmissionError::Duplicate
    } else {
        AdmissionError::Driver(error)
    }
}

async fn reconcile(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    receipt: &AdmissionReceipt,
) -> Result<Option<SandboxAdmissionResult>, SandboxStoreError> {
    if let Some(current) = durable_replay(store, admission).await? {
        return match receipt {
            AdmissionReceipt::Created { sandbox, event, .. } if current.id == sandbox.id => {
                let stored_event = store
                    .sandbox_events
                    .find_one(doc! { "_id": format!("{}:{:020}", event.sandbox_id, 1) })
                    .await
                    .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                    .as_ref()
                    .map(read_event)
                    .transpose()?;
                Ok((stored_event.as_ref() == Some(event))
                    .then_some(SandboxAdmissionResult::Created(current)))
            }
            AdmissionReceipt::Created { .. } | AdmissionReceipt::Existing(_) => {
                Ok(Some(SandboxAdmissionResult::Existing(current)))
            }
        };
    }
    let AdmissionReceipt::Created { sandbox, event, .. } = receipt else {
        return Ok(None);
    };
    let stored_sandbox = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    let stored_event = store
        .sandbox_events
        .find_one(doc! { "_id": format!("{}:{:020}", event.sandbox_id, 1) })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_event)
        .transpose()?;
    Ok(
        (stored_sandbox.as_ref() == Some(sandbox) && stored_event.as_ref() == Some(event))
            .then(|| SandboxAdmissionResult::Created(sandbox.clone())),
    )
}

#[cfg(test)]
#[path = "sandbox_admission_tests.rs"]
mod tests;
