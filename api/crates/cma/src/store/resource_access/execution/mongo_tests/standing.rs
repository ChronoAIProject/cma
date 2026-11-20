use super::*;
use crate::auth::account_standing::ErasingSnapshot;
use crate::store::account_deletions::MongoAccountDeletionStore;
use fixture::{context, Fixture};
use layer2_core::account_deletion::AccountDeletionStore;
use layer2_core::resource_access::ResourceCapability;
use layer2_core::SubjectId;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn account_deletion_revokes_minted_execution_authority_and_new_admission() {
    for deleting in ["owner", "recipient", "unrelated"] {
        let fixture = Fixture::new().await;
        let db = fixture.client.database(&fixture.name);
        let deletions = Arc::new(MongoAccountDeletionStore::open(&db).await.unwrap());
        let standing = Arc::new(ErasingSnapshot::new(deletions.clone()));
        standing.refresh().await.unwrap();
        let policy = ResourceExecutionPolicy::enforce(fixture.store.clone(), true)
            .await
            .unwrap();
        let coordinator = ResourceExecutionCoordinator::new(
            fixture.access.clone().with_account_standing(standing),
            policy,
        );
        let authority = coordinator
            .authorize_execution(
                &context("recipient"),
                Some("synthetic"),
                &fixture.target,
                ResourceCapability::Interact,
                None,
            )
            .await
            .unwrap();
        authority.validate_live().await.unwrap();
        coordinator
            .register_execution(&authority, &fixture.execution("before-deletion"))
            .await
            .unwrap();
        deletions
            .request(
                &SubjectId::new(deleting),
                "accepted",
                &layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();

        let retained = authority.validate_live().await;
        let next = coordinator
            .register_execution(&authority, &fixture.execution("after-deletion"))
            .await;
        let fresh = coordinator
            .authorize_execution(
                &context("recipient"),
                Some("synthetic"),
                &fixture.target,
                ResourceCapability::Interact,
                None,
            )
            .await;
        if deleting == "unrelated" {
            retained.unwrap();
            next.unwrap();
            fresh.unwrap();
        } else {
            assert_eq!(retained, Err(ExecutionError::AuthorityChanged));
            assert!(matches!(next, Err(ExecutionError::AuthorityChanged)));
            assert!(matches!(fresh, Err(ExecutionError::AuthorityChanged)));
        }
        db.drop().await.unwrap();
    }
}
