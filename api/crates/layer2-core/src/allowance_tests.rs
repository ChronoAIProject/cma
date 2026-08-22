//! The advisory live-session allowance remains scoped to its caller.

use std::num::NonZeroU32;
use std::sync::Arc;

use crate::authz::{Authorization, Caller, Capability};
use crate::ids::SubjectId;
use crate::test_support::{
    no_environments, service_with_environments_and_settings, spec, user, FakeAdapter, FakeStore,
};
use crate::PlatformSettings;

#[tokio::test]
async fn allowance_reports_the_setting_subject_count_and_explicit_exemption() {
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        no_environments(),
        PlatformSettings {
            live_sessions: NonZeroU32::new(3),
            ..PlatformSettings::default()
        },
    );
    let owner = user("owner");
    for _ in 0..2 {
        service
            .create_session(&owner, spec())
            .await
            .expect("two of three slots are available");
    }

    let ordinary = service.session_allowance(&owner).await.unwrap();
    assert_eq!(ordinary.limit, Some(3));
    assert_eq!(ordinary.live, 2);
    assert!(!ordinary.exempt);

    let exempt = Caller::new(
        SubjectId("owner".into()),
        Authorization::from_capabilities([Capability::ExceedSessionLimit]),
    );
    let exempt = service.session_allowance(&exempt).await.unwrap();
    assert_eq!(exempt.limit, Some(3));
    assert_eq!(exempt.live, 2);
    assert!(exempt.exempt);
}

#[tokio::test]
async fn an_unconfigured_deployment_reports_no_limit() {
    let service = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        no_environments(),
        PlatformSettings::default(),
    );

    let allowance = service.session_allowance(&user("owner")).await.unwrap();
    assert_eq!(allowance.limit, None);
    assert_eq!(allowance.live, 0);
    assert!(!allowance.exempt);
}
