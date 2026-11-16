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
        cloud: None,
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

#[tokio::test]
async fn saving_cloud_fields_is_refused_until_the_platform_spend_gate_opens() {
    let (service, store, activity) = service();
    let service = service
        .with_model_budget_writes_enabled(true)
        .with_model_request_budget_writes_enabled(true);
    let operator = caller([Capability::ConfigurePlatform]);
    let mut platform = crate::budget::PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
        cloud: Some(crate::budget::CloudSpendPolicy {
            run_rate_units_per_day: 1,
            max_observation_age_seconds: 3_600,
        }),
    };
    let mut configured = PlatformSettings::default();
    configured.model_budget.platform = Some(platform.clone());
    assert!(!service.platform_spend_writes_enabled());
    assert_eq!(
        service.configure_platform(&operator, 0, &configured).await,
        Err(CoreError::ModelBudgetWritesUnavailable)
    );
    assert_eq!(store.read().await.unwrap().revision, 0);
    assert!(activity.records.lock().unwrap().is_empty());
    platform.cloud = None;
    configured.model_budget.platform = Some(platform.clone());
    assert_eq!(
        service
            .configure_platform(&operator, 0, &configured)
            .await
            .unwrap()
            .revision,
        1
    );
    // The platform spend gate never opens without the foundation gate.
    let (closed, _, _) = super::service();
    let closed = closed.with_platform_spend_writes_enabled(true);
    assert!(!closed.platform_spend_writes_enabled());
    let service = service.with_platform_spend_writes_enabled(true);
    assert!(service.platform_spend_writes_enabled());
    platform.cloud = Some(crate::budget::CloudSpendPolicy {
        run_rate_units_per_day: 1,
        max_observation_age_seconds: 3_600,
    });
    configured.model_budget.platform = Some(platform);
    let saved = service
        .configure_platform(&operator, 1, &configured)
        .await
        .unwrap();
    assert_eq!(saved.settings, configured);
}
