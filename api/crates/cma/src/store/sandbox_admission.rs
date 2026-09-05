//! Serialized Mongo admission for sandbox limits and live names.

use std::time::Duration;

use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent, SandboxEventKind};
use mongodb::bson::{doc, Bson};

use super::sandbox_events::{event_document, read_event};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::{admitted_sandbox_document, read_admission_fingerprint, read_sandbox};

const ADMISSION_TRANSACTION_ATTEMPTS: usize = 16;
const ADMISSION_RETRY_MAX_DELAY_MILLIS: u64 = 32;

enum AdmissionError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    Retry(mongodb::error::Error),
    Duplicate,
}

enum AdmissionReceipt {
    Created {
        sandbox: Sandbox,
        event: SandboxEvent,
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
    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        admission.validate()?;
        let mut last_transient = None;
        for attempt in 0..ADMISSION_TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(admission_retry_delay(attempt)).await;
            }
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let receipt = match apply(self, &admission, &mut session).await {
                Ok(receipt) => receipt,
                Err(AdmissionError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(AdmissionError::Retry(error)) | Err(AdmissionError::Driver(error))
                    if is_transient(&error) =>
                {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(AdmissionError::Retry(error)) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(AdmissionError::Duplicate) => {
                    let _ = session.abort_transaction().await;
                    return match durable_replay(self, &admission).await? {
                        Some(sandbox) => Ok(SandboxAdmissionResult::Existing(sandbox)),
                        None => Err(SandboxStoreError::Duplicate),
                    };
                }
                Err(AdmissionError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt.into_result()),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return reconcile(self, &admission, &receipt).await?.ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "sandbox admission commit remained unknown: {error}"
                        ))
                    });
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "sandbox admission exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}

fn admission_retry_delay(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let millis = 1_u64
        .checked_shl(exponent)
        .unwrap_or(u64::MAX)
        .min(ADMISSION_RETRY_MAX_DELAY_MILLIS);
    Duration::from_millis(millis)
}

async fn apply(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    session: &mut mongodb::ClientSession,
) -> Result<AdmissionReceipt, AdmissionError> {
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
    if let Some(sandbox) = replay_in_transaction(store, admission, session).await? {
        return Ok(AdmissionReceipt::Existing(sandbox));
    }
    let live = store
        .sandboxes
        .count_documents(doc! { "owner": owner, "retired_at": Bson::Null })
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    if live >= u64::from(admission.live_limit) {
        return Err(AdmissionError::Domain(SandboxStoreError::LimitReached {
            limit: admission.live_limit,
            live: u16::try_from(live).unwrap_or(u16::MAX),
        }));
    }
    let mut sandbox = admission.sandbox.clone();
    sandbox.name = choose_name(store, session, owner, &admission.name).await?;
    let event = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: admission.event_at.clone(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandboxes
        .insert_one(
            admitted_sandbox_document(&sandbox, 1, &admission.admission_fingerprint)
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
    Ok(AdmissionReceipt::Created { sandbox, event })
}

async fn replay_in_transaction(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    session: &mut mongodb::ClientSession,
) -> Result<Option<Sandbox>, AdmissionError> {
    let Some(filter) = trigger_filter(admission) else {
        return Ok(None);
    };
    store
        .sandboxes
        .find_one(filter)
        .session(session)
        .await
        .map_err(AdmissionError::Driver)?
        .as_ref()
        .map(|document| classify_replay(admission, document).map_err(AdmissionError::Domain))
        .transpose()
}

async fn durable_replay(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
) -> Result<Option<Sandbox>, SandboxStoreError> {
    let Some(filter) = trigger_filter(admission) else {
        return Ok(None);
    };
    store
        .sandboxes
        .find_one(filter)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(|document| classify_replay(admission, document))
        .transpose()
}

fn trigger_filter(admission: &SandboxAdmission) -> Option<mongodb::bson::Document> {
    admission.trigger_launch_id().map(|launch_id| {
        doc! {
            "owner": admission.sandbox.owner.as_str(),
            "origin_type": "trigger",
            "origin_launch_id": launch_id,
        }
    })
}

fn classify_replay(
    admission: &SandboxAdmission,
    document: &mongodb::bson::Document,
) -> Result<Sandbox, SandboxStoreError> {
    let sandbox = read_sandbox(document)?;
    let fingerprint = read_admission_fingerprint(document, &sandbox)?.ok_or_else(|| {
        SandboxStoreError::Corrupt("trigger sandbox has no admission fingerprint".into())
    })?;
    if fingerprint == admission.admission_fingerprint {
        Ok(sandbox)
    } else {
        Err(SandboxStoreError::SandboxOriginConflict)
    }
}

async fn choose_name(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    owner: &str,
    requested: &SandboxAdmissionName,
) -> Result<String, AdmissionError> {
    match requested {
        SandboxAdmissionName::Exact(name) => {
            if name_exists(store, session, owner, name).await? {
                Err(AdmissionError::Domain(SandboxStoreError::Duplicate))
            } else {
                Ok(name.clone())
            }
        }
        SandboxAdmissionName::Allocate(base) => {
            for number in 1_u64.. {
                let candidate = suffixed(base, number);
                if !name_exists(store, session, owner, &candidate).await? {
                    return Ok(candidate);
                }
            }
            unreachable!("the numeric sandbox-name suffix space is unbounded")
        }
    }
}

async fn name_exists(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    owner: &str,
    name: &str,
) -> Result<bool, AdmissionError> {
    store
        .sandboxes
        .find_one(doc! { "owner": owner, "name": name, "retired_at": Bson::Null })
        .projection(doc! { "_id": 1 })
        .session(session)
        .await
        .map(|row| row.is_some())
        .map_err(AdmissionError::Driver)
}

fn suffixed(base: &str, number: u64) -> String {
    if number == 1 {
        return base.to_string();
    }
    let suffix = format!("-{number}");
    let maximum = 64_usize.saturating_sub(suffix.len());
    let mut end = base.len().min(maximum);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &base[..end], suffix)
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
            AdmissionReceipt::Created { sandbox, event } if current.id == sandbox.id => {
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
    let AdmissionReceipt::Created { sandbox, event } = receipt else {
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
mod tests {
    use std::time::Duration;

    use super::{admission_retry_delay, suffixed};

    #[test]
    fn retry_delay_is_exponential_and_bounded() {
        let delays = (1..=8).map(admission_retry_delay).collect::<Vec<_>>();
        assert_eq!(
            delays,
            [1, 2, 4, 8, 16, 32, 32, 32].map(Duration::from_millis)
        );
        assert_eq!(admission_retry_delay(usize::MAX), Duration::from_millis(32));
    }

    #[test]
    fn allocated_suffixes_preserve_the_name_limit_and_utf8() {
        assert_eq!(suffixed("research", 1), "research");
        assert_eq!(suffixed("research", 2), "research-2");
        let value = suffixed(&"é".repeat(40), 12);
        assert!(value.len() <= 64);
        assert!(value.ends_with("-12"));
    }
}
