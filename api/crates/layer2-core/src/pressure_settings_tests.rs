//! Memory-pressure policy in the deployment settings document.

use std::sync::Arc;

use crate::store::{PlatformSettingsStore, StoreError, StoredSettings};
use crate::{MemoryPressurePolicy, PlatformSettings, PlatformSettingsService};

#[derive(Clone)]
struct StoredDocument(serde_json::Value);

#[async_trait::async_trait]
impl PlatformSettingsStore for StoredDocument {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        let settings = serde_json::from_value(self.0.clone())
            .map_err(|error| StoreError::Malformed(format!("stored platform settings: {error}")))?;
        Ok(Some(StoredSettings {
            settings,
            revision: 1,
            previous: None,
        }))
    }

    async fn save_if_unchanged(
        &self,
        _expected: u64,
        _settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable(
            "this fixture does not write settings".into(),
        ))
    }
}

fn narrow_band_document() -> serde_json::Value {
    serde_json::json!({
        "memory_pressure": {
            "enter_permille": 800,
            "exit_permille": 780,
            "enter_samples": 5,
            "exit_samples": 15,
            "settle_seconds": 120
        }
    })
}

#[tokio::test]
async fn a_stored_narrow_band_falls_back_to_the_shipped_settings() {
    let document = narrow_band_document();
    assert!(
        serde_json::from_value::<PlatformSettings>(document.clone()).is_err(),
        "the stored document itself must fail to deserialize"
    );
    let service = PlatformSettingsService::new(Arc::new(StoredDocument(document)));
    assert_eq!(
        service.for_provisioning().await,
        PlatformSettings::default()
    );
}

#[test]
fn an_older_settings_document_receives_the_shipped_pressure_policy() {
    let settings: PlatformSettings = serde_json::from_value(serde_json::json!({
        "live_sessions": 3
    }))
    .expect("an older document remains readable");
    assert_eq!(settings.memory_pressure, MemoryPressurePolicy::default());
}

#[test]
fn the_policy_is_a_named_serialized_setting() {
    let settings = PlatformSettings {
        memory_pressure: MemoryPressurePolicy::new(850, 650, 6, 18, 180).expect("valid policy"),
        ..PlatformSettings::default()
    };
    let value = serde_json::to_value(&settings).expect("settings serialize");
    assert_eq!(value["memory_pressure"]["enter_permille"], 850);
    assert_eq!(value["memory_pressure"]["settle_seconds"], 180);
    assert_eq!(
        serde_json::from_value::<PlatformSettings>(value).expect("settings deserialize"),
        settings
    );
}
