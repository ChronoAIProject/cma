use super::*;

#[tokio::test]
async fn budget_writer_defaults_closed_and_refuses_before_any_store_write() {
    let (service, store, activity) = service();
    let operator = caller([Capability::ConfigurePlatform]);
    assert!(!service.model_budget_writes_enabled());
    let mut configured = PlatformSettings::default();
    configured.model_budget.platform = Some(crate::budget::PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
    });
    assert!(!configured.model_budget.enabled);
    assert_eq!(
        service.configure_platform(&operator, 0, &configured).await,
        Err(CoreError::ModelBudgetWritesUnavailable)
    );
    assert_eq!(store.read().await.unwrap().revision, 0);
    assert!(activity.records.lock().unwrap().is_empty());
    let service = service.with_model_budget_writes_enabled(true);
    let saved = service
        .configure_platform(&operator, 0, &configured)
        .await
        .unwrap();
    assert_eq!(saved.settings, configured);
    assert_eq!(saved.revision, 1);
    assert!(matches!(
        service.configure_platform(&operator, 0, &configured).await,
        Err(CoreError::InvalidState(_))
    ));
}
