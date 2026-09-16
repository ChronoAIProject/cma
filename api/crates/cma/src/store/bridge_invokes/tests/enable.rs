use super::*;
use layer2_core::bridge::setup::*;
mod setup;

struct Gate {
    store: Arc<MongoBridgeStore>,
    observed: tokio::sync::Notify,
    proceed: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl BridgeStore for Gate {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError> {
        self.store.bridge(id).await
    }
    async fn receipt(&self, id: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError> {
        self.store.receipt(id).await
    }
    async fn list(
        &self,
        owner: &ResourceOwner,
        issuer: &str,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError> {
        self.store.list(owner, issuer, after, limit).await
    }
    async fn apply(&self, write: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError> {
        assert!(write.enable_grant().is_some());
        self.observed.notify_one();
        self.proceed.notified().await;
        self.store.apply(write).await
    }
}
fn mutation(f: &Fixture, key: &str) -> BridgeMutation {
    BridgeMutation {
        id: f.bridge.id.clone(),
        expected_revision: f.bridge.revision,
        idempotency_key: key.into(),
        change: BridgeChange::Enable,
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn disabled_issue_enable_consumes_exact_grant_and_old_receipt_never_restores_revoked_permission(
) {
    let mut f = Fixture::new().await;
    let setup = setup::service(&f);
    let command = ControlBridge {
        expected_revision: f.bridge.revision,
        command: BridgeControlCommand::Enable,
    };
    let missing = BridgeError::ConnectionRequired(BridgeSetupBlock::InvokeConsentRequired);
    assert!(
        setup
            .validate(&caller(), None, &f.bridge.id, f.bridge.revision)
            .await
            .unwrap()
            .valid
    );
    assert_eq!(
        setup
            .control(
                &caller(),
                None,
                f.bridge.id.clone(),
                "setup-no-consent".into(),
                command.clone()
            )
            .await,
        Err(missing.clone())
    );
    assert_eq!(
        f.definitions
            .mutate(&caller(), None, mutation(&f, "no-proof"))
            .await,
        Err(missing.clone())
    );
    let management = f
        .definitions
        .management_lease(&caller(), None, &f.bridge.id, f.bridge.revision)
        .await
        .unwrap();
    assert!(matches!(f.service.prepare_enable(&management).await,Err(error) if error==missing));
    let grant = f
        .service
        .issue(&caller(), None, f.issue("consent"))
        .await
        .unwrap()
        .result;
    // A stored grant does not turn generic mutation into an execution proof.
    assert_eq!(
        f.definitions
            .mutate(&caller(), None, mutation(&f, "still-no-proof"))
            .await,
        Err(missing)
    );
    let original = mutation(&f, "enable");
    let receipt = setup
        .control(
            &caller(),
            None,
            f.bridge.id.clone(),
            "enable".into(),
            command.clone(),
        )
        .await
        .unwrap();
    f.bridge = f.bridges.bridge(&f.bridge.id).await.unwrap().unwrap();
    assert_eq!(f.bridge.state, BridgeState::Enabled);
    assert_eq!(
        f.store.grant(&grant.id).await.unwrap().unwrap().view(),
        grant
    );
    f.service
        .revoke(&caller(), None, f.revoke(&grant, "revoke"))
        .await
        .unwrap();
    f.definitions
        .mutate(
            &caller(),
            None,
            BridgeMutation {
                id: f.bridge.id.clone(),
                expected_revision: f.bridge.revision,
                idempotency_key: "disable".into(),
                change: BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.definitions
            .mutate(&caller(), None, original)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        setup
            .control(
                &caller(),
                None,
                f.bridge.id.clone(),
                "enable".into(),
                command
            )
            .await
            .unwrap(),
        receipt
    );
    assert!(matches!(
        f.bridges.bridge(&f.bridge.id).await.unwrap().unwrap().state,
        BridgeState::Disabled { .. }
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn enable_fences_revocation_configuration_and_recreated_target_after_provider_proof_before_commit(
) {
    for change in 0..3 {
        let f = Fixture::new().await;
        let grant = f
            .service
            .issue(&caller(), None, f.issue("consent"))
            .await
            .unwrap()
            .result;
        let management = f
            .definitions
            .management_lease(&caller(), None, &f.bridge.id, f.bridge.revision)
            .await
            .unwrap();
        let proof = f.service.prepare_enable(&management).await.unwrap();
        let gate = Arc::new(Gate {
            store: f.bridges.clone(),
            observed: tokio::sync::Notify::new(),
            proceed: tokio::sync::Notify::new(),
        });
        let service = BridgeService::new(gate.clone(), f.authority.clone(), f.authority.clone());
        let request = mutation(&f, "enable");
        let enable = tokio::spawn(async move {
            service
                .mutate_with_enable_proof(&caller(), None, request, proof)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), gate.observed.notified())
            .await
            .unwrap();
        match change {
            0 => {
                f.service
                    .revoke(&caller(), None, f.revoke(&grant, "revoke"))
                    .await
                    .unwrap();
            }
            1 => {
                let mut config = f.bridge.current().unwrap().config.clone();
                config.display_name = "Changed after proof".into();
                f.definitions
                    .mutate(
                        &caller(),
                        None,
                        BridgeMutation {
                            id: f.bridge.id.clone(),
                            expected_revision: f.bridge.revision,
                            idempotency_key: "refresh".into(),
                            change: BridgeChange::Update {
                                config: Box::new(config),
                                previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                            },
                        },
                    )
                    .await
                    .unwrap();
            }
            _ => {
                f.resources.agents.update_one(doc!{"_id":f.target.agent_id.as_str()},doc!{"$set":{"creation_fingerprint":"f".repeat(64),"record.creation_fingerprint":"f".repeat(64)}}).await.unwrap();
            }
        }
        gate.proceed.notify_one();
        let expected = match change {
            0 => BridgeError::ConnectionRequired(BridgeSetupBlock::InvokeConsentRequired),
            1 => BridgeError::Conflict,
            _ => BridgeError::ConnectionRequired(BridgeSetupBlock::TargetMismatch),
        };
        assert_eq!(enable.await.unwrap(), Err(expected));
        assert!(matches!(
            f.bridges.bridge(&f.bridge.id).await.unwrap().unwrap().state,
            BridgeState::Disabled { .. }
        ));
        assert_eq!(
            f.client
                .database(&f.database)
                .collection::<Document>("bridge_definition_receipts")
                .count_documents(doc! {"record.receipt.command":"enable"})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            f.resources.agents.count_documents(doc! {}).await.unwrap(),
            1
        );
        f.close().await;
    }
}
