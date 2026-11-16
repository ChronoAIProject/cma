use super::*;
use layer2_core::sandbox::store::{
    Page as SandboxPage, PageRequest, SandboxImageRotationRequest, SandboxInactivityPauseRequest,
    SandboxMaintenanceOutcome, SandboxMaintenanceStore, SandboxStoreError,
};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Maintenance {
    candidate_failure: Option<SandboxStoreError>,
    preview_failure: Option<SandboxStoreError>,
    reads: AtomicUsize,
}

#[async_trait]
impl SandboxMaintenanceStore for Maintenance {
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<SandboxPage<SandboxId>, SandboxStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.candidate_failure {
            return Err(error.clone());
        }
        Ok(if page.cursor.is_none() {
            SandboxPage {
                items: vec![SandboxId("candidate".into())],
                next: Some("next".into()),
            }
        } else {
            assert_eq!(page.cursor.as_deref(), Some("next"));
            SandboxPage {
                items: vec![SandboxId("active".into())],
                next: None,
            }
        })
    }

    async fn preview_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.preview_failure {
            return Err(error.clone());
        }
        Ok(if request.sandbox_id.as_str() == "candidate" {
            SandboxMaintenanceOutcome::Scheduled
        } else {
            SandboxMaintenanceOutcome::NotEligible
        })
    }

    async fn schedule_inactivity_pause(
        &self,
        _: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        panic!("preview must not schedule a pause")
    }

    async fn set_sandbox_always_on(
        &self,
        _: SandboxId,
        _: u64,
        _: bool,
    ) -> Result<Sandbox, SandboxStoreError> {
        panic!("preview must not change policy")
    }

    async fn schedule_image_rotation(
        &self,
        _: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        panic!("preview must not rotate a runtime")
    }
}

struct Settings(bool);

#[async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        if self.0 {
            Err(StoreError::Unavailable("private-driver-payload".into()))
        } else {
            Ok(StoredPlatformSettings {
                revision: 0,
                settings: PlatformSettings::default(),
            })
        }
    }

    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        panic!("preview must not write settings")
    }
}

async fn preview(
    maintenance: Option<Arc<Maintenance>>,
    settings_failure: bool,
    authorized: bool,
) -> (StatusCode, Value, Vec<Value>) {
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _scope = tracing::subscriber::set_default(subscriber);
    let activity = ActivityService::new(Arc::new(MemoryActivity::default()));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(Settings(settings_failure)),
        activity.clone(),
    ));
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let operators = Arc::new(SandboxOperatorService::new(
        sandboxes.clone(),
        Arc::new(InMemoryProfileStore::default()),
        Arc::new(StoredSandboxRetirement::new(sandboxes)),
        activity.clone(),
    ));
    let mut state = AdminState::new(settings, operators, activity);
    if let Some(maintenance) = maintenance {
        state = state.with_maintenance(maintenance);
    }
    let response = send(
        &router(state),
        Method::GET,
        "/api/v1/admin/auto-pause/preview",
        None,
        authorized.then_some(Capability::ConfigurePlatform),
    )
    .await;
    let status = response.status();
    let body = response_json(response).await;
    let bytes = output.0.lock().unwrap();
    let logs = std::str::from_utf8(&bytes).unwrap();
    assert!(!logs.contains("private-driver-payload"));
    assert!(!body.to_string().contains("private-driver-payload"));
    let records = logs
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (status, body, records)
}

#[tokio::test]
async fn preview_reports_the_failed_dependency_without_exposing_store_payloads() {
    let cases = [
        (None, false, "maintenance", "not_configured"),
        (
            Some(Arc::new(Maintenance::default())),
            true,
            "settings",
            "runtime_unavailable",
        ),
        (
            Some(Arc::new(Maintenance {
                candidate_failure: Some(SandboxStoreError::Unavailable(
                    "private-driver-payload".into(),
                )),
                ..Default::default()
            })),
            false,
            "candidates",
            "store_unavailable",
        ),
        (
            Some(Arc::new(Maintenance {
                preview_failure: Some(SandboxStoreError::Conflict),
                ..Default::default()
            })),
            false,
            "activity_preview",
            "revision_conflict",
        ),
        (
            Some(Arc::new(Maintenance {
                preview_failure: Some(SandboxStoreError::Corrupt("private-driver-payload".into())),
                ..Default::default()
            })),
            false,
            "activity_preview",
            "corrupt_data",
        ),
    ];
    for (store, settings_failure, stage, failure) in cases {
        let (status, body, records) = preview(store, settings_failure, true).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "operator_service_unavailable");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["target"], "cma::admin::dependencies");
        assert_eq!(records[0]["level"], "WARN");
        let fields = &records[0]["fields"];
        assert_eq!(fields["operation"], "auto_pause_preview");
        assert_eq!(fields["stage"], stage);
        assert_eq!(fields["failure"], failure);
        assert_eq!(fields.as_object().unwrap().len(), 4);
    }
}

#[tokio::test]
async fn preview_preserves_pagination_read_only_success_and_early_authorization() {
    let store = Arc::new(Maintenance::default());
    let (status, body, records) = preview(Some(store.clone()), false, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "candidates": 1 }));
    assert_eq!(store.reads.load(Ordering::SeqCst), 4);
    assert!(records.is_empty());
    let (status, _, records) = preview(Some(store.clone()), true, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(store.reads.load(Ordering::SeqCst), 4);
    assert!(records.is_empty());
}
