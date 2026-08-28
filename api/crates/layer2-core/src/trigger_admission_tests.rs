//! Server-owned Trigger pricing, quota, and runtime admission boundaries.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use layer3_adapter::{KernelAdapter, KernelCapabilities};
use sha2::{Digest, Sha256};

use crate::service::{Deployment, ScreenFactory, SessionPorts, SessionService};
use crate::store::{PlatformSettingsStore, StoreError, StoredSettings};
use crate::{
    AgentConfiguration, Baseline, BillingMode, CoreError, DeploymentCapacity,
    EnvironmentProfileService, KernelId, Launch, LaunchReviewAdmission, LaunchState,
    PlatformSettings, PlatformSettingsService, Rate, RequiredCapability, SessionMode, SubjectId,
    TriggerAdmissionError, TriggerLaunchAdmission, CMA_TRIGGER_CONTRIBUTOR_POLICY_V1,
    CMA_TRIGGER_ZERO_COST_POLICY_V1,
};

const REVIEWED_AT: &str = "2026-08-26T12:00:00Z";
const STARTED_AT: &str = "2026-08-26T12:01:00Z";
const RUNTIME_V1: &str = "cma-trigger-test-runtime@sha256:admitted";
const RUNTIME_V2: &str = "cma-trigger-test-runtime@sha256:replacement";
const TEST_KERNEL: &str = "trigger-test-kernel";

struct MutableSettings(StdMutex<StoredSettings>);

impl MutableSettings {
    fn new(settings: PlatformSettings) -> Self {
        Self(StdMutex::new(StoredSettings {
            settings,
            revision: 1,
            previous: None,
        }))
    }

    fn current(&self) -> PlatformSettings {
        self.0.lock().expect("test settings lock").settings.clone()
    }

    fn replace(&self, settings: PlatformSettings) {
        let mut stored = self.0.lock().expect("test settings lock");
        let previous = stored.settings.clone();
        stored.settings = settings;
        stored.revision += 1;
        stored.previous = Some(previous);
    }
}

#[async_trait]
impl PlatformSettingsStore for MutableSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Ok(Some(self.0.lock().expect("test settings lock").clone()))
    }

    async fn save_if_unchanged(
        &self,
        _expected: u64,
        _settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable(
            "this Trigger fixture changes settings directly".into(),
        ))
    }
}

struct WorkspaceBytes(Vec<u8>);

#[async_trait]
impl crate::SessionWorkspaceSource for WorkspaceBytes {
    async fn load_workspace(
        &self,
        _owner: &SubjectId,
        _origin: &crate::TriggerSessionOrigin,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(Some(self.0.clone()))
    }
}

struct MissingExactServiceCredential;

