//! Fresh lifecycle operations check majority control before the original
//! single-document insert. Recovery never repeats this admission check.

use super::*;
use layer2_core::sandbox::store::SandboxStoreError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LifecycleCreateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
}

impl MongoAgentLifecycleStore {
    pub(super) async fn finish_create(
        &self,
        record: LifecycleRecord,
    ) -> Result<CreateResult, LifecycleCreateError> {
        match super::super::admission_checks::require_fresh_read(&self.admission_settings).await {
            Ok(()) => {}
            Err(SandboxStoreError::AdmissionPaused) => {
                // A concurrent exact writer can commit after the outer replay
                // lookup but before the stop. Its operation keeps authority.
                return self
                    .replay(
                        &SubjectId::new(&record.owner),
                        &record.key,
                        &record.input.request,
                    )
                    .await?
                    .map(|existing| CreateResult::Existing(Box::new(existing)))
                    .ok_or(LifecycleCreateError::AdmissionPaused);
            }
            Err(error) => return Err(StoreError::Unavailable(error.to_string()).into()),
        }
        match self.operations.insert_one(&record).await {
            Ok(_) => Ok(CreateResult::Created(record.id)),
            Err(error) if is_duplicate_key(&error) => self
                .replay(
                    &SubjectId::new(&record.owner),
                    &record.key,
                    &record.input.request,
                )
                .await?
                .map(|existing| CreateResult::Existing(Box::new(existing)))
                .ok_or_else(|| {
                    StoreError::Unavailable("lifecycle operation raced out of view".into()).into()
                }),
            Err(error) => Err(driver(error).into()),
        }
    }
}
