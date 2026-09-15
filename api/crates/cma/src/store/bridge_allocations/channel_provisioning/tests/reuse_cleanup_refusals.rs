use super::*;
use crate::store::{agent_provenance, sandbox_mongo::record_bson};
use layer2_core::resource_access::ResourceOwner;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_refuses_missing_recreated_and_wrong_owner_evidence() {
    for corruption in [
        "missing_creation",
        "wrong_owner_kind",
        "recreated_agent",
        "misindexed_tree",
        "missing_startup",
    ] {
        let mut f = CleanupFixture::new(true).await;
        f.reset().await;
        let target = f.original.placement.target();
        match corruption {
            "missing_creation" => {
                f.db()
                    .collection::<Document>("agent_creation_provenance")
                    .delete_one(doc! {"_id":target.agent_id.as_str()})
                    .await
                    .unwrap();
            }
            "wrong_owner_kind" => {
                let row = f
                    .row("agent_creation_provenance", target.agent_id.as_str())
                    .await;
                let mut creation = agent_provenance::read(&row).unwrap();
                let ResourceOwner::Person { subject } = &creation.owner else {
                    panic!()
                };
                creation.owner = ResourceOwner::Organization {
                    subject: subject.clone(),
                };
                f.db()
                    .collection::<Document>("agent_creation_provenance")
                    .replace_one(
                        doc! {"_id":target.agent_id.as_str()},
                        agent_provenance::document(&creation).unwrap(),
                    )
                    .await
                    .unwrap();
            }
            "recreated_agent" => {
                let mut agent =
                    f.f.f
                        .original
                        .resources
                        .get_agent(&target.agent_id)
                        .await
                        .unwrap()
                        .unwrap();
                agent.creation_fingerprint = "f".repeat(64);
                f.db().collection::<Document>("agents").update_one(
                    doc! {"_id":target.agent_id.as_str()},
                    doc! {"$set":crate::store::agents::agent_replacement_fields(&agent).unwrap()},
                ).await.unwrap();
            }
            "misindexed_tree" => {
                f.db()
                    .collection::<Document>("worktrees")
                    .update_one(
                        doc! {"_id":target.worktree_id.as_str()},
                        doc! {"$set":{"workspace_id":"another-workspace"}},
                    )
                    .await
                    .unwrap();
            }
            "missing_startup" => {
                f.db()
                    .collection::<Document>("bridge_allocations")
                    .update_one(
                        doc! {"_id":&f.f.record.id},
                        doc! {"$unset":{"channel_reuse.startup_plan":""}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(f.request_cleanup().await.is_err(), "accepted {corruption}");
        f.assert_no_request().await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_refuses_stale_bridge_parent_and_revoked_human_custody() {
    for change_kind in ["bridge", "parent", "custody"] {
        let mut f = CleanupFixture::new(true).await;
        f.reset().await;
        let permit = f.permit().await;
        match change_kind {
            "bridge" => {
                let bridge =
                    f.f.f
                        .original
                        .bridges
                        .bridge(&f.f.record.identity.bridge_id)
                        .await
                        .unwrap()
                        .unwrap();
                f.f.f
                    .original
                    .svc
                    .mutate(
                        &caller(),
                        None,
                        change(
                            &bridge.id,
                            bridge.revision,
                            "cleanup-stale-bridge",
                            BridgeChange::Disable {
                                accepted_work: AcceptedWorkPolicy::DrainOriginal,
                            },
                        ),
                    )
                    .await
                    .unwrap();
            }
            "parent" => {
                f.f.pause().await;
            }
            "custody" => {
                let reference = &f.f.record.config.connector.custody;
                f.db()
                    .collection::<Document>("bridge_credentials")
                    .update_one(
                        doc! {"_id":reference.as_str()},
                        doc! {"$unset":{"control_plane":""}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(f
            .f
            .f
            .original
            .allocations
            .request_channel_reuse_cleanup(&permit, &f.request())
            .await
            .is_err());
        f.assert_no_request().await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_seal_requires_the_actual_reset_edge() {
    let mut f = CleanupFixture::new(false).await;
    let next = f.reset().await;
    let row = f.row("bridge_allocations", &next.id).await;
    let mut malformed = next.clone();
    malformed.created_at_ms += 1;
    malformed.updated_at_ms += 1;
    malformed.validate().unwrap();
    f.db()
        .collection::<Document>("bridge_allocations")
        .update_one(
            doc! {"_id":&next.id},
            doc! {"$set":{"record":record_bson(&malformed).unwrap()}},
        )
        .await
        .unwrap();
    assert!(
        f.request_cleanup().await.is_err(),
        "a timestamp unrelated to original reset is not its successor"
    );
    f.assert_no_request().await;
    f.db()
        .collection::<Document>("bridge_allocations")
        .replace_one(doc! {"_id":&next.id}, row)
        .await
        .unwrap();
    f.request_cleanup().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_seal_checks_and_fences_every_reset_generation() {
    let mut f = CleanupFixture::with_capacity(true, 8).await;
    let middle = f.reset().await;
    let head = f.f.f.reset(&middle, "67890").await.unwrap();
    let middle =
        f.f.f
            .original
            .allocations
            .generation(&middle.identity)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(middle.state, AllocationState::Retired);
    for corruption in ["middle_live", "middle_time", "head_retired"] {
        let mut malformed = if corruption == "head_retired" {
            head.clone()
        } else {
            middle.clone()
        };
        match corruption {
            "middle_live" => malformed.state = AllocationState::Reserved,
            "middle_time" => malformed.updated_at_ms += 1,
            "head_retired" => malformed.state = AllocationState::Retired,
            _ => unreachable!(),
        }
        malformed.validate().unwrap();
        let row = f.row("bridge_allocations", &malformed.id).await;
        f.db()
            .collection::<Document>("bridge_allocations")
            .update_one(
                doc! {"_id":&malformed.id},
                doc! {"$set":{"record":record_bson(&malformed).unwrap()}},
            )
            .await
            .unwrap();
        assert!(f.request_cleanup().await.is_err(), "accepted {corruption}");
        f.assert_no_request().await;
        f.db()
            .collection::<Document>("bridge_allocations")
            .replace_one(doc! {"_id":&malformed.id}, row)
            .await
            .unwrap();
    }
    f.request_cleanup().await.unwrap();
    for id in [&f.f.record.id, &middle.id, &head.id] {
        assert!(
            f.row("bridge_allocations", id)
                .await
                .get_i64("channel_reuse_cleanup_fence")
                .unwrap()
                > 0
        );
    }
    assert!(
        f.row("bridge_allocation_heads", &head.identity.head_id().unwrap())
            .await
            .get_i64("channel_reuse_cleanup_fence")
            .unwrap()
            > 0
    );
    f.close().await;
}