#[async_trait]
impl crate::CredentialSource for MissingExactServiceCredential {
    async fn exact_services_available(
        &self,
        _subject: &SubjectId,
        _services: &[crate::ServiceId],
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn credential_for(
        &self,
        _session: &crate::SessionId,
        _subject: &SubjectId,
        _services: &[crate::ServiceId],
        _caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        panic!("credential mint must not run after exact-service admission fails")
    }

    async fn release(&self, _session: &crate::SessionId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct UnverifiableExactServiceCredential;

#[async_trait]
impl crate::CredentialSource for UnverifiableExactServiceCredential {
    async fn credential_for(
        &self,
        _session: &crate::SessionId,
        _subject: &SubjectId,
        _services: &[crate::ServiceId],
        _caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        panic!("credential mint must not run when exact-service state is unavailable")
    }

    async fn release(&self, _session: &crate::SessionId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct AdmittedButUnavailableCredential;

#[async_trait]
impl crate::CredentialSource for AdmittedButUnavailableCredential {
    async fn exact_services_available(
        &self,
        _subject: &SubjectId,
        _services: &[crate::ServiceId],
    ) -> Result<bool, StoreError> {
        Ok(true)
    }

    async fn credential_for(
        &self,
        _session: &crate::SessionId,
        _subject: &SubjectId,
        _services: &[crate::ServiceId],
        _caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        Ok(None)
    }

    async fn release(&self, _session: &crate::SessionId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct AdmissionFixture {
    service: Arc<SessionService>,
    store: Arc<crate::test_support::FakeStore>,
    settings: Arc<MutableSettings>,
    adapter: Arc<crate::test_support::FakeAdapter>,
    archive_sha256: String,
}

fn capabilities() -> Vec<RequiredCapability> {
    vec![
        RequiredCapability::Conversation,
        RequiredCapability::DurableConversation,
        RequiredCapability::WorkspaceCheckpoint,
        RequiredCapability::RestrictedServiceGateway,
        RequiredCapability::CredentialRefresh,
        RequiredCapability::CredentialRevocation,
    ]
}

fn priced_settings() -> PlatformSettings {
    let mut settings = PlatformSettings::default();
    settings.pricing = settings.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        "2026-08-25T12:00:00Z",
    );
    settings
}

fn fixture(settings: PlatformSettings) -> AdmissionFixture {
    fixture_with_credentials(
        settings,
        Arc::new(crate::test_support::FixedCredential(
            "session-scoped-test-key".into(),
        )),
    )
}

fn fixture_with_credentials(
    settings: PlatformSettings,
    credentials: Arc<dyn crate::CredentialSource>,
) -> AdmissionFixture {
    let store = Arc::new(crate::test_support::FakeStore::default());
    let settings = Arc::new(MutableSettings::new(settings));
    let adapter = Arc::new(crate::test_support::FakeAdapter {
        capabilities: KernelCapabilities {
            conversation: true,
            snapshot: true,
            ..crate::test_support::FakeAdapter::default().capabilities
        },
        ..crate::test_support::FakeAdapter::default()
    });
    let archive = b"server-owned Trigger admission workspace".to_vec();
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    let service = build_service(
        Arc::clone(&store),
        Arc::clone(&settings),
        Arc::clone(&adapter),
        Arc::new(WorkspaceBytes(archive)),
        RUNTIME_V1,
        capabilities(),
        credentials,
    );
    AdmissionFixture {
        service,
        store,
        settings,
        adapter,
        archive_sha256,
    }
}

fn build_service(
    store: Arc<crate::test_support::FakeStore>,
    settings: Arc<MutableSettings>,
    adapter: Arc<crate::test_support::FakeAdapter>,
    workspaces: Arc<dyn crate::SessionWorkspaceSource>,
    runtime_version: &str,
    trigger_runtime_capabilities: Vec<RequiredCapability>,
    credentials: Arc<dyn crate::CredentialSource>,
) -> Arc<SessionService> {
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    let handovers = Arc::clone(&adapter) as Arc<dyn crate::store::HandoverStore>;
    adapters.insert(KernelId::new(TEST_KERNEL), adapter);
    let profiles = Arc::new(crate::test_support::FakeProfileStore::default());
    let environments = EnvironmentProfileService::new(
        profiles,
        Baseline::default(),
        None,
        None,
        AgentConfiguration {
            kernel: KernelId::new(TEST_KERNEL),
            mode: SessionMode::Conversation,
            ..AgentConfiguration::default()
        },
        crate::test_support::unconfigured_settings(),
    );
    let factory: ScreenFactory =
        Arc::new(|geometry| Box::new(crate::test_support::NullScreen(geometry)));
    let service = SessionService::new(
        adapters,
        factory,
        store,
        SessionPorts {
            handovers: Some(handovers),
            credentials: Some(credentials),
            workspaces: Some(workspaces),
            settings: Some(Arc::new(PlatformSettingsService::new(settings))),
            ..SessionPorts::default()
        },
        environments,
        Deployment {
            runtime_version: Some(runtime_version.into()),
            trigger_runtime_capabilities,
            trigger_kernel: Some(KernelId::new(TEST_KERNEL)),
            ..crate::test_support::owned()
        },
    );
    service.retry_backoff_ms.store(1, Ordering::Relaxed);
    service
}

fn launch(billing_mode: BillingMode) -> Launch {
    let mut revision =
        crate::trigger::tests::revision_fixture(SubjectId("trigger-publisher".into()));
    revision.content.commercial_policy.billing_mode = billing_mode;
    revision.content.commercial_policy.policy_reference = match billing_mode {
        BillingMode::ZeroCost => CMA_TRIGGER_ZERO_COST_POLICY_V1,
        BillingMode::Contributor => CMA_TRIGGER_CONTRIBUTOR_POLICY_V1,
        BillingMode::Sponsored => "disabled-test-sponsorship",
    }
    .into();
    revision.digest = revision.content.digest().expect("test revision is valid");
    Launch::new(
        SubjectId("trigger-contributor".into()),
        revision,
        REVIEWED_AT,
        "create-admission-test",
    )
    .expect("test Launch is valid")
}

async fn review(
    service: &SessionService,
    launch: &mut Launch,
) -> Result<LaunchReviewAdmission, TriggerAdmissionError> {
    let admission =
        TriggerLaunchAdmission::review(service, &launch.owner, launch, REVIEWED_AT).await?;
    launch
        .apply_review_admission(admission.clone())
        .expect("the production admission snapshot validates");
    Ok(admission)
}

async fn ready_for_provisioning(fixture: &AdmissionFixture, billing_mode: BillingMode) -> Launch {
    let mut launch = launch(billing_mode);
    review(fixture.service.as_ref(), &mut launch)
        .await
        .expect("review admission succeeds");
    let start =
        TriggerLaunchAdmission::start(fixture.service.as_ref(), &launch.owner, &launch, STARTED_AT)
            .await
            .expect("Start admission succeeds");
    launch
        .record_start_admission(start)
        .expect("Start evidence belongs to this review");

    let template = crate::trigger::tests::session_launch(&fixture.archive_sha256);
    launch.connection = template.connection;
    launch.authority = template.authority;
    launch.source_sha = template.source_sha;
    launch.workspace = template.workspace;
    launch.started_at = Some(STARTED_AT.into());
    launch.state = LaunchState::SandboxPending;
    launch
}

#[tokio::test]
async fn zero_cost_is_pinned_even_when_the_deployment_has_a_price() {
    let fixture = fixture(priced_settings());
    let mut launch = launch(BillingMode::ZeroCost);
    let admission = review(fixture.service.as_ref(), &mut launch)
        .await
        .expect("a public zero-cost Launch is admitted");

    assert_eq!(admission.pricing.billing_mode, BillingMode::ZeroCost);
    assert_eq!(
        admission.pricing.policy_reference,
        CMA_TRIGGER_ZERO_COST_POLICY_V1
    );
    assert_eq!(admission.pricing.rate_card, None);
    assert_eq!(admission.pricing.rate, None);
    assert_eq!(admission.pricing.micros_per_hour, 0);
    assert_eq!(admission.pricing.estimated_micros, 0);
    assert_eq!(admission.runtime.kernel, KernelId::new(TEST_KERNEL));

    let launch = ready_for_provisioning(&fixture, BillingMode::ZeroCost).await;
    let session = fixture
        .service
        .ensure_trigger_session(&launch)
        .await
        .expect("the zero-cost Trigger session is accepted");
    assert_eq!(session.pricing, None);
}

#[tokio::test]
async fn public_contributor_estimate_uses_the_review_rate_and_trigger_defaults() {
    let fixture = fixture(priced_settings());
    let launch = launch(BillingMode::Contributor);

    let cost = TriggerLaunchAdmission::preview_cost(
        fixture.service.as_ref(),
        &launch.revision,
        REVIEWED_AT,
    )
    .await;

    assert_eq!(
        cost,
        crate::TriggerPreviewCost::ContributorEstimate {
            currency: "USD".into(),
            // 500m CPU at 40,000 + 1 GiB at 5,000, for the one-hour default.
            amount_micros: 25_000,
            display: "$0.025".into(),
            quoted_at: REVIEWED_AT.into(),
        }
    );

    let mut reviewed = launch;
    let admission = review(fixture.service.as_ref(), &mut reviewed)
        .await
        .expect("authenticated review uses the same deployment pricing");
    assert_eq!(admission.pricing.estimated_micros, 25_000);
}

#[tokio::test]
async fn public_contributor_estimate_is_unavailable_without_a_rate_card() {
    let fixture = fixture(PlatformSettings::default());
    let launch = launch(BillingMode::Contributor);

    let cost = TriggerLaunchAdmission::preview_cost(
        fixture.service.as_ref(),
        &launch.revision,
        REVIEWED_AT,
    )
    .await;

    assert_eq!(cost, crate::TriggerPreviewCost::QuoteUnavailable);
}

#[tokio::test]
async fn missing_configured_trigger_kernel_fails_review_without_inference() {
    let mut fixture = fixture(PlatformSettings::default());
    Arc::get_mut(&mut fixture.service)
        .expect("the fixture retains the sole service handle")
        .trigger_kernel = None;
    let launch = launch(BillingMode::ZeroCost);

    let error = TriggerLaunchAdmission::review(
        fixture.service.as_ref(),
        &launch.owner,
        &launch,
        REVIEWED_AT,
    )
    .await
    .expect_err("a missing configured kernel must not be inferred");
    assert_eq!(
        error,
        TriggerAdmissionError::CapabilityUnsupported {
            capability: "trigger_kernel".into(),
        }
    );
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unknown_configured_trigger_kernel_fails_review_without_inference() {
    let mut fixture = fixture(PlatformSettings::default());
    Arc::get_mut(&mut fixture.service)
        .expect("the fixture retains the sole service handle")
        .trigger_kernel = Some(KernelId::new("future-kernel"));
    let launch = launch(BillingMode::ZeroCost);

    let error = TriggerLaunchAdmission::review(
        fixture.service.as_ref(),
        &launch.owner,
        &launch,
        REVIEWED_AT,
    )
    .await
    .expect_err("an unknown configured kernel must not be inferred from runtime metadata");
    assert_eq!(
        error,
        TriggerAdmissionError::CapabilityUnsupported {
            capability: "trigger_kernel".into(),
        }
    );
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_retired_rate_card_requires_a_fresh_review_before_start() {
    let fixture = fixture(priced_settings());
    let mut launch = launch(BillingMode::Contributor);
    let reviewed = review(fixture.service.as_ref(), &mut launch)
        .await
        .expect("the current contributor rate is admitted");
    let reviewed_card = reviewed.pricing.rate_card.expect("a priced card is pinned");

    let mut changed = fixture.settings.current();
    changed.pricing = changed.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 60_000,
            memory_micros_per_gib_hour: 8_000,
        },
        STARTED_AT,
    );
    fixture.settings.replace(changed);

    let error =
        TriggerLaunchAdmission::start(fixture.service.as_ref(), &launch.owner, &launch, STARTED_AT)
            .await
            .expect_err("Start cannot accept a superseded review price");
    assert_eq!(error, TriggerAdmissionError::PriceChanged);
    assert_eq!(launch.pricing.unwrap().rate_card, Some(reviewed_card));
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn retirement_after_start_keeps_the_exact_pinned_price() {
    let fixture = fixture(priced_settings());
    let launch = ready_for_provisioning(&fixture, BillingMode::Contributor).await;
    let pinned = launch
        .pricing
        .as_ref()
        .and_then(|pricing| pricing.rate_card.clone())
        .expect("the accepted contributor card is pinned");

    let mut changed = fixture.settings.current();
    changed.pricing = changed.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 60_000,
            memory_micros_per_gib_hour: 8_000,
        },
        "2026-08-26T12:02:00Z",
    );
    let replacement = changed
        .pricing
        .in_force()
        .expect("the replacement card is in force")
        .id
        .clone();
    fixture.settings.replace(changed);

    let session = fixture
        .service
        .ensure_trigger_session(&launch)
        .await
        .expect("post-Start retirement cannot withdraw an accepted price");
    let session_price = session.pricing.expect("contributor pricing is durable");
    assert_eq!(session_price.rate_card, pinned);
    assert_ne!(session_price.rate_card, replacement);
}

#[tokio::test]
async fn start_rechecks_live_session_and_deployment_capacity_quota() {
    let live_fixture = fixture(PlatformSettings::default());
    let mut live_limited = launch(BillingMode::ZeroCost);
    review(live_fixture.service.as_ref(), &mut live_limited)
        .await
        .expect("the initially empty deployment admits review");

    live_fixture
        .service
        .create_session(
            &crate::test_support::user(&live_limited.owner.0),
            crate::test_support::spec(),
        )
        .await
        .expect("one ordinary live session occupies the owner quota");
    let mut limited = live_fixture.settings.current();
    limited.live_sessions = NonZeroU32::new(1);
    live_fixture.settings.replace(limited);
    let error = TriggerLaunchAdmission::start(
        live_fixture.service.as_ref(),
        &live_limited.owner,
        &live_limited,
        STARTED_AT,
    )
    .await
    .expect_err("Start rechecks the owner's current live-session count");
    assert!(matches!(error, TriggerAdmissionError::QuotaExceeded(_)));

    let capacity_fixture = fixture(PlatformSettings::default());
    let mut capacity_limited = launch(BillingMode::ZeroCost);
    review(capacity_fixture.service.as_ref(), &mut capacity_limited)
        .await
        .expect("review succeeds before deployment capacity changes");
    let mut settings = capacity_fixture.settings.current();
    settings.deployment_capacity = DeploymentCapacity {
        cpu_millis: Some(0),
        memory_mib: Some(0),
        assumed_session_size: None,
    };
    capacity_fixture.settings.replace(settings);
    let error = TriggerLaunchAdmission::start(
        capacity_fixture.service.as_ref(),
        &capacity_limited.owner,
        &capacity_limited,
        STARTED_AT,
    )
    .await
    .expect_err("Start rechecks current deployment headroom");
    assert!(matches!(error, TriggerAdmissionError::QuotaExceeded(_)));
}

#[tokio::test]
async fn quote_expiry_and_runtime_drift_are_refused_before_start() {
    let fixture = fixture(priced_settings());
    let mut expired = launch(BillingMode::Contributor);
    review(fixture.service.as_ref(), &mut expired)
        .await
        .expect("the contributor quote starts current");
    expired
        .pricing
        .as_mut()
        .expect("the review has pricing")
        .expires_at = STARTED_AT.into();
    let error = TriggerLaunchAdmission::start(
        fixture.service.as_ref(),
        &expired.owner,
        &expired,
        STARTED_AT,
    )
    .await
    .expect_err("the exact expiry instant is no longer startable");
    assert_eq!(error, TriggerAdmissionError::QuoteExpired);

    let mut changed_runtime = launch(BillingMode::ZeroCost);
    review(fixture.service.as_ref(), &mut changed_runtime)
        .await
        .expect("runtime v1 admits the review");
    let replacement_adapter = Arc::new(crate::test_support::FakeAdapter {
        capabilities: KernelCapabilities {
            conversation: true,
            snapshot: true,
            ..crate::test_support::FakeAdapter::default().capabilities
        },
        ..crate::test_support::FakeAdapter::default()
    });
    let replacement = build_service(
        Arc::clone(&fixture.store),
        Arc::clone(&fixture.settings),
        replacement_adapter,
        Arc::new(WorkspaceBytes(Vec::new())),
        RUNTIME_V2,
        capabilities(),
        Arc::new(crate::test_support::FixedCredential(
            "session-scoped-test-key".into(),
        )),
    );
    let error = TriggerLaunchAdmission::start(
        replacement.as_ref(),
        &changed_runtime.owner,
        &changed_runtime,
        STARTED_AT,
    )
    .await
    .expect_err("a different runtime image requires a fresh review");
    assert_eq!(error, TriggerAdmissionError::RuntimeChanged);
}

#[tokio::test]
async fn unsupported_capabilities_fail_review_and_provisioning_before_provider_calls() {
    let fixture = fixture(PlatformSettings::default());
    let missing = RequiredCapability::CredentialRevocation;
    let reduced: Vec<_> = capabilities()
        .into_iter()
        .filter(|capability| *capability != missing)
        .collect();
    let reduced_adapter = Arc::new(crate::test_support::FakeAdapter {
        capabilities: KernelCapabilities {
            conversation: true,
            snapshot: true,
            ..crate::test_support::FakeAdapter::default().capabilities
        },
        ..crate::test_support::FakeAdapter::default()
    });
    let reduced_service = build_service(
        Arc::clone(&fixture.store),
        Arc::clone(&fixture.settings),
        Arc::clone(&reduced_adapter),
        Arc::new(WorkspaceBytes(Vec::new())),
        RUNTIME_V1,
        reduced,
        Arc::new(crate::test_support::FixedCredential(
            "session-scoped-test-key".into(),
        )),
    );
    let unsupported = launch(BillingMode::ZeroCost);
    let error = TriggerLaunchAdmission::review(
        reduced_service.as_ref(),
        &unsupported.owner,
        &unsupported,
        REVIEWED_AT,
    )
    .await
    .expect_err("review fails closed on a missing published capability");
    assert_eq!(
        error,
        TriggerAdmissionError::CapabilityUnsupported {
            capability: "credential_revocation".into(),
        }
    );

    let ready = ready_for_provisioning(&fixture, BillingMode::ZeroCost).await;
    let error = reduced_service
        .ensure_trigger_session(&ready)
        .await
        .expect_err("capabilities are checked again immediately before provisioning");
    assert!(
        matches!(error, CoreError::InvalidRequest(ref detail) if detail.contains("credential_revocation")),
        "got {error:?}"
    );
    assert_eq!(reduced_adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_identity_is_rechecked_immediately_before_provisioning() {
    let fixture = fixture(PlatformSettings::default());
    let ready = ready_for_provisioning(&fixture, BillingMode::ZeroCost).await;
    let replacement_adapter = Arc::new(crate::test_support::FakeAdapter {
        capabilities: KernelCapabilities {
            conversation: true,
            snapshot: true,
            ..crate::test_support::FakeAdapter::default().capabilities
        },
        ..crate::test_support::FakeAdapter::default()
    });
    let replacement = build_service(
        Arc::clone(&fixture.store),
        Arc::clone(&fixture.settings),
        Arc::clone(&replacement_adapter),
        Arc::new(WorkspaceBytes(Vec::new())),
        RUNTIME_V2,
        capabilities(),
        Arc::new(crate::test_support::FixedCredential(
            "session-scoped-test-key".into(),
        )),
    );

    let error = replacement
        .ensure_trigger_session(&ready)
        .await
        .expect_err("runtime identity drift is fenced before provisioning");
    assert!(
        matches!(error, CoreError::InvalidRequest(ref detail) if detail.contains("runtime changed")),
        "got {error:?}"
    );
    assert_eq!(
        replacement_adapter.provision_calls.load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn missing_exact_github_service_fails_before_session_or_runtime_creation() {
    let fixture = fixture_with_credentials(
        PlatformSettings::default(),
        Arc::new(MissingExactServiceCredential),
    );
    let ready = ready_for_provisioning(&fixture, BillingMode::ZeroCost).await;
    let session_id = crate::trigger_session_id(&ready.id).expect("the Launch identity is valid");

    let error = fixture
        .service
        .ensure_trigger_session(&ready)
        .await
        .expect_err("a Trigger session requires the exact frozen GitHub service");

    assert!(
        matches!(error, CoreError::ServiceNotGranted(ref service) if service == "service-1"),
        "got {error:?}"
    );
    assert!(
        crate::store::SessionStore::get(fixture.store.as_ref(), &session_id)
            .await
            .expect("the fake session store is readable")
            .is_none(),
        "admission failure must not leave the deterministic session record behind"
    );
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unavailable_exact_service_catalogue_fails_before_session_or_runtime_creation() {
    let fixture = fixture_with_credentials(
        PlatformSettings::default(),
        Arc::new(UnverifiableExactServiceCredential),
    );
    let ready = ready_for_provisioning(&fixture, BillingMode::ZeroCost).await;
    let session_id = crate::trigger_session_id(&ready.id).expect("the Launch identity is valid");

    let error = fixture
        .service
        .ensure_trigger_session(&ready)
        .await
        .expect_err("a catalogue outage cannot become positive grant evidence");

    assert!(
        matches!(error, CoreError::ServiceCatalogUnavailable(_)),
        "got {error:?}"
    );
    assert!(
        crate::store::SessionStore::get(fixture.store.as_ref(), &session_id)
            .await
            .expect("the fake session store is readable")
            .is_none(),
        "catalogue failure must not leave the deterministic session record behind"
    );
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn credential_loss_after_admission_refuses_provider_runtime_creation() {
    let fixture = fixture_with_credentials(
        PlatformSettings::default(),
        Arc::new(AdmittedButUnavailableCredential),
    );
    let ready = ready_for_provisioning(&fixture, BillingMode::ZeroCost).await;

    let created = fixture
        .service
        .ensure_trigger_session(&ready)
        .await
        .expect("the exact service was available at the admission boundary");
    for _ in 0..400 {
        if !fixture.service.starting.read().await.contains(&created.id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    assert!(
        !fixture.service.starting.read().await.contains(&created.id),
        "the failed start must settle for deterministic reconciliation"
    );
    assert_eq!(fixture.adapter.provision_calls.load(Ordering::SeqCst), 0);
    let durable = crate::store::SessionStore::get(fixture.store.as_ref(), &created.id)
        .await
        .expect("the deterministic session is readable")
        .expect("the admitted session record remains reconcilable");
    assert_eq!(durable.runtime_id, None);
    assert_eq!(durable.initial_prompt, None);
}
