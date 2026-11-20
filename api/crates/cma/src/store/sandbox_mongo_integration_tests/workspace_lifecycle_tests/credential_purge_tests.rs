use super::*;
use crate::sandbox::{CredentialIssuance, IssuanceIdentity, IssuanceState};

mod fixture;
mod matrix_tests;
mod transaction_tests;
use fixture::Fixture;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unknown_issuance_retains_parent_at_transactional_purge() {
    for legacy in [false, true] {
        let f = Fixture::new("purge_unknown").await;
        let mut intent = f.intent(IssuanceState::Unknown);
        if legacy {
            intent.identity = IssuanceIdentity::LegacyUnobserved;
        }
        f.seed_intent(&intent).await;
        let before = f.snapshot().await;
        let result = f.purge().await;
        let after = f.snapshot().await;
        let due = f.store.due_sandbox_work(NOW, 10).await.unwrap();
        f.cleanup().await;
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert_eq!(before, after, "purge refusal must preserve every record");
        assert_eq!(due, vec![f.parent.id]);
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn settled_accepted_issuance_and_never_issued_parent_can_purge() {
    for accepted in [false, true] {
        let f = Fixture::new("purge_settled").await;
        if accepted {
            f.seed_intent(&f.intent(IssuanceState::Accepted)).await;
        }
        f.purge().await.unwrap();
        assert!(f.store.get_sandbox(&f.parent.id).await.unwrap().is_none());
        assert_eq!(
            f.rows("workspaces").count_documents(doc! {}).await.unwrap(),
            0
        );
        assert_eq!(
            f.rows("sandbox_credential_issuance")
                .count_documents(doc! {})
                .await
                .unwrap(),
            u64::from(accepted),
            "settled issuance history is retained"
        );
        f.cleanup().await;
    }
}
