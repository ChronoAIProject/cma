use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use crate::profiles::{
    AgentProfileRef, InMemoryProfileStore, SandboxProfileRef, WorkspaceProfileRef,
};
use crate::{
    rfc3339_at, Authorization, Caller, SandboxProfileId, ServiceHandoffId, ServiceId, SubjectId,
    WorkspaceProfileId,
};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

#[derive(Default)]
struct BrokerState {
    create_calls: usize,
    state_calls: usize,
    observed: Option<BrokerHandoffState>,
    descriptions: Vec<ServiceConnectionMetadata>,
    description_calls: Vec<(SubjectId, Vec<ServiceId>)>,
}

#[derive(Default)]
struct StubBroker {
    state: Mutex<BrokerState>,
}

impl StubBroker {
    fn calls(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.create_calls, state.state_calls)
    }

    fn observe(&self, observed: BrokerHandoffState) {
        self.state.lock().unwrap().observed = Some(observed);
    }
}

#[async_trait]
impl ServiceConnectionBroker for StubBroker {
    async fn describe_services(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        let mut state = self.state.lock().unwrap();
        state
            .description_calls
            .push((owner.clone(), services.to_vec()));
        Ok(state.descriptions.clone())
    }

    async fn requirements(
        &self,
        _owner: &SubjectId,
        _services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        Ok(Vec::new())
    }

    async fn create_handoff(
        &self,
        _owner: &SubjectId,
        _service: &ServiceId,
        intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        self.state.lock().unwrap().create_calls += 1;
        Ok(BrokerHandoff {
            external_id: format!("external-{intent_id}"),
            hosted_url: format!("https://identity.example/connect/{intent_id}"),
            expires_at: rfc3339_at(SystemTime::now() + Duration::from_secs(900)),
        })
    }

    async fn handoff_state(
        &self,
        _owner: &SubjectId,
        _external_id: &str,
        _service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        let mut state = self.state.lock().unwrap();
        state.state_calls += 1;
        Ok(state.observed.unwrap_or(BrokerHandoffState::Pending))
    }
}

struct Fixture {
    store: Arc<InMemoryLaunchStore>,
    broker: Arc<StubBroker>,
    service: Arc<ServiceConnectionService>,
    caller: Caller,
    launch: Launch,
}

fn content() -> TriggerContent {
    TriggerContent {
        presentation: Presentation {
            title: "Connect and launch".into(),
            description: String::new(),
            button_label: ButtonLabel::RunWithCma,
            theme: Theme::Auto,
            signup_hint: None,
        },
        target: TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_one".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp_one".into()), 1),
            agent_profile: AgentProfileRef::new(crate::AgentProfileId("agp_one".into()), 1),
            inline: None,
        },
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::AlwaysNew,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::AlwaysNew,
        },
        first_message: String::new(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: false,
        },
    }
}

