use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountedProfiles {
    inner: Arc<InMemoryProfileStore>,
    calls: AtomicUsize,
}

#[async_trait]
impl TriggerProfileReader for CountedProfiles {
    async fn read_trigger_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.read_trigger_profiles(target).await
    }
}

fn observed_service(fixture: &Fixture, limit: u32) -> (Arc<LaunchService>, Arc<CountedProfiles>) {
    let profiles = Arc::new(CountedProfiles {
        inner: fixture.profiles.clone(),
        calls: AtomicUsize::new(0),
    });
    let mut service = fixture.service_with_policy(LaunchPolicy {
        enabled: true,
        orchestration_paused: false,
        visitor_launches_per_day: limit,
    });
    Arc::get_mut(&mut service).unwrap().profile_reader = profiles.clone();
    (service, profiles)
}

async fn exhausted_fixture() -> (Fixture, Launch) {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let mut original = None;
    for index in 0..5 {
        let created = fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request(format!("allowance-{index}")),
            )
            .await
            .unwrap();
        original.get_or_insert(created.launch);
    }
    fixture.broker.state.lock().unwrap().calls = 0;
    (fixture, original.unwrap())
}

#[tokio::test]
async fn exhausted_visitor_is_refused_before_profiles_or_connections() {
    let (fixture, original) = exhausted_fixture().await;
    fixture.broker.set_unavailable(true);
    let (service, profiles) = observed_service(&fixture, 5);
    let (_, reset_at) = super::super::visitor_allowance::visitor_launch_day(
        crate::seconds_from_rfc3339(&original.created_at).unwrap(),
    );
    assert_eq!(
        service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("over-allowance")
            )
            .await,
        Err(LaunchServiceError::VisitorQuota { limit: 5, reset_at })
    );
    assert_eq!(profiles.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.broker.calls(), 0);
    assert_eq!(fixture.stats().await.launches_total, 5);
}

#[tokio::test]
async fn exhausted_visitor_exact_replay_returns_original_launch() {
    let (fixture, original) = exhausted_fixture().await;
    fixture.broker.set_unavailable(true);
    let (service, profiles) = observed_service(&fixture, 0);
    let replay = service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("allowance-0"),
        )
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, original);
    assert_eq!(profiles.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.broker.calls(), 0);
    assert_eq!(fixture.stats().await.launches_total, 5);
}

#[tokio::test]
async fn sandbox_creator_is_never_refused_by_visitor_allowance() {
    let (fixture, _) = exhausted_fixture().await;
    let caller = Caller::new(
        fixture.caller.subject.clone(),
        Authorization::new([Capability::LaunchTriggers, Capability::CreateSandbox]),
    );
    let (service, profiles) = observed_service(&fixture, 0);
    let created = service
        .create_or_resume(
            &caller,
            &fixture.public.public_id,
            fixture.request("creator-after-allowance"),
        )
        .await
        .unwrap();
    assert!(created.created);
    assert_eq!(
        created.launch.authority,
        LaunchAuthorityClass::SandboxCreator
    );
    assert_eq!(profiles.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.broker.calls(), 1);
}

#[tokio::test]
async fn raised_allowance_accepts_new_commands_without_changing_existing_launches() {
    let (fixture, original) = exhausted_fixture().await;
    let (lowered, _) = observed_service(&fixture, 4);
    assert!(matches!(
        lowered
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("after-settings-change")
            )
            .await,
        Err(LaunchServiceError::VisitorQuota { limit: 4, .. })
    ));
    let (raised, _) = observed_service(&fixture, 6);
    assert!(
        raised
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("after-settings-change")
            )
            .await
            .unwrap()
            .created
    );
    assert_eq!(
        fixture
            .launches
            .launch(&original.id, &original.owner)
            .await
            .unwrap(),
        Some(original)
    );
    assert_eq!(fixture.stats().await.launches_total, 6);
}

#[tokio::test]
async fn unknown_trigger_is_not_hidden_by_exhausted_allowance() {
    let (fixture, _) = exhausted_fixture().await;
    let (service, profiles) = observed_service(&fixture, 5);
    assert_eq!(
        service
            .create_or_resume(
                &fixture.caller,
                &PublicTriggerId("pub_unknown".into()),
                fixture.request("unknown-after-allowance")
            )
            .await,
        Err(LaunchServiceError::TriggerNotFound)
    );
    assert_eq!(profiles.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.broker.calls(), 0);
}

#[tokio::test]
async fn memory_allowance_counts_only_the_visitor_owner_and_day() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let original = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("count-visitor"),
        )
        .await
        .unwrap()
        .launch;
    let creator = Caller::new(
        fixture.caller.subject.clone(),
        Authorization::new([Capability::LaunchTriggers, Capability::CreateSandbox]),
    );
    for (caller, key) in [
        (creator, "count-creator"),
        (launcher("another-visitor"), "count-other"),
    ] {
        fixture
            .service
            .create_or_resume(&caller, &fixture.public.public_id, fixture.request(key))
            .await
            .unwrap();
    }
    let (day, _) = super::super::visitor_allowance::visitor_launch_day(
        crate::seconds_from_rfc3339(&original.created_at).unwrap(),
    );
    assert_eq!(
        fixture
            .launches
            .visitor_launches_admitted(&fixture.caller.subject, day)
            .await,
        Ok(1)
    );
    assert_eq!(
        fixture
            .launches
            .visitor_launches_admitted(&SubjectId::new("another-visitor"), day)
            .await,
        Ok(1)
    );
    assert_eq!(
        fixture
            .launches
            .visitor_launches_admitted(&fixture.caller.subject, day + 1)
            .await,
        Ok(0)
    );
}
