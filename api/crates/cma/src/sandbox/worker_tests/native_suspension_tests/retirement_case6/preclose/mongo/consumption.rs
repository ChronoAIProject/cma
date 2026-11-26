use super::*;
use crate::store::native_retirement_action::{
    capture::RetirementCaptureConsumer, RetirementAction, RetirementRecords,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_consumption_requires_current_parent_and_participants() {
    let f = Fixture::new().await;
    let offer = f
        .store
        .offer_retirement_capture(&f.parent, &f.child.id)
        .await
        .unwrap();
    let bound = f
        .authority
        .run(
            f.store
                .bind_retirement_capture(&f.child, offer, Uuid::new_v4()),
        )
        .await
        .unwrap()
        .unwrap();
    let children = [f.child.clone()];
    assert!(f
        .store
        .observed(&f.parent, &children)
        .await
        .unwrap()
        .is_none());
    let capture = receive(&f, &bound, f.authority.clone(), "404 Not Found").await;
    f.store
        .complete_retirement_capture(&bound, capture.sealed().unwrap())
        .await
        .unwrap();

    let mut unclaimed = f.parent.clone();
    unclaimed.work.lease = None;
    let mut stale = f.parent.clone();
    stale.revision -= 1;
    let mut replaced = f.parent.clone();
    replaced.runtime.as_mut().unwrap().boot_id = Some("00000000000000000000000000000002".into());
    let mut replaced_image = f.parent.clone();
    replaced_image.runtime.as_mut().unwrap().runtime_version = "replacement-image".into();
    let mut replaced_home = f.parent.clone();
    replaced_home
        .lifecycle
        .home
        .as_mut()
        .unwrap()
        .receipt
        .as_mut()
        .unwrap()
        .storage_generation = "replacement-home-generation".into();
    for candidate in [
        &unclaimed,
        &stale,
        &replaced,
        &replaced_image,
        &replaced_home,
    ] {
        assert!(matches!(
            f.store.observed(candidate, &children).await,
            Err(SandboxStoreError::Conflict)
        ));
    }
    assert!(f.store.observed(&f.parent, &[]).await.unwrap().is_none());
    let captured = f
        .store
        .observed(&f.parent, &children)
        .await
        .unwrap()
        .unwrap();
    let (observation, fences) = captured.observation();
    let action = RetirementAction::new(&f.parent, observation, &children, fences).unwrap();
    assert_eq!(
        RetirementCaptureConsumer::admit(&f.store, &unclaimed, &action, &captured).await,
        Err(SandboxStoreError::Conflict)
    );

    let sandboxes = f.collection("sandboxes");
    let parent_filter = doc! {"_id": f.parent.id.as_str()};
    let parent_row = f.parent_row().await;
    let expired = "2026-01-01T00:00:00Z";
    sandboxes
        .update_one(
            parent_filter.clone(),
            doc! {"$set": {
                "work_lease_expires_at": expired, "record.work.lease.expires_at": expired,
            }},
        )
        .await
        .unwrap();
    assert_eq!(
        RetirementCaptureConsumer::admit(&f.store, &f.parent, &action, &captured).await,
        Err(SandboxStoreError::Conflict)
    );
    sandboxes
        .replace_one(parent_filter, parent_row)
        .await
        .unwrap();

    let agents = f.collection("agents");
    let filter = doc! {"_id":f.child.id.as_str()};
    let revision = i64::try_from(f.child.revision).unwrap();
    let original = agents.find_one(filter.clone()).await.unwrap().unwrap();
    agents
        .update_one(
            filter.clone(),
            doc! {"$set":{"revision":revision+1,"record.revision":revision+1}},
        )
        .await
        .unwrap();
    assert_eq!(
        RetirementCaptureConsumer::admit(&f.store, &f.parent, &action, &captured).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(f.store.action(&f.parent.id).await.unwrap(), None);
    agents.replace_one(filter, original).await.unwrap();

    let owners = f.collection("cma_agent_manager_owners");
    let filter = doc! {"_id":f.child.id.as_str()};
    let original = owners.find_one(filter.clone()).await.unwrap().unwrap();
    owners
        .update_one(filter.clone(), doc! {"$inc": {"epoch":1_i64}})
        .await
        .unwrap();
    assert_eq!(
        RetirementCaptureConsumer::admit(&f.store, &f.parent, &action, &captured).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(f.store.action(&f.parent.id).await.unwrap(), None);
    owners.replace_one(filter, original).await.unwrap();

    RetirementCaptureConsumer::admit(&f.store, &f.parent, &action, &captured)
        .await
        .unwrap();
    assert_eq!(f.store.action(&f.parent.id).await.unwrap(), Some(action));
    assert!(f.store.observation(&f.parent.id).await.unwrap().is_none());
    assert_eq!(phase(&f.parent_row().await), "observed");
    f.cleanup().await;
}
