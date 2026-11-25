//! Explicit lifecycle intents use an additive ledger. A terminal receipt and the
//! existing model's lifecycle transition/event commit together; no runtime IO is
//! performed here. Restarted workers cannot apply the same intent twice.
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::ids::SandboxCommandId;
use layer2_core::sandbox::control::*;
use layer2_core::sandbox::store::{Page, PageRequest};
use mongodb::bson::doc;
use std::sync::Arc;
pub(super) mod records;
mod transaction;
pub(super) use records::ensure_indexes;

enum Change<'a> {
    Reserve(
        &'a SandboxControlRecord,
        &'a SandboxControlAuthorization,
        Result<(), layer2_core::admission::AdmissionRefusal>,
    ),
    Apply(&'a SandboxControlRecord, &'a SandboxControlAuthorization),
    Reject(&'a SandboxControlRecord, SandboxControlFailure),
}
impl Change<'_> {
    fn require_current(&self) -> Result<(), SandboxControlError> {
        match self {
            Self::Reserve(_, proof, _) | Self::Apply(_, proof) => proof
                .require_current()
                .map_err(|_| SandboxControlError::Conflict),
            Self::Reject(..) => Ok(()),
        }
    }
}

enum ApplyError {
    Domain(SandboxControlError),
    Driver(mongodb::error::Error),
}
impl From<SandboxControlError> for ApplyError {
    fn from(value: SandboxControlError) -> Self {
        Self::Domain(value)
    }
}
impl From<mongodb::error::Error> for ApplyError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<super::resource_access::ReadError> for ApplyError {
    fn from(value: super::resource_access::ReadError) -> Self {
        use layer2_core::resource_access::ResourceAccessStoreError;
        match value {
            super::resource_access::ReadError::Driver(error) => Self::Driver(error),
            super::resource_access::ReadError::Domain(error) => Self::Domain(match error {
                ResourceAccessStoreError::NotFound => SandboxControlError::NotFound,
                ResourceAccessStoreError::Corrupt => SandboxControlError::Corrupt,
                ResourceAccessStoreError::Unavailable => SandboxControlError::Unavailable,
                _ => SandboxControlError::Conflict,
            }),
        }
    }
}

#[async_trait::async_trait]
impl SandboxControlStore for MongoSandboxStore {
    async fn reserve_sandbox_control(
        &self,
        record: SandboxControlRecord,
        authorization: Arc<SandboxControlAuthorization>,
        new_commitment: Result<(), layer2_core::admission::AdmissionRefusal>,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        self.change_sandbox_control(Change::Reserve(&record, &authorization, new_commitment))
            .await
    }
    async fn sandbox_control(
        &self,
        id: &SandboxCommandId,
    ) -> Result<Option<SandboxControlRecord>, SandboxControlError> {
        self.sandbox_controls
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|_| SandboxControlError::Unavailable)?
            .as_ref()
            .map(records::read)
            .transpose()
    }
    async fn queued_sandbox_controls(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxControlRecord>, SandboxControlError> {
        PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| SandboxControlError::Invalid)?;
        let mut filter = doc! { "queued": true };
        if self
            .empty_discovery
            .empty(
                super::empty_discovery::Kind::Controls,
                &self.sandbox_controls,
                filter.clone(),
            )
            .await
        {
            return Ok(Page {
                items: Vec::new(),
                next: None,
            });
        }
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandbox_controls
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|_| SandboxControlError::Unavailable)?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|_| SandboxControlError::Unavailable)?
        {
            items.push(records::read(&document)?);
        }
        self.empty_discovery
            .observed(super::empty_discovery::Kind::Controls, items.is_empty());
        let more = items.len() > usize::from(page.limit);
        items.truncate(usize::from(page.limit));
        let next = more.then(|| {
            items
                .last()
                .expect("positive page")
                .envelope
                .command_id
                .to_string()
        });
        Ok(Page { items, next })
    }
    async fn apply_sandbox_control(
        &self,
        record: &SandboxControlRecord,
        authorization: Arc<SandboxControlAuthorization>,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        let result = self
            .change_sandbox_control(Change::Apply(record, &authorization))
            .await?;
        if matches!(result.status, SandboxControlStatus::Applied { .. }) {
            self.sandbox_admitted.notify_one();
        }
        Ok(result)
    }
    async fn reject_sandbox_control(
        &self,
        record: &SandboxControlRecord,
        reason: SandboxControlFailure,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        self.change_sandbox_control(Change::Reject(record, reason))
            .await
    }
}

impl MongoSandboxStore {
    async fn change_sandbox_control(
        &self,
        change: Change<'_>,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            change.require_current()?;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| SandboxControlError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| SandboxControlError::Unavailable)?;
            let result = match transaction::apply(self, &change, &mut session).await {
                Ok(result) => result,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if is_transient(&error) || is_duplicate_key(&error) {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    return Err(SandboxControlError::Unavailable);
                }
            };
            if let Err(error) = change.require_current() {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            // Authorization and queue lifetime include all transaction IO. An
            // unknown commit is recovered from this immutable command identity.
            if matches!(change, Change::Apply(..))
                && matches!(result.status, SandboxControlStatus::Applied { .. })
                && result.expired(&layer2_core::now_rfc3339())?
            {
                let _ = session.abort_guarded().await;
                return Err(SandboxControlError::Conflict);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(SandboxControlError::Unavailable)
                }
            }
        }
        Err(SandboxControlError::Conflict)
    }
}

/// A queued retire keeps its original admission budget after atomic application.
/// Exact application time joins the retained receipt to the Retiring event.
pub(super) async fn retirement_admission_at(
    store: &MongoSandboxStore,
    id: &layer2_core::SandboxId,
    applied_at: &str,
) -> Result<Option<String>, layer2_core::sandbox::store::SandboxStoreError> {
    use layer2_core::sandbox::store::SandboxStoreError;
    let row = store
        .sandbox_controls
        .find_one(doc! {
            "sandbox_id": id.as_str(), "queued": false,
            "record.envelope.command": "retire", "record.status.status": "applied",
            "record.status.disposition": "scheduled", "record.status.at": applied_at,
        })
        .max_time(std::time::Duration::from_secs(2))
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    row.as_ref()
        .map(records::read)
        .transpose()
        .map(|record| record.map(|record| record.admitted_at))
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}
