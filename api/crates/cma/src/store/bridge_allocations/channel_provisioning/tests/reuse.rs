//! Disposable shared parent fixtures. The seed is constructed before reuse
//! assertions; no live parent, platform, provider or production fixture is used.
use super::*;
use crate::sandbox::{
    channel_reuse::{startup::ChannelReuseStartupDriver, ChannelReuseDriver},
    OrnnAgentSkillResolver,
};
use crate::store::MongoBridgeInvokeStore;
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore, WorktreeStore};
use layer2_core::{resource_access::*, sandbox::*};
use mongodb::bson::Document;
#[path = "reuse/fixture.rs"]
pub(crate) mod fixture;
use fixture::ReuseFixture;
#[path = "reuse/faults.rs"]
mod faults;
#[path = "reuse/progress.rs"]
mod progress;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reuse_concurrent_first_messages_reserve_distinct_children_and_protect_owner_capacity() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let (one, proof_one) = f.proposal("sender-one").await;
        let duplicate = one.clone();
        let (two, proof_two) = f.proposal("sender-two").await;
        let write_one = AllocationReservation::new(one.clone(), proof_one.clone(), now()).unwrap();
        let write_two = AllocationReservation::new(two, proof_two, now()).unwrap();
        let (one, two) = tokio::join!(
            f.original.allocations.reserve(write_one),
            f.original.allocations.reserve(write_two)
        );
        let one = one.unwrap();
        let two = two.unwrap();
        assert_ne!(one.id, two.id);
        assert_ne!(
            one.placement.target().agent_id,
            two.placement.target().agent_id
        );
        assert_eq!(
            one.placement.target().sandbox_id,
            two.placement.target().sandbox_id
        );
        assert_eq!(
            one.placement.target().workspace_id == two.placement.target().workspace_id,
            shared_workspace
        );
        assert!(
            f.original
                .allocations
                .reserve(AllocationReservation::new(duplicate, proof_one, now()).unwrap())
                .await
                .unwrap()
                == one
        );
        let (third, proof) = f.proposal("sender-three").await;
        assert!(matches!(
            f.original
                .allocations
                .reserve(AllocationReservation::new(third, proof, now()).unwrap())
                .await,
            Err(AllocationError::Capacity)
        ));
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap(),
            f.parent
        );
        let rows = f
            .original
            .client
            .database(&f.original.database)
            .collection::<Document>("bridge_allocations");
        assert_eq!(
            rows.count_documents(doc! {"sandbox_admission":{"$exists":true}})
                .await
                .unwrap(),
            0
        );
        f.original.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reuse_child_startup_preserves_parent_and_sibling_with_distinct_runtime_paths() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let mut ready = Vec::new();
        for sender in ["sender-one", "sender-two"] {
            let (admission, proof) = f.proposal(sender).await;
            let mut record = f
                .original
                .allocations
                .reserve(AllocationReservation::new(admission, proof, now()).unwrap())
                .await
                .unwrap();
            for _ in 0..8 {
                let proof = f.authority(&record).await;
                record = f
                    .driver()
                    .run_one(record, &proof, &f.original.lease)
                    .await
                    .unwrap();
                if record.state == AllocationState::Ready {
                    break;
                }
                assert!(
                    record.lease.is_none(),
                    "completed stage must allow immediate scheduler continuation"
                );
            }
            assert_eq!(record.state, AllocationState::Ready);
            assert!(
                f.original
                    .allocations
                    .current(&record.identity)
                    .await
                    .unwrap()
                    .unwrap()
                    == record
            );
            ready.push(record);
        }
        let one = f
            .original
            .resources
            .get_worktree(&ready[0].placement.target().worktree_id)
            .await
            .unwrap()
            .unwrap();
        let two = f
            .original
            .resources
            .get_worktree(&ready[1].placement.target().worktree_id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(one.id, two.id);
        assert_ne!(one.path, two.path);
        assert_eq!(one.workspace_id == f.workspace.id, shared_workspace);
        if shared_workspace {
            assert_ne!(one.branch, two.branch);
            assert_ne!(one.name, two.name);
        }
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap(),
            f.parent
        );
        assert_eq!(
            f.original
                .resources
                .get_agent(&f.sibling.id)
                .await
                .unwrap()
                .unwrap(),
            f.sibling
        );
        let rows = f
            .original
            .client
            .database(&f.original.database)
            .collection::<Document>("sandboxes");
        assert_eq!(
            rows.count_documents(doc! {"channel_provisioning":{"$exists":true}})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            f.original.server.state.mint_count.load(Ordering::SeqCst),
            1,
            "only fixture bootstrap minted a credential"
        );
        for record in &ready {
            let row = f
                .original
                .client
                .database(&f.original.database)
                .collection::<Document>("agent_creation_provenance")
                .find_one(doc! {"_id":record.placement.target().agent_id.as_str()})
                .await
                .unwrap()
                .unwrap();
            let origin = crate::store::agent_provenance::read(&row).unwrap();
            assert!(matches!(
                origin.creation_origin,
                AgentCreationOrigin::ChannelBridge { .. }
            ));
        }
        f.original.close().await;
    }
}
