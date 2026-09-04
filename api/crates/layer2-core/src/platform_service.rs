//! Authorization and optimistic-concurrency boundary for platform settings.

use std::sync::Arc;

use crate::{Caller, Capability, CoreError, PlatformSettings, StoreError};

/// Settings paired with their durable optimistic-concurrency revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlatformSettings {
    pub revision: u64,
    pub settings: PlatformSettings,
}

#[async_trait::async_trait]
pub trait PlatformSettingsStore: Send + Sync {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError>;
    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError>;
}

pub struct PlatformSettingsService {
    store: Arc<dyn PlatformSettingsStore>,
}

impl PlatformSettingsService {
    pub fn new(store: Arc<dyn PlatformSettingsStore>) -> Self {
        Self { store }
    }

    pub async fn read(&self, caller: &Caller) -> Result<StoredPlatformSettings, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.store.read().await.map_err(store_failed)
    }

    pub async fn replace(
        &self,
        caller: &Caller,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        settings.validate()?;
        self.store
            .replace(expected_revision, settings)
            .await
            .map_err(store_failed)
    }
}

fn store_failed(error: StoreError) -> CoreError {
    match error {
        StoreError::RevisionConflict { expected, actual } => CoreError::InvalidState(format!(
            "platform settings changed: expected revision {expected}, found {actual}"
        )),
        error => CoreError::RuntimeUnavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{Authorization, SubjectId};

    struct MemoryStore(Mutex<StoredPlatformSettings>);

    #[async_trait::async_trait]
    impl PlatformSettingsStore for MemoryStore {
        async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
            Ok(self.0.lock().unwrap().clone())
        }

        async fn replace(
            &self,
            expected_revision: u64,
            settings: &PlatformSettings,
        ) -> Result<StoredPlatformSettings, StoreError> {
            let mut stored = self.0.lock().unwrap();
            if stored.revision != expected_revision {
                return Err(StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: stored.revision,
                });
            }
            *stored = StoredPlatformSettings {
                revision: expected_revision + 1,
                settings: settings.clone(),
            };
            Ok(stored.clone())
        }
    }

    fn service() -> PlatformSettingsService {
        PlatformSettingsService::new(Arc::new(MemoryStore(Mutex::new(StoredPlatformSettings {
            revision: 0,
            settings: PlatformSettings::default(),
        }))))
    }

    #[tokio::test]
    async fn requires_platform_authority_and_preserves_revision_cas() {
        let denied = Caller::new(SubjectId::new("subject-a"), Authorization::none());
        assert_eq!(service().read(&denied).await, Err(CoreError::Forbidden));

        let caller = Caller::new(
            SubjectId::new("subject-a"),
            Authorization::new([Capability::ConfigurePlatform]),
        );
        let service = service();
        let replaced = service
            .replace(&caller, 0, &PlatformSettings::default())
            .await
            .unwrap();
        assert_eq!(replaced.revision, 1);
        assert!(matches!(
            service
                .replace(&caller, 0, &PlatformSettings::default())
                .await,
            Err(CoreError::InvalidState(_))
        ));
    }
}
