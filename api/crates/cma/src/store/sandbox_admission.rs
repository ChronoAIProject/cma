//! Serialized Mongo admission for sandbox limits and live names.

use std::time::Duration;

use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus};
use mongodb::bson::{doc, Bson};

use super::sandbox_events::{event_document, read_event};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::{read_sandbox, sandbox_document};

const ADMISSION_TRANSACTION_ATTEMPTS: usize = 16;
const ADMISSION_RETRY_MAX_DELAY_MILLIS: u64 = 32;

enum AdmissionError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    Retry(mongodb::error::Error),
}

struct AdmissionReceipt {
    sandbox: Sandbox,
    event: SandboxEvent,
}

#[async_trait::async_trait]
impl SandboxAdmissionStore for MongoSandboxStore {
    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<Sandbox, SandboxStoreError> {
        validate(&admission)?;
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
                Err(AdmissionError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt.sandbox),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return if reconcile(self, &receipt).await? {
                        Ok(receipt.sandbox)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "sandbox admission commit remained unknown: {error}"
                        )))
                    };
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

fn validate(admission: &SandboxAdmission) -> Result<(), SandboxStoreError> {
    admission.sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("sandbox aggregate is invalid: {error}"))
    })?;
    if admission.live_limit == 0
        || admission.sandbox.revision != 1
        || admission.sandbox.status != SandboxStatus::Provisioning
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "sandbox admission requires a positive limit and fresh aggregate",
        ));
    }
    Ok(())
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
        .insert_one(sandbox_document(&sandbox, 1).map_err(AdmissionError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_insert)?;
    store
        .sandbox_events
        .insert_one(event_document(&event).map_err(AdmissionError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_insert)?;
    Ok(AdmissionReceipt { sandbox, event })
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
        AdmissionError::Domain(SandboxStoreError::Duplicate)
    } else {
        AdmissionError::Driver(error)
    }
}

async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &AdmissionReceipt,
) -> Result<bool, SandboxStoreError> {
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    let event = store
        .sandbox_events
        .find_one(doc! { "_id": format!("{}:{:020}", receipt.event.sandbox_id, 1) })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_event)
        .transpose()?;
    Ok(sandbox.as_ref() == Some(&receipt.sandbox) && event.as_ref() == Some(&receipt.event))
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
