use super::*;

struct FeatureValidator {
    enabled: AtomicBool,
    inner: Arc<Validator>,
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for FeatureValidator {
    fn check_feature(&self, _: BridgeFeature, _: &BridgePlatform) -> Result<(), BridgeError> {
        self.enabled
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or(BridgeError::Unavailable)
    }
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.inner.validate_connection(owner, config).await
    }
}
fn fixture() -> (Fixture, Arc<FeatureValidator>) {
    let mut fixture = Fixture::new();
    let validator = Arc::new(FeatureValidator {
        enabled: AtomicBool::new(true),
        inner: fixture.validator.clone(),
    });
    fixture.service = BridgeService::new(
        fixture.store.clone(),
        fixture.organizations.clone(),
        validator.clone(),
    );
    (fixture, validator)
}

#[tokio::test]
async fn disabled_management_recovers_exact_receipts_and_reads_but_stops_fresh_effects() {
    let (fixture, flags) = fixture();
    let original = fixture.create(owner()).await;
    flags.enabled.store(false, Ordering::SeqCst);
    let replay = fixture
        .change(
            0,
            "create-1",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        )
        .await
        .unwrap();
    assert_eq!(replay, original);
    assert!(
        fixture
            .service
            .detail(&context("human"), Some("fixture-auth"), &fixture.id)
            .await
            .unwrap()
            .can_manage
    );
    assert_eq!(
        fixture
            .change(1, "enable-off", BridgeChange::Enable)
            .await
            .unwrap_err(),
        BridgeError::Unavailable
    );
    let mut changed = config();
    changed.display_name = "new name".into();
    assert_eq!(
        fixture
            .change(
                1,
                "update-off",
                BridgeChange::Update {
                    config: Box::new(changed),
                    previous_work: None
                }
            )
            .await
            .unwrap_err(),
        BridgeError::Unavailable
    );
    let retained = fixture.store.bridge(&fixture.id).await.unwrap().unwrap();
    assert_eq!(retained.revision, 1);
    assert_eq!(
        retained.current().unwrap().config.display_name,
        config().display_name
    );
}

#[tokio::test]
async fn disabled_management_allows_emergency_reduction_but_not_restoration() {
    let (fixture, flags) = fixture();
    fixture.create(owner()).await;
    flags.enabled.store(false, Ordering::SeqCst);
    let controls = BridgeEmergencyControls {
        ingress_stopped: true,
        execution_stopped: true,
        outbound_stopped: true,
    };
    let stopped = fixture
        .change(1, "stop-off", BridgeChange::Emergency { controls })
        .await
        .unwrap();
    assert_eq!(stopped.revision, 2);
    assert_eq!(
        fixture
            .change(
                2,
                "restore-off",
                BridgeChange::Emergency {
                    controls: BridgeEmergencyControls::default()
                }
            )
            .await
            .unwrap_err(),
        BridgeError::Unavailable
    );
    assert_eq!(
        fixture
            .store
            .bridge(&fixture.id)
            .await
            .unwrap()
            .unwrap()
            .emergency,
        controls
    );
    flags.enabled.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .change(
                2,
                "restore-off",
                BridgeChange::Emergency {
                    controls: BridgeEmergencyControls::default()
                }
            )
            .await
            .unwrap()
            .revision,
        3
    );
}

#[tokio::test]
async fn a_disabled_flag_never_substitutes_for_current_manager_authority() {
    let (fixture, flags) = fixture();
    fixture.create(organization()).await;
    flags.enabled.store(false, Ordering::SeqCst);
    *fixture.organizations.role.lock().unwrap() = Some(OrganizationRole::Member);
    assert_eq!(
        fixture
            .change(
                0,
                "create-1",
                BridgeChange::Create {
                    owner: organization(),
                    config: Box::new(config())
                }
            )
            .await
            .unwrap_err(),
        BridgeError::Forbidden
    );
}
