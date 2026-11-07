use super::*;
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Page, PlatformSettings,
    PlatformSettingsService, PlatformSettingsStore, StoreError, StoredPlatformSettings,
};
use std::sync::{Arc, Mutex};

struct Settings(Mutex<Option<StoredPlatformSettings>>);

#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| StoreError::Unavailable("fixture".into()))
    }

    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        unreachable!("reconciliation only reads settings")
    }
}

struct Activity;

#[async_trait::async_trait]
impl ActivityStore for Activity {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        unreachable!("reconciliation only reads settings")
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!("reconciliation does not read activity content")
    }
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_566_400)
}

fn enable(settings: &Settings, at: SystemTime, threshold: u64) {
    let mut stored = settings.0.lock().unwrap();
    let inactivity = &mut stored.as_mut().unwrap().settings.inactivity;
    inactivity.auto_pause_enabled = true;
    inactivity.grace_started_at = Some(rfc3339_at(at));
    inactivity.pause_after_seconds = threshold;
}

async fn live() -> (Arc<fixtures::Store>, SandboxReconciler, Arc<Settings>) {
    let (store, mut reconciler) = setup(3, Duration::ZERO).await;
    let settings = Arc::new(Settings(Mutex::new(Some(StoredPlatformSettings {
        revision: 1,
        settings: PlatformSettings::default(),
    }))));
    reconciler.policy = Arc::new(crate::sandbox::policy::LiveSandboxPolicy::new(
        Arc::new(PlatformSettingsService::new(
            settings.clone(),
            ActivityService::new(Arc::new(Activity)),
        )),
        vec![],
    ));
    (store, reconciler, settings)
}

#[tokio::test(start_paused = true)]
async fn disabled_and_grace_sweeps_finish_without_scanning_pause_candidates() {
    let (store, reconciler, settings) = live().await;
    let at = now();
    assert!(reconciler.run_due(at).await.unwrap());
    assert!(!store.keys("authority").is_empty());
    assert!(store.keys("candidates").is_empty());
    assert!(store.keys("pause").is_empty());
    enable(&settings, at, 900);
    assert!(reconciler
        .run_due(at + Duration::from_secs(899))
        .await
        .unwrap());
    assert!(store.keys("candidates").is_empty());
    assert!(reconciler.progress.lock().await.is_none());

    let eligible_at = at + Duration::from_secs(900);
    assert!(reconciler.run_due(eligible_at).await.unwrap());
    let pauses = store.pauses.lock().unwrap();
    assert_eq!(pauses.len(), 3);
    assert!(pauses.iter().all(|request| request.auto_pause_enabled
        && request.pause_after_seconds == 900
        && request.requested_at == rfc3339_at(eligible_at)));
}

#[tokio::test(start_paused = true)]
async fn resumed_idle_phase_honors_disabling_and_restarted_grace() {
    for disable in [true, false] {
        let (store, reconciler, settings) = live().await;
        let at = now();
        enable(&settings, at - Duration::from_secs(900), 900);
        store.fail("pause", "sbx_001", Failure::Deadline);
        assert!(reconciler.run_due(at).await.is_err());
        assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
        if disable {
            settings
                .0
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .settings
                .inactivity
                .auto_pause_enabled = false;
        } else {
            enable(&settings, at, 1_800);
        }
        assert!(reconciler
            .run_due(at + Duration::from_secs(15))
            .await
            .unwrap());
        assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
        assert_eq!(store.pauses.lock().unwrap().len(), 1);
        assert!(reconciler.progress.lock().await.is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn resumed_idle_phase_reads_new_threshold_and_retries_unavailable_policy() {
    let (store, reconciler, settings) = live().await;
    let at = now();
    enable(&settings, at - Duration::from_secs(900), 900);
    store.fail("pause", "sbx_001", Failure::Deadline);
    assert!(reconciler.run_due(at).await.is_err());
    let retained = settings.0.lock().unwrap().take().unwrap();
    assert!(reconciler.run_due(at).await.is_err());
    assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .phase,
        Phase::IdlePause
    );

    *settings.0.lock().unwrap() = Some(retained);
    enable(&settings, at, 1_800);
    let resumed_at = at + Duration::from_secs(1_800);
    assert!(reconciler.run_due(resumed_at).await.unwrap());
    // Each retained pause attempt refreshes bindings first, including the one
    // whose policy is unavailable. Neither refresh repeats a pause admission.
    assert_eq!(store.keys("authority"), ["start", "sbx_001"].repeat(3));
    assert_eq!(
        store.keys("pause"),
        ["sbx_000", "sbx_001", "sbx_001", "sbx_002"]
    );
    let pauses = store.pauses.lock().unwrap();
    assert_eq!(pauses.len(), 3);
    assert_eq!(pauses[0].pause_after_seconds, 900);
    assert!(pauses[1..].iter().all(|request| request.auto_pause_enabled
        && request.pause_after_seconds == 1_800
        && request.requested_at == rfc3339_at(resumed_at)));
}
