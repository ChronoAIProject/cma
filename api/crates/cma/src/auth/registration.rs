//! Registration capacity is separate from authentication credential custody.

use std::sync::Arc;

use layer2_core::{StoreError, SubjectId};

use super::AuthState;

#[async_trait::async_trait]
pub(crate) trait RegistrationCapacity: Send + Sync {
    /// Tracking becomes durable at the first activation, independently of limits.
    async fn tracking_active(&self) -> Result<bool, StoreError> {
        Ok(true)
    }
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
    pub(crate) fn with_registration_capacity(
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

#[cfg(test)]
mod mongo_tests;
