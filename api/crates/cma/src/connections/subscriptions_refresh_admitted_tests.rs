use super::*;
use crate::connections::{
    subscription_store::SubscriptionStore,
    subscriptions::{NativeLogin, NativeSubscriptionAuth, REFRESH_BUDGET},
};
use futures_util::FutureExt;
use layer3_adapter::ingress_budget::{IngressBudget, IngressWorkScope};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering::SeqCst},
        Arc, Mutex,
    },
    time::SystemTime,
};
use tokio::sync::Semaphore;

#[path = "subscriptions_refresh_admitted_fixture.rs"]
mod fixture;
use fixture::*;
#[path = "subscriptions_refresh_admitted_domains_tests.rs"]
mod domains;
#[path = "subscriptions_refresh_admitted_lifetime_tests.rs"]
mod lifetime;

#[tokio::test]
async fn usable_cache_and_owned_previous_account_preserve_decisions() {
    let f = Fixture::new(3600);
    let tokens = f.tokens(false, None).await.unwrap().unwrap();
    assert_eq!(tokens.account_id, "account");
    assert_eq!(f.native.calls(), 0);
    let previous = own_string(&f.policy, "other");
    assert_eq!(
        f.tokens(false, Some(&previous)).await.unwrap().unwrap_err(),
        SubscriptionError::SignInRequired
    );
    let previous = own_string(&f.policy, "account");
    f.tokens(true, Some(&previous)).await.unwrap().unwrap();
    assert_eq!(f.native.calls(), 1);
    assert_eq!(f.store.claim.calls(), 1);
    assert_eq!(f.store.finish.calls(), 1);
}

#[tokio::test]
async fn real_source_defaults_refuse_without_legacy_fallback() {
    struct Unsupported;
    #[async_trait::async_trait]
    impl SubscriptionStore for Unsupported {
        async fn get(
            &self,
            _: &SubjectId,
        ) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
            panic!("legacy get")
        }
        async fn insert(&self, _: SubscriptionRecord) -> Result<bool, SubscriptionError> {
            panic!("legacy insert")
        }
        async fn replace(
            &self,
            _: &SubscriptionRecord,
            _: SubscriptionRecord,
        ) -> Result<bool, SubscriptionError> {
            panic!("legacy replace")
        }
    }
    #[async_trait::async_trait]
    impl NativeSubscriptionAuth for Unsupported {
        async fn start(&self) -> Result<NativeLogin, SubscriptionError> {
            panic!("legacy start")
        }
        async fn refresh(&self, _: &SecretValue) -> Result<SecretValue, SubscriptionError> {
            panic!("legacy refresh")
        }
    }
    let mut f = Fixture::new(3600);
    f.manager.native = Arc::new(Unsupported);
    f.tokens(false, None).await.unwrap().unwrap();
    assert!(matches!(
        f.tokens(true, None).await.unwrap_err(),
        AdmittedError::Unsupported(_)
    ));
    assert!(f.store.record().unwrap().cache.is_some());
    f.manager.store = Arc::new(Unsupported);
    assert!(matches!(
        f.tokens(false, None).await.unwrap_err(),
        AdmittedError::Unsupported(_)
    ));
}

#[tokio::test]
async fn missing_foreign_and_invalid_records_refuse_before_writes() {
    for case in 0..3 {
        let f = Fixture::new(1);
        if case == 0 {
            *f.store.record.lock().unwrap() = None;
        } else {
            f.edit(|record| {
                if case == 1 {
                    record.owner = SubjectId::new("foreign");
                } else {
                    record.cache = Some(SecretValue::new("{}".into()));
                }
            });
        }
        let expected = if case == 2 {
            SubscriptionError::InvalidAccount
        } else {
            SubscriptionError::SignInRequired
        };
        assert_eq!(f.tokens(false, None).await.unwrap().unwrap_err(), expected);
        assert_eq!(f.store.claim.calls(), 0);
        assert_eq!(f.native.calls(), 0);
    }
}

#[tokio::test]
async fn claim_and_completion_uncertainty_never_retries_or_cleans_up() {
    for claim in [true, false] {
        for committed in [true, false] {
            let f = Fixture::new(1);
            let boundary = if claim {
                &f.store.claim
            } else {
                &f.store.finish
            };
            boundary.config.lock().unwrap().fault =
                Some(Fault::Domain(SubscriptionError::Unavailable, committed));
            assert_eq!(
                f.tokens(false, None).await.unwrap().unwrap_err(),
                SubscriptionError::Unavailable
            );
            assert_eq!(f.store.claim.calls(), 1);
            assert_eq!(f.store.finish.calls(), usize::from(!claim));
            assert_eq!(f.native.calls(), usize::from(!claim));
            let record = f.store.record().unwrap();
            assert_eq!(
                record.revision,
                if claim {
                    1 + u64::from(committed)
                } else {
                    2 + u64::from(committed)
                }
            );
            assert_eq!(
                record.refresh_lease.is_some(),
                if claim { committed } else { !committed }
            );
        }
    }
}

