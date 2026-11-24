use super::*;
use layer2_core::{OperationClaim, OperationReservation, OperationStore, StoreError};

/// Cancel at the post-admission HTTP ledger boundary; production persistence is
/// still delegated to the real Mongo operation store without changing its result.
pub(super) struct CancelAfterAdmission {
    pub inner: Arc<MongoOperationStore>,
    pub authority: Arc<Authority>,
    pub cancel: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl OperationStore for CancelAfterAdmission {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        self.inner.reserve(claim).await
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        if self.cancel.load(Ordering::SeqCst) {
            let response: Value = serde_json::from_slice(&outcome).unwrap();
            assert_eq!(response["status"], 202);
            assert!(layer2_core::agent_ownership::execution_authority().is_none());
            TransportAuthority::revoke(self.authority.as_ref());
        }
        self.inner.complete(claim, outcome).await
    }
}

impl Fixture {
    pub(super) async fn control_receipt(&self) -> Option<mongodb::bson::Document> {
        let row = self
            .admin
            .database(&self.name)
            .collection::<mongodb::bson::Document>("agents")
            .find_one(doc! {"_id":self.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        row.get_document(if self.deleting {
            "control_delete_receipt"
        } else {
            "control_stop_receipt"
        })
        .ok()
        .cloned()
    }

    pub(super) async fn completed_response(&self, key: &str, status: u16) {
        let operation = format!(
            "agent:{}:{}",
            if self.deleting { "delete" } else { "stop" },
            self.agent.id
        );
        let row = self
            .admin
            .database(&self.name)
            .collection::<mongodb::bson::Document>("operation_claims")
            .find_one(doc! {"_id.subject":"owner", "_id.operation":operation, "_id.key":key})
            .await
            .unwrap()
            .expect("HTTP idempotency claim persisted");
        let outcome = row
            .get_binary_generic("outcome")
            .expect("canonical HTTP response completed");
        let response: Value = serde_json::from_slice(outcome).unwrap();
        assert_eq!(response["status"], status);
        assert_eq!(response["body"], json!([]));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn delete_post_admission_cancellation_completes_idempotency_and_replays_on_other_ingress() {
    for peer in [false, true] {
        let f = Fixture::new(true).await;
        let ingress = if peer { 1 - f.owner } else { f.owner };
        let key = uuid::Uuid::new_v4().to_string();
        f.cancel_after_admission.store(true, Ordering::SeqCst);
        f.request_key(
            "delete_committed_then_receiver_cancelled",
            ingress,
            202,
            None,
            &key,
        )
        .await;
        assert!(f.authority.receiver_cancelled());
        assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision + 1);
        f.assert_pending_while_cancelled("delete_committed_then_receiver_cancelled")
            .await;
        f.request_key(
            "delete_same_key_other_ingress",
            1 - ingress,
            202,
            None,
            &key,
        )
        .await;
        f.cancel_after_admission.store(false, Ordering::SeqCst);
        f.replicas[f.owner]
            .coordinator
            .handoff(&f.authority, f.authority.revoke_transports())
            .await
            .unwrap();
        f.complete().await;
        f.close().await;
    }
}
