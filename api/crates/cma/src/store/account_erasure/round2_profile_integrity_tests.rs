//! Damaged graph identity must block before a surviving transitive pin is erased.
use super::*;
use crate::store::trigger_definitions::integration_tests::seeded;
use layer2_core::{
    profiles::{SandboxProfileSnapshot, SandboxProfileStore, WorkspaceProfileStore},
    sandbox::store::SandboxStore,
};
use mongodb::bson::{doc, Document};

const NOW: &str = "2026-11-14T00:00:00Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn damaged_profile_revision_identity_blocks_before_transitive_content_is_removed() {
    for damage in [
        "missing_id",
        "wrong_id",
        "missing_revision",
        "wrong_revision",
        "revision_type",
        "missing_edge",
        "none",
    ] {
        let seed = seeded("round2_profile_graph_integrity").await;
        let db = seed.client.database(&seed.database);
        let store = crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
        let mut sandbox =
            crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
                "sbx_graph_visitor",
            );
        sandbox.owner = SubjectId::new("visitor");
        let pinned = seed
            .profiles
            .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
            .await
            .unwrap()
            .unwrap();
        sandbox.profile = SandboxProfileSnapshot::from(&pinned);
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let workspace = db.collection::<Document>("workspace_profile_revisions");
        let filter = doc! {"profile_id":seed.target.workspace_profile.id.as_str(),"revision":1_i64};
        let original = workspace.find_one(filter.clone()).await.unwrap().unwrap();
        let unrelated = doc! {"_id":"unrelated-corrupt-profile-revision",
        "profile_id":"wkp_unrelated","record":{}};
        workspace.insert_one(unrelated.clone()).await.unwrap();
        match damage {
            "missing_id" => {
                workspace
                    .update_one(filter.clone(), doc! {"$unset":{"profile_id":""}})
                    .await
                    .unwrap();
            }
            "wrong_id" => {
                workspace
                    .update_one(filter.clone(), doc! {"$set":{"profile_id":"wkp_unrelated"}})
                    .await
                    .unwrap();
            }
            "missing_revision" => {
                workspace
                    .update_one(filter.clone(), doc! {"$unset":{"revision":""}})
                    .await
                    .unwrap();
            }
            "wrong_revision" => {
                workspace
                    .update_one(filter.clone(), doc! {"$set":{"revision":9_i64}})
                    .await
                    .unwrap();
            }
            "revision_type" => {
                workspace
                    .update_one(filter.clone(), doc! {"$set":{"revision":"first"}})
                    .await
                    .unwrap();
            }
            "missing_edge" => {
                workspace.delete_one(filter.clone()).await.unwrap();
            }
            "none" => {}
            _ => unreachable!(),
        }
        let damaged = workspace
            .find_one(doc! {"_id":original.get("_id").unwrap()})
            .await
            .unwrap();
        let agent = db
            .collection::<Document>("agent_profile_revisions")
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap();
        let mut inventory = ErasureInventory::default();
        let mut cursor = Cursor::default();
        let mut failure = None;
        let mut finished = false;
        for _ in 0..1000 {
            match erase_records(
                &db,
                &[Target {
                    subject: &seed.owner,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            {
                Ok(batch) => {
                    progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
                    if !batch.has_more {
                        finished = true;
                        break;
                    }
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if damage == "none" {
            assert!(
                finished && failure.is_none(),
                "valid transitive pins must remain usable: {failure:?}"
            );
            assert!(seed
                .profiles
                .workspace_profile_revision(&seed.target.workspace_profile.id, 1)
                .await
                .unwrap()
                .is_some());
        } else {
            let Some(Error::Blocked(block)) = failure else {
                panic!("damaged {damage} must block before effects, got {failure:?}");
            };
            assert_eq!(
                block.subject_hash,
                layer2_core::account_deletion::subject_hash(&seed.owner)
            );
            assert_eq!(
                block.reason,
                ErasureBlock::CorruptRecord {
                    collection: "workspace_profile_revisions".into()
                }
            );
            assert_eq!(
                workspace
                    .find_one(doc! {"_id":original.get("_id").unwrap()})
                    .await
                    .unwrap(),
                damaged
            );
            let parent = db
                .collection::<Document>("agent_profiles")
                .find_one(doc! {"_id":seed.target.agent_profile.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(parent.get_str("owner").unwrap(), seed.owner.as_str());
        }
        assert_eq!(
            db.collection::<Document>("agent_profile_revisions")
                .find_one(doc! {"_id":agent.get("_id").unwrap()})
                .await
                .unwrap(),
            Some(agent)
        );
        assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        assert_eq!(
            workspace
                .find_one(doc! {"_id":unrelated.get("_id").unwrap()})
                .await
                .unwrap(),
            Some(unrelated)
        );
        db.drop().await.unwrap();
    }
}
