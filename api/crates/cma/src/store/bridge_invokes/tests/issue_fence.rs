use super::*;
use crate::bridge::custody::BridgeCredentialStore;

struct Gate {
    store: Arc<MongoBridgeInvokeStore>,
    observed: tokio::sync::Notify,
    proceed: tokio::sync::Notify,
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn proof_clones_and_delayed_issuance_preserve_the_original_nonrenewing_deadline() {
    let f = Fixture::new().await;
    let grant = f
        .service
        .issue(&caller(), None, f.issue("consent"))
        .await
        .unwrap()
        .result;
    let proof = f
        .service
        .authorize(f.request(
            &grant,
            BridgeInvokeEffect::AdmitMessage {
                inbox_id: "inbox-expiry".into(),
            },
        ))
        .await
        .unwrap();
    let clone = proof.clone();
    assert_eq!(proof.deadline(), clone.deadline());
    let gate = Arc::new(Gate {
        store: f.store.clone(),
        observed: tokio::sync::Notify::new(),
        proceed: tokio::sync::Notify::new(),
    });
    let service = BridgeInvokeService::new(
        f.definitions.clone(),
        Arc::new(ResourceAccessService::new(
            f.resources.clone(),
            f.authority.clone(),
        )),
        f.resources.clone(),
        gate.clone(),
    );
    let request = f.issue("delayed");
    let issue = tokio::spawn(async move { service.issue(&caller(), None, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.observed.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(16)).await;
    assert_eq!(proof.require_current(), Err(BridgeInvokeError::Unavailable));
    assert_eq!(clone.require_current(), Err(BridgeInvokeError::Unavailable));
    tokio::time::resume();
    gate.proceed.notify_one();
    assert!(issue.await.unwrap().is_err());
    assert_eq!(f.store.grants.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(f.store.receipts.count_documents(doc! {}).await.unwrap(), 1);
    f.close().await;
}
#[async_trait::async_trait]
impl BridgeInvokeStore for Gate {
    async fn grant(
        &self,
        id: &BridgeInvokeGrantId,
    ) -> Result<Option<BridgeInvokeGrant>, BridgeInvokeError> {
        self.store.grant(id).await
    }
    async fn receipt(&self, id: &str) -> Result<Option<BridgeInvokeReceipt>, BridgeInvokeError> {
        self.store.receipt(id).await
    }
    async fn list(
        &self,
        id: &BridgeId,
        after: Option<&BridgeInvokeGrantId>,
        limit: u32,
    ) -> Result<Vec<BridgeInvokeGrant>, BridgeInvokeError> {
        self.store.list(id, after, limit).await
    }
    async fn apply(
        &self,
        write: BridgeInvokeWrite,
    ) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
        self.observed.notify_one();
        self.proceed.notified().await;
        self.store.apply(write).await
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn issue_rechecks_custody_and_creation_identity_after_authorization_before_atomic_write() {
    for custody in [false, true] {
        let f = Fixture::new().await;
        let gate = Arc::new(Gate {
            store: f.store.clone(),
            observed: tokio::sync::Notify::new(),
            proceed: tokio::sync::Notify::new(),
        });
        let service = BridgeInvokeService::new(
            f.definitions.clone(),
            Arc::new(ResourceAccessService::new(
                f.resources.clone(),
                f.authority.clone(),
            )),
            f.resources.clone(),
            gate.clone(),
        );
        let request = f.issue("gated");
        let issue = tokio::spawn(async move { service.issue(&caller(), None, request).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), gate.observed.notified())
            .await
            .unwrap();
        if custody {
            let management = f
                .definitions
                .management_lease(&caller(), None, &f.bridge.id, f.bridge.revision)
                .await
                .unwrap();
            f.bridges
                .credentials
                .revoke(
                    &management,
                    &f.bridge.current().unwrap().config.connector.custody,
                    1,
                    &"a".repeat(64),
                )
                .await
                .unwrap();
        } else {
            f.resources
                .agents
                .update_one(
                    doc! {"_id":f.target.agent_id.as_str()},
                    doc! {"$set":{"record.creation_fingerprint":"e".repeat(64),"creation_fingerprint":"e".repeat(64)}},
                )
                .await
                .unwrap();
        }
        gate.proceed.notify_one();
        assert!(issue.await.unwrap().is_err());
        assert_eq!(f.store.grants.count_documents(doc! {}).await.unwrap(), 0);
        assert_eq!(f.store.receipts.count_documents(doc! {}).await.unwrap(), 0);
        assert_eq!(f.store.audits.count_documents(doc! {}).await.unwrap(), 0);
        f.close().await;
    }
}
