//! Registration capacity is separate from authentication credential custody.

use std::sync::Arc;

use layer2_core::budget::{BudgetError, PlatformAdmissionState, PlatformSpendLedger};
use layer2_core::{StoreError, SubjectId};

use super::AuthState;

pub type RegistrationAdmission = (
    PlatformAdmissionState,
    Result<layer2_core::admission::AdmissionCeilings, ()>,
);

#[async_trait::async_trait]
pub trait RegistrationPolicy: Send + Sync {
    async fn admission_snapshot(&self, now: u64) -> Result<RegistrationAdmission, BudgetError>;
}

pub(super) struct SpendRegistrationPolicy(pub Arc<dyn PlatformSpendLedger>);

#[async_trait::async_trait]
impl RegistrationPolicy for SpendRegistrationPolicy {
    async fn admission_snapshot(&self, now: u64) -> Result<RegistrationAdmission, BudgetError> {
        self.0.admission_snapshot(now).await
    }
}

#[async_trait::async_trait]
pub trait RegistrationCapacity: Send + Sync {
    /// Includes an exact registration replay and retained existing consumers.
    async fn existing_subject(&self, subject: &SubjectId) -> Result<bool, StoreError>;
    /// Atomically claim one public registration, or refuse without side effects.
    async fn admit(&self, subject: &SubjectId) -> Result<bool, StoreError>;
    /// Advisory only; the callback remains the admission authority.
    async fn available(&self) -> Result<bool, StoreError>;
    async fn available_with_limit(
        &self,
        _limit: Result<Option<u32>, ()>,
    ) -> Result<bool, StoreError> {
        self.available().await
    }
    async fn admit_with_limit(
        &self,
        subject: &SubjectId,
        _limit: Result<Option<u32>, ()>,
    ) -> Result<bool, StoreError> {
        self.admit(subject).await
    }
}

impl AuthState {
    pub fn with_registration_capacity(
        mut self: Arc<Self>,
        capacity: Arc<dyn RegistrationCapacity>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("registration capacity is composed before sharing authentication")
            .registration_capacity = Some(capacity);
        self
    }
}

#[cfg(test)]
mod tests;
