use super::{
    journal_fixture::{definition, Fixture},
    status::{FailureCode, Stage, StatusAttempt, StatusRegistry},
    DurableJournal, GatewayError, NativeDiscordJournal,
};
use std::{sync::Arc, time::Duration};

async fn acquire(f: &Fixture, status: StatusAttempt) -> Result<Arc<DurableJournal>, GatewayError> {
    DurableJournal::acquire_store_observed(
        f.store.clone(),
        &f.record,
        f.api.clone(),
        Arc::new(crate::turns::observer::NoopObserver),
        status.clone(),
    )
    .await
    .inspect_err(|error| status.finish((*error).into(), false, Duration::from_secs(30)))
}
#[tokio::test]
async fn acquisition_authorization_and_contention_are_visible_without_claiming_ready() {
    let f = Fixture::new().await;
    let registry = StatusRegistry::default();
    let first = registry.begin(&f.record, Some("revision"));
    let journal = acquire(&f, first.clone()).await.unwrap();
    assert_eq!(registry.view(&f.record, Some("revision")).lease, "acquired");
    journal.authorize_connection(None).await.unwrap();
    let view = registry.view(&f.record, Some("revision"));
    assert_eq!(view.identify, "reserved");
    assert_eq!(view.authenticated, None);
    let contender = registry.begin(&f.record, Some("revision"));
    assert!(matches!(
        acquire(&f, contender).await,
        Err(GatewayError::Authority)
    ));
    let view = registry.view(&f.record, Some("revision"));
    assert_eq!(view.last_failure_stage, Some(Stage::Lease));
    assert_eq!(view.last_failure_code, Some(FailureCode::LeaseContended));
    assert_eq!(view.lease, "contended");
    journal.renew().await.unwrap(); // An older attempt cannot overwrite the newer observation.
    assert_eq!(
        registry.view(&f.record, Some("revision")).lease,
        "contended"
    );
}
#[tokio::test]
async fn discovery_refusals_are_specific_and_do_not_acquire_a_lease() {
    for expected in [
        FailureCode::DiscoveryUnauthorized,
        FailureCode::BotIdentityMismatch,
        FailureCode::ShardingUnsupported,
    ] {
        let f = Fixture::new().await;
        match expected {
            FailureCode::DiscoveryUnauthorized => {
                let mut d = definition();
                d.secrets.discord_bot_token =
                    Some(cmaeg_core::channel::Secret::new("different-fixture"));
                f.replace(&d);
            }
            FailureCode::BotIdentityMismatch => f.memory.user.lock().unwrap()["id"] = "99".into(),
            _ => f.memory.gateway.lock().unwrap()["shards"] = 2.into(),
        }
        let registry = StatusRegistry::default();
        assert!(acquire(&f, registry.begin(&f.record, Some("revision")))
            .await
            .is_err());
        let view = registry.view(&f.record, Some("revision"));
        assert_eq!(view.last_failure_stage, Some(Stage::Discovery));
        assert_eq!(view.last_failure_code, Some(expected));
        assert_eq!(view.identify, "not_attempted");
        assert!(f.memory.rows.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn quota_refusal_does_not_claim_identify_was_sent() {
    let f = Fixture::new().await;
    f.memory.gateway.lock().unwrap()["session_start_limit"]["remaining"] = 0.into();
    let registry = StatusRegistry::default();
    let status = registry.begin(&f.record, Some("revision"));
    let journal = acquire(&f, status.clone()).await.unwrap();
    let error = journal.authorize_connection(None).await.unwrap_err();
    status.finish(error.into(), false, Duration::from_secs(60));
    let view = registry.view(&f.record, Some("revision"));
    assert_eq!(
        view.last_failure_code,
        Some(FailureCode::IdentifyBudgetExhausted)
    );
    assert_eq!(view.last_failure_stage, Some(Stage::Authorize));
    assert_eq!(view.identify, "not_attempted");
    assert!(view.retry_at.is_some());
}
