use super::*;

#[tokio::test]
async fn create_uses_origin_specific_capabilities_and_limits() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_origin",
            "owner",
            "origin",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;

    let manual_store = Arc::new(InMemorySandboxStore::default());
    let manual_service = SandboxService::new(
        manual_store,
        profiles.clone(),
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let manual = caller(
        "owner",
        [Capability::CreateSandbox, Capability::UseOwnSandboxes],
    );
    let mut rejected_trigger = request(reference.clone());
    rejected_trigger.origin = SandboxOrigin::Trigger {
        launch_id: "launch-rejected-public".into(),
    };
    assert_eq!(
        manual_service
            .create(&manual, rejected_trigger.clone())
            .await
            .unwrap_err(),
        SandboxServiceError::InvalidOrigin
    );
    assert_eq!(
        manual_service
            .create_for_launch(
                &manual,
                Capability::CreateSandbox,
                request(reference.clone()),
            )
            .await
            .unwrap_err(),
        SandboxServiceError::InvalidOrigin
    );
    assert_eq!(
        manual_service
            .create_for_launch(
                &manual,
                Capability::LaunchTriggers,
                rejected_trigger.clone(),
            )
            .await
            .unwrap_err(),
        SandboxServiceError::Forbidden
    );
    assert_eq!(
        manual_service
            .create_for_launch(
                &manual,
                Capability::UseOwnSandboxes,
                rejected_trigger.clone(),
            )
            .await
            .unwrap_err(),
        SandboxServiceError::Forbidden
    );
    rejected_trigger.origin = SandboxOrigin::Trigger {
        launch_id: "launch\ninvalid".into(),
    };
    assert_eq!(
        manual_service
            .create_for_launch(&manual, Capability::CreateSandbox, rejected_trigger.clone(),)
            .await
            .unwrap_err(),
        SandboxServiceError::InvalidOrigin
    );
    assert_eq!(
        manual_service
            .create_for_launch(
                &caller("owner", []),
                Capability::LaunchTriggers,
                rejected_trigger,
            )
            .await
            .unwrap_err(),
        SandboxServiceError::Forbidden
    );
    for _ in 0..2 {
        manual_service
            .create(&manual, request(reference.clone()))
            .await
            .unwrap();
    }
    let mut trigger_request = request(reference.clone());
    trigger_request.origin = SandboxOrigin::Trigger {
        launch_id: "launch-owner".into(),
    };
    let owner_launch = manual_service
        .create_for_launch(&manual, Capability::CreateSandbox, trigger_request.clone())
        .await
        .unwrap();
    assert_eq!(
        manual_service
            .create_for_launch(&manual, Capability::CreateSandbox, trigger_request)
            .await
            .unwrap(),
        owner_launch
    );
    assert_eq!(
        manual_service
            .create(&manual, request(reference.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxLimitReached {
            limit: 3,
            live: 3,
            usage: Default::default()
        }
    );
    let visitor_store = Arc::new(InMemorySandboxStore::default());
    let visitor_service = SandboxService::new(
        visitor_store,
        profiles.clone(),
        policy(3, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let launcher = caller("owner", [Capability::LaunchTriggers]);
    let mut trigger_request = request(reference.clone());
    trigger_request.origin = SandboxOrigin::Trigger {
        launch_id: "launch-1".into(),
    };
    let visitor_launch = visitor_service
        .create_for_launch(
            &launcher,
            Capability::LaunchTriggers,
            trigger_request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        visitor_service
            .create_for_launch(
                &launcher,
                Capability::LaunchTriggers,
                trigger_request.clone(),
            )
            .await
            .unwrap(),
        visitor_launch
    );
    trigger_request.origin = SandboxOrigin::Trigger {
        launch_id: "launch-2".into(),
    };
    assert_eq!(
        visitor_service
            .create_for_launch(&launcher, Capability::LaunchTriggers, trigger_request)
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxLimitReached {
            limit: 1,
            live: 1,
            usage: Default::default()
        }
    );
    assert_eq!(
        visitor_service
            .create(&launcher, request(reference.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::Forbidden
    );

    let dual_store = Arc::new(RacingStore::default());
    let dual_service = SandboxService::new(
        dual_store.clone(),
        profiles,
        policy(2, 1),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let dual = caller(
        "owner",
        [Capability::CreateSandbox, Capability::LaunchTriggers],
    );
    let launch = |launch_id: &str| {
        let mut create = request(reference.clone());
        create.origin = SandboxOrigin::Trigger {
            launch_id: launch_id.into(),
        };
        create
    };
    dual_service
        .create_for_launch(&dual, Capability::LaunchTriggers, launch("launch-dual-1"))
        .await
        .unwrap();
    assert_eq!(
        dual_service
            .create_for_launch(&dual, Capability::LaunchTriggers, launch("launch-dual-2"))
            .await
            .unwrap_err(),
        SandboxServiceError::SandboxLimitReached {
            limit: 1,
            live: 1,
            usage: Default::default()
        }
    );
    dual_service
        .create_for_launch(&dual, Capability::CreateSandbox, launch("launch-dual-2"))
        .await
        .unwrap();
    use crate::sandbox::store::SandboxLimitClass::{Owner, Visitor};
    assert_eq!(
        *dual_store.admitted_classes.lock().unwrap(),
        vec![Visitor, Visitor, Owner]
    );
}
