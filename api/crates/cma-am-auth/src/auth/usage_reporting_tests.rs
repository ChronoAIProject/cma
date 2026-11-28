use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer1_public::AuthenticatedSubject;
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Capability, Page,
    PlatformSettings, PlatformSettingsStore, StoreError, StoredPlatformSettings, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::auth::AuthMode;

#[derive(Default)]
struct Settings {
    visible: AtomicBool,
    unavailable: AtomicBool,
    reads: Mutex<usize>,
}
#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        *self.reads.lock().unwrap() += 1;
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("private storage failure".into()));
        }
        let mut settings = PlatformSettings::default();
        settings.usage_reporting.measured_visible = self.visible.load(Ordering::SeqCst);
        Ok(StoredPlatformSettings {
            changes: Default::default(),
            revision: 1,
            settings,
        })
    }
    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        panic!("session reads must never write settings")
    }
}
struct NoAudit;
#[async_trait::async_trait]
impl ActivityStore for NoAudit {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        panic!("session reads must not enter the admin audit path")
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("session reads must not read audit records")
    }
}
fn fixture() -> (Router, Arc<Settings>) {
    let settings = Arc::new(Settings::default());
    let service = Arc::new(PlatformSettingsService::new(
        settings.clone(),
        ActivityService::new(Arc::new(NoAudit)),
    ));
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "unused".into(),
        },
        None,
    )
    .with_platform_settings(service);
    (crate::auth::router(auth), settings)
}
async fn session(app: &Router, grants: Option<Vec<Capability>>) -> (StatusCode, Value) {
    let mut request = Request::get("/auth/session").body(Body::empty()).unwrap();
    if let Some(grants) = grants {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new("session-caller")));
        request.extensions_mut().insert(Authorization::new(grants));
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private storage failure"));
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn every_authenticated_role_reads_live_usage_presentation_and_failures_are_closed() {
    let (app, settings) = fixture();
    for visible in [false, true, false] {
        settings.visible.store(visible, Ordering::SeqCst);
        for grants in [
            vec![Capability::UseOwnSandboxes],
            vec![Capability::ConfigurePlatform],
            vec![Capability::LaunchTriggers],
            vec![],
        ] {
            let (status, body) = session(&app, Some(grants)).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body["usage_reporting"],
                json!({"measured_visible": visible})
            );
            assert!(body.get("settings").is_none());
        }
    }
    assert_eq!(*settings.reads.lock().unwrap(), 12);
    settings.unavailable.store(true, Ordering::SeqCst);
    let (status, body) = session(&app, Some(vec![Capability::UseOwnSandboxes])).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "settings_unavailable");
    assert!(body.get("retryable").is_none());
    assert!(body.get("usage_reporting").is_none());
    assert_eq!(
        session(&app, None).await,
        (
            StatusCode::UNAUTHORIZED,
            json!({"authenticated": false, "silent_sign_in": false})
        )
    );
    assert_eq!(*settings.reads.lock().unwrap(), 13);
}