async fn fixture() -> Fixture {
    let profiles: Arc<dyn TriggerProfileResolver> = Arc::new(InMemoryProfileStore::default());
    let definitions = Arc::new(InMemoryTriggerStore::new(profiles));
    let publisher = SubjectId::new("publisher");
    let mut trigger = Trigger::new(
        TriggerId("trg_connections".into()),
        publisher.clone(),
        content(),
        NOW.into(),
    )
    .unwrap();
    let revision = trigger.publish(NOW.into()).unwrap();
    let public = PublicTrigger {
        trigger_id: trigger.id.clone(),
        public_id: trigger.public_id.clone().unwrap(),
        owner: publisher,
        availability: TriggerAvailability::Published,
        revision,
    };
    definitions.seed_trigger(trigger).unwrap();
    let store = Arc::new(InMemoryLaunchStore::new(definitions));
    let owner = SubjectId::new("visitor");
    let requirements = RequirementsSnapshot {
        services: vec![ServiceRequirement {
            service: ServiceId::new("github"),
            state: ServiceRequirementState::Missing,
        }],
        checked_at: NOW.into(),
    };
    let launch = Launch::new(LaunchSeed {
        trigger_id: public.trigger_id.clone(),
        revision_digest: public.revision.digest.clone(),
        public_id: public.public_id.clone(),
        owner: owner.clone(),
        authority: LaunchAuthorityClass::Visitor,
        placement_id: None,
        referrer_origin: None,
        review: ReviewRequirements::build(&requirements, None, false, false).unwrap(),
        requirements,
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: false,
        command_key_hash: "a".repeat(64),
        command_fingerprint: "b".repeat(64),
        created_at: NOW.into(),
    })
    .unwrap();
    let launch = store
        .admit_launch(LaunchAdmission {
            launch,
            trigger: public,
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap()
        .launch()
        .clone();
    let broker = Arc::new(StubBroker::default());
    let service = ServiceConnectionService::new(store.clone(), broker.clone());
    Fixture {
        store,
        broker,
        service,
        caller: Caller::new(owner, Authorization::none()),
        launch,
    }
}

fn request(fixture: &Fixture, service: &str, intent: &str) -> CreateServiceConnection {
    CreateServiceConnection {
        service_id: ServiceId::new(service),
        launch_id: fixture.launch.id.clone(),
        intent_id: intent.into(),
    }
}

#[tokio::test]
async fn creation_replays_before_the_broker_and_validates_the_launch_requirement() {
    let fixture = fixture().await;
    let intended = request(&fixture, "github", "intent-one");
    let created = fixture
        .service
        .create(&fixture.caller, intended.clone())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .create(&fixture.caller, intended)
            .await
            .unwrap(),
        created
    );
    assert_eq!(fixture.broker.calls(), (1, 0));
    assert_eq!(
        fixture
            .service
            .create(&fixture.caller, request(&fixture, "gitlab", "intent-one"))
            .await,
        Err(ServiceConnectionServiceError::IntentReused)
    );
    assert_eq!(
        fixture
            .service
            .create(&fixture.caller, request(&fixture, "gitlab", "intent-two"))
            .await,
        Err(ServiceConnectionServiceError::InvalidRequest(
            "service_id is not an unmet Launch requirement".into()
        ))
    );
    assert_eq!(fixture.broker.calls(), (1, 0));

    let other = Caller::new(SubjectId::new("other"), Authorization::none());
    assert_eq!(
        fixture
            .service
            .create(&other, request(&fixture, "github", "other-intent"))
            .await,
        Err(ServiceConnectionServiceError::NotFound)
    );
}

#[tokio::test]
async fn polling_persists_one_terminal_provider_observation_and_hides_ownership() {
    let fixture = fixture().await;
    let created = fixture
        .service
        .create(
            &fixture.caller,
            request(&fixture, "github", "satisfied-intent"),
        )
        .await
        .unwrap();
    fixture.broker.observe(BrokerHandoffState::Satisfied);
    let satisfied = fixture
        .service
        .read_and_refresh(&fixture.caller, &created.id)
        .await
        .unwrap();
    assert_eq!(satisfied.state, BrokerHandoffState::Satisfied);
    assert_eq!(satisfied.revision, 2);
    assert_eq!(
        fixture
            .service
            .read_and_refresh(&fixture.caller, &created.id)
            .await
            .unwrap(),
        satisfied
    );
    assert_eq!(fixture.broker.calls(), (1, 1));

    let other = Caller::new(SubjectId::new("other"), Authorization::none());
    assert_eq!(
        fixture.service.read_and_refresh(&other, &created.id).await,
        Err(ServiceConnectionServiceError::NotFound)
    );
    assert_eq!(
        fixture
            .service
            .read_and_refresh(&fixture.caller, &ServiceHandoffId("hnd_missing".into()))
            .await,
        Err(ServiceConnectionServiceError::NotFound)
    );
}

#[tokio::test]
async fn locally_expired_handoffs_do_not_call_the_provider() {
    let fixture = fixture().await;
    let handoff = ServiceConnectionHandoff {
        id: ServiceHandoffId("hnd_expired".into()),
        launch_id: fixture.launch.id.clone(),
        owner: fixture.caller.subject.clone(),
        service_id: ServiceId::new("github"),
        intent_id: "expired-intent".into(),
        hosted_url: "https://identity.example/connect/expired".into(),
        expires_at: "2026-01-01T00:15:00Z".into(),
        state: BrokerHandoffState::Pending,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        external_id: "external-expired".into(),
        revision: 1,
    };
    fixture.store.create_handoff(handoff.clone()).await.unwrap();
    let refreshed = fixture
        .service
        .read_and_refresh(&fixture.caller, &handoff.id)
        .await
        .unwrap();
    assert_eq!(refreshed.state, BrokerHandoffState::Expired);
    assert_eq!(fixture.broker.calls(), (0, 0));
}

fn metadata() -> ServiceConnectionMetadata {
    ServiceConnectionMetadata {
        service: ServiceId::new("github"),
        display_name: Some("GitHub".into()),
        connection_mode: ServiceConnectionMode::Hosted,
        setup_url: None,
    }
}

#[tokio::test]
async fn metadata_reads_are_owner_scoped_and_never_change_launch_history() {
    let fixture = fixture().await;
    fixture.broker.state.lock().unwrap().descriptions = vec![metadata()];
    let foreign = Caller::new(SubjectId::new("other"), Authorization::none());
    for (caller, id) in [
        (&foreign, fixture.launch.id.clone()),
        (&fixture.caller, crate::LaunchId("lch_absent".into())),
    ] {
        assert_eq!(
            fixture.service.describe_launch_services(caller, &id).await,
            Err(ServiceConnectionServiceError::NotFound)
        );
    }
    assert!(fixture
        .broker
        .state
        .lock()
        .unwrap()
        .description_calls
        .is_empty());
    assert_eq!(
        fixture
            .service
            .describe_launch_services(&fixture.caller, &fixture.launch.id)
            .await
            .unwrap(),
        vec![metadata()]
    );
    assert_eq!(
        fixture.broker.state.lock().unwrap().description_calls,
        vec![(
            fixture.caller.subject.clone(),
            vec![ServiceId::new("github")]
        )]
    );
    assert_eq!(
        fixture
            .store
            .launch(&fixture.launch.id, &fixture.caller.subject)
            .await
            .unwrap()
            .unwrap(),
        fixture.launch
    );
}

#[tokio::test]
async fn metadata_rejects_foreign_duplicate_and_unsafe_broker_rows() {
    let fixture = fixture().await;
    let mut foreign = metadata();
    foreign.service = ServiceId::new("other");
    let mut unsafe_url = metadata();
    unsafe_url.connection_mode = ServiceConnectionMode::Manual;
    unsafe_url.setup_url = Some("https://user:credential@nyx.example/services".into());
    let mut unexpected_url = metadata();
    unexpected_url.setup_url = Some("https://nyx.example/services".into());
    let mut long_name = metadata();
    long_name.display_name = Some("x".repeat(257));
    let mut control_name = metadata();
    control_name.display_name = Some("GitHub\nprivate".into());
    for rows in [
        vec![foreign],
        vec![metadata(), metadata()],
        vec![unsafe_url],
        vec![unexpected_url],
        vec![long_name],
        vec![control_name],
    ] {
        fixture.broker.state.lock().unwrap().descriptions = rows;
        assert!(matches!(
            fixture
                .service
                .describe_launch_services(&fixture.caller, &fixture.launch.id)
                .await,
            Err(ServiceConnectionServiceError::Corrupt(_))
        ));
    }
    fixture.broker.state.lock().unwrap().descriptions.clear();
    assert!(fixture
        .service
        .describe_launch_services(&fixture.caller, &fixture.launch.id)
        .await
        .unwrap()
        .is_empty());
}