#[tokio::test]
async fn source_write_failures_keep_their_category_and_stop_after_one_attempt() {
    for claim in [true, false] {
        let f = Fixture::new(1);
        let boundary = if claim {
            &f.store.claim
        } else {
            &f.store.finish
        };
        boundary.config.lock().unwrap().fault = Some(Fault::Source(
            AdmittedError::Unavailable("uncertain synthetic write"),
            true,
        ));
        assert!(matches!(
            f.tokens(false, None).await.unwrap_err(),
            AdmittedError::Unavailable(_)
        ));
        assert_eq!(f.store.claim.calls(), 1);
        assert_eq!(f.store.finish.calls(), usize::from(!claim));
        assert_eq!(f.native.calls(), usize::from(!claim));
    }
}

#[tokio::test]
async fn completion_conflicts_stop_at_the_existing_bound() {
    let f = Fixture::new(1);
    f.store.reject_finish.store(1, SeqCst);
    assert_eq!(
        f.tokens(false, None).await.unwrap().unwrap_err(),
        SubscriptionError::Changed
    );
    assert_eq!(f.store.finish.calls(), super::super::CAS_ATTEMPTS);
    assert_eq!(f.native.calls(), 1);
    assert!(f.store.record().unwrap().refresh_lease.is_some());
}

#[tokio::test]
async fn native_domain_errors_clear_only_sign_in_required_cache() {
    for error in [
        SubscriptionError::SignInRequired,
        SubscriptionError::Unavailable,
        SubscriptionError::InvalidAccount,
        SubscriptionError::Cancelled,
        SubscriptionError::Expired,
        SubscriptionError::Changed,
    ] {
        let f = Fixture::new(1);
        f.native.boundary.config.lock().unwrap().fault = Some(Fault::Domain(error, false));
        assert_eq!(f.tokens(false, None).await.unwrap().unwrap_err(), error);
        assert_eq!(f.store.finish.calls(), 1);
        let record = f.store.record().unwrap();
        assert!(record.refresh_lease.is_none());
        assert_eq!(
            record.cache.is_none(),
            error == SubscriptionError::SignInRequired
        );
    }
}

#[tokio::test]
async fn source_refusal_never_becomes_sign_in_required() {
    let f = Fixture::new(1);
    f.native.boundary.config.lock().unwrap().fault = Some(Fault::Source(
        AdmissionRefusal::AtCapacity {
            phase: "synthetic producer",
        }
        .into(),
        false,
    ));
    assert!(matches!(
        f.tokens(false, None).await.unwrap_err(),
        AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. })
    ));
    assert!(f.store.record().unwrap().cache.is_some());
    assert!(f.store.record().unwrap().refresh_lease.is_none());
}

#[tokio::test]
async fn changed_incarnation_holder_and_account_defeat_stale_completion() {
    for field in 0..3 {
        let f = Fixture::new(1);
        f.native.boundary.config.lock().unwrap().wait = true;
        let mut operation = f.operation(false, None).unwrap();
        assert!(futures_util::poll!(&mut operation).is_pending());
        let claim = f.store.record().unwrap();
        let lease = claim.refresh_lease.as_ref().unwrap();
        assert!(uuid::Uuid::parse_str(&lease.holder).is_ok());
        let remaining = lease.expires_at.duration_since(SystemTime::now()).unwrap();
        assert!(remaining > Duration::from_secs(28) && remaining <= Duration::from_secs(30));
        if field < 2 {
            f.edit(|r| {
                if field == 0 {
                    r.incarnation = "new-incarnation".into();
                } else {
                    r.refresh_lease.as_mut().unwrap().holder = "new-holder".into();
                }
            });
        } else {
            *f.native.renewed.lock().unwrap() = own_cache(&f.policy, "other", 3600);
        }
        f.native.boundary.release.add_permits(1);
        let result = operation.await.unwrap();
        assert_eq!(
            result.as_ref().unwrap_err(),
            &if field < 2 {
                SubscriptionError::Changed
            } else {
                SubscriptionError::InvalidAccount
            }
        );
        assert_eq!(f.store.finish.calls(), usize::from(field == 2));
    }
}

#[tokio::test]
async fn concurrent_forced_refresh_uses_the_other_holders_result() {
    let f = Fixture::new(3600);
    f.native.boundary.config.lock().unwrap().wait = true;
    let mut first = f.operation(true, None).unwrap();
    let mut second = f.operation(true, None).unwrap();
    assert!(futures_util::poll!(&mut first).is_pending());
    assert!(futures_util::poll!(&mut second).is_pending());
    assert_eq!(f.native.calls(), 1);
    f.native.boundary.release.add_permits(1);
    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert_eq!(
        first.as_ref().unwrap().access_token,
        second.as_ref().unwrap().access_token
    );
    assert_eq!(f.native.calls(), 1);
}
