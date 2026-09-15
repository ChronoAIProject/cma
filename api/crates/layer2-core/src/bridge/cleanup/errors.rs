use super::CleanupError;
use crate::resource_access::{AccessError, ResourceAccessError, ResourceAccessStoreError};

impl From<AccessError> for CleanupError {
    fn from(error: AccessError) -> Self {
        match error {
            AccessError::RevalidationRequired => Self::Unavailable,
            AccessError::NotFound
            | AccessError::Forbidden
            | AccessError::UnsafeInteractiveShare => Self::NotFound,
        }
    }
}
impl From<ResourceAccessError> for CleanupError {
    fn from(error: ResourceAccessError) -> Self {
        match error {
            ResourceAccessError::Access(error) => error.into(),
            ResourceAccessError::Membership(_) => Self::Unavailable,
            ResourceAccessError::Store(error) => match error {
                ResourceAccessStoreError::NotFound
                | ResourceAccessStoreError::UnsafeInteractiveShare => Self::NotFound,
                ResourceAccessStoreError::Conflict
                | ResourceAccessStoreError::IdempotencyConflict => Self::Conflict,
                ResourceAccessStoreError::Corrupt | ResourceAccessStoreError::Invalid => {
                    Self::Corrupt
                }
                ResourceAccessStoreError::Unavailable | ResourceAccessStoreError::Capacity => {
                    Self::Unavailable
                }
            },
        }
    }
}
