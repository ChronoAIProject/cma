use async_trait::async_trait;
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityStore, Page, PlatformSettings, PlatformSettingsStore,
    StoreError, StoredPlatformSettings,
};

pub(super) struct UnavailableSettings;

#[async_trait]
impl PlatformSettingsStore for UnavailableSettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Err(StoreError::Unavailable(
            "contract settings unavailable".into(),
        ))
    }

    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        Err(StoreError::Unavailable(
            "contract settings unavailable".into(),
        ))
    }
}

pub(super) struct UnavailableActivity;

#[async_trait]
impl ActivityStore for UnavailableActivity {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        Err(StoreError::Unavailable(
            "contract activity unavailable".into(),
        ))
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "contract activity unavailable".into(),
        ))
    }
}
