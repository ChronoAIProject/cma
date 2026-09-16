use super::*;

struct AdmissionGate {
    store: Arc<MongoBridgeStore>,
    observed: tokio::sync::Notify,
    proceed: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl BridgeStore for AdmissionGate {
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
        let pin = write.custody_pin().expect("enable retains custody proof");
        assert_eq!(pin.revision(), 1);
        assert_eq!(pin.key_state_version(), 3);
        assert_eq!(pin.reference(), &config().connector.custody);
        self.observed.notify_one();
        self.proceed.notified().await;
        self.store.apply(write).await
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn enable_rechecks_selected_custody_after_proof_before_atomic_write() {
    let (client, database, store) = fixture().await;
    let svc = service(store.clone());
    let id = BridgeService::creation_id(&caller(), "create").unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    admit_custody(&svc, &store, &id, 1).await;
    let original = store.bridge(&id).await.unwrap().unwrap();
    let gate = Arc::new(AdmissionGate {
        store: store.clone(),
        observed: tokio::sync::Notify::new(),
        proceed: tokio::sync::Notify::new(),
    });
    let gated = BridgeService::new(gate.clone(), Arc::new(Authority), Arc::new(Authority));
    let enable_id = id.clone();
    let enable = tokio::spawn(async move {
        gated
            .mutate(
                &caller(),
                None,
                change(&enable_id, 1, "enable", BridgeChange::Enable),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), gate.observed.notified())
        .await
        .unwrap();
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    store
        .credentials
        .revoke(&management, &config().connector.custody, 1, &"a".repeat(64))
        .await
        .unwrap();
    gate.proceed.notify_one();
    assert_eq!(
        enable.await.unwrap(),
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::CustodyUnavailable,
        ))
    );
    assert_eq!(store.bridge(&id).await.unwrap().unwrap(), original);
    assert_eq!(store.receipts.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&database).drop().await.unwrap();
}
