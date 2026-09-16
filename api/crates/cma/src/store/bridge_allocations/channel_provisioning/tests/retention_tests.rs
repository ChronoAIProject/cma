use super::*;
use crate::sandbox::channel_retirement::retention::{ChannelStorageRetention, StorageDisposition};
use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
#[path = "retention_fixture.rs"]
mod support;
use support::*;
#[path = "final_retention_tests.rs"]
mod final_assessment;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_keeps_the_exact_persistent_home_after_runtime_teardown() {
    let f = with_persistent_home().await;
    let before = f.sandbox().await;
    let storage = f.f.original.adapter.home_volumes().unwrap();
    let inventory = storage.inventory().await.unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(
        before.lifecycle.home.as_ref().unwrap().attachment(),
        Some(inventory[0].clone())
    );
    f.step().await;
    let retained = retention(&f).await.unwrap();
    assert_eq!(retained.home, before.lifecycle.home);
    assert_eq!(storage.inventory().await.unwrap(), inventory);
    assert!(!f.sandbox().await.lifecycle.home.unwrap().released);
    assert!(f.sandbox().await.retired_at.is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_blocks_ordinary_home_release_with_stripped_provenance() {
    use crate::sandbox::channel_credentials::SandboxCredentialProvenance;
    for evidence in ["retention_only", "placement_only"] {
        let f = with_persistent_home().await;
        f.step().await;
        let before = f.sandbox().await;
        let storage = f.f.original.adapter.home_volumes().unwrap();
        let inventory = storage.inventory().await.unwrap();
        let mut removed = doc! {"channel_creation":"", "channel_provisioning":"",
        "channel_renewal":"", "channel_lifecycle_credential":"",
        "channel_mint_reconciliation":"", "channel_retirement":""};
        let allocations = f.db().collection::<Document>("bridge_allocations");
        if evidence == "retention_only" {
            allocations.delete_many(doc! {}).await.unwrap();
        } else {
            removed.insert("channel_storage_retention", "");
            allocations
                .update_many(doc! {}, doc! {"$unset":{"sandbox_admission":""}})
                .await
                .unwrap();
        }
        f.db()
            .collection::<Document>("sandboxes")
            .update_one(doc! {"_id":before.id.as_str()}, doc! {"$unset":removed})
            .await
            .unwrap();
        assert!(
            f.f.original
                .allocations
                .require_ordinary(&before.id)
                .await
                .is_err(),
            "{evidence}"
        );
        f.retry().await;
        // This is the real worker path. The fixture's volume remove panics if
        // provenance ever admits ordinary MarkRetired before archive refusal.
        f.step().await;
        assert_eq!(f.sandbox().await.lifecycle.home, before.lifecycle.home);
        assert_eq!(storage.inventory().await.unwrap(), inventory);
        assert_eq!(f.sandbox().await.status, SandboxStatus::Retiring);
        assert!(f.sandbox().await.retired_at.is_none());
        f.close().await;
    }
    let f = RetirementFixture::new(0).await;
    let ordinary =
        crate::store::sandbox_mongo_integration_tests::sandbox("sbx_retention_ordinary_parent");
    f.f.original
        .resources
        .create_sandbox(ordinary.clone())
        .await
        .unwrap();
    for kind in ["reuse_parent", "existing_agent"] {
        f.db()
            .collection::<Document>("bridge_allocations")
            .insert_one(doc! {
                "_id":format!("ordinary-{kind}"),"bridge_id":format!("ordinary-{kind}"),"creates_sandbox":false,
                "record":{"placement":{"kind":kind,"target":{"sandbox_id":ordinary.id.as_str()}}},
            })
            .await
            .unwrap();
        assert!(f
            .f
            .original
            .allocations
            .require_ordinary(&ordinary.id)
            .await
            .is_ok());
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_seals_writers_preserves_published_storage_and_reopens() {
    let f = with_checkpoint().await;
    let before = f.sandbox().await;
    let history = rows(&f, "agent_log").await;
    let checkpoints = rows(&f, "checkpoints").await;
    let archives = rows(&f, "checkpoint_objects").await;
    f.step().await;
    let retained = retention(&f).await.unwrap();
    assert_eq!(retained.disposition, StorageDisposition::RetainForRecovery);
    assert_eq!(retained.allocation_id, f.f.original.record.id);
    assert_eq!(
        retained.original_human,
        f.f.original.lease.actor_context().actor
    );
    assert_eq!(
        retained.runtime,
        f.receipt().await.unwrap().runtime.unwrap()
    );
    assert_eq!(retained.checkpoints, before.lifecycle.checkpoints);
    assert_eq!(retained.home, before.lifecycle.home);
    assert_eq!(retained.archives.len(), 1);
    assert_eq!(rows(&f, "agent_log").await, history);
    assert_eq!(rows(&f, "checkpoints").await, checkpoints);
    assert_eq!(rows(&f, "checkpoint_objects").await, archives);
    let after = f.sandbox().await;
    assert_eq!(after.status, SandboxStatus::Retiring);
    assert_eq!(after.work.step, SandboxWorkStep::MarkRetired);
    assert!(after.retired_at.is_none() && after.purge_after.is_none());
    let reopened = crate::store::MongoCheckpointObjectStore::open(
        &f.f.original.client,
        &f.f.original.database,
    )
    .await
    .unwrap();
    let authority = reopened.for_sandbox(after.id.as_str()).unwrap();
    assert_eq!(
        bytes(
            authority
                .download(&retained.archives[0].object_reference)
                .await
                .unwrap()
        )
        .await,
        ARCHIVE
    );
    assert!(authority.upload(upload(b"late new archive")).await.is_err());
    assert!(authority.upload(upload(ARCHIVE)).await.is_err());
    assert!(reopened.purge_sandbox(after.id.as_str()).await.is_err());
    assert_eq!(reopened.cleanup_due(SystemTime::now()).await.unwrap(), 0);
    f.step().await;
    assert_eq!(retention(&f).await, Some(retained.clone()));
    assert_eq!(rows(&f, "checkpoint_objects").await, archives);
    let audit = f
        .db()
        .collection::<Document>("channel_retirement_audits")
        .find_one(doc! {"action":"storage_retained"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        audit.get("storage_retention"),
        Some(&crate::store::sandbox_mongo::record_bson(&retained).unwrap())
    );
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_blocks_generic_purge_with_retired_status_and_stripped_markers() {
    let f = with_checkpoint().await;
    f.step().await;
    let mut retired = f.sandbox().await;
    retired.status = SandboxStatus::Retired;
    retired.retired_at = Some(layer2_core::now_rfc3339());
    retired.purge_after = retired.retired_at.clone();
    retired.work.step = SandboxWorkStep::Purge;
    replace_sandbox(&f, retired).await;
    let preserved = [
        "agent_log",
        "agents",
        "workspaces",
        "worktrees",
        "checkpoints",
        "checkpoint_objects",
    ];
    let mut originals = Vec::new();
    for collection in preserved {
        originals.push((collection, rows(&f, collection).await));
    }
    for strip in [false, true] {
        if strip {
            f.db().collection::<Document>("sandboxes").update_one(
                doc! {"_id":f.sandbox().await.id.as_str()},
                doc! {"$unset":{"channel_creation":"", "channel_provisioning":"", "channel_retirement":"", "channel_storage_retention":""}},
            ).await.unwrap();
        }
        let sandbox = f.sandbox().await;
        let objects = &f.f.original.resources.checkpoint_objects;
        assert!(objects.purge_sandbox(sandbox.id.as_str()).await.is_err());
        assert_eq!(objects.cleanup_due(SystemTime::now()).await.unwrap(), 0);
        assert!(f
            .f
            .original
            .resources
            .purge_sandbox(&sandbox.id, sandbox.revision, &layer2_core::now_rfc3339())
            .await
            .is_err());
        for (collection, original) in &originals {
            assert_eq!(&rows(&f, collection).await, original, "{collection}");
        }
        assert!(f
            .f
            .original
            .resources
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .is_some());
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_refuses_missing_original_human_before_writer_seal() {
    let f = with_checkpoint().await;
    f.db()
        .collection::<Document>("bridge_credentials")
        .update_many(doc! {}, doc! {"$unset":{"control_plane":""}})
        .await
        .unwrap();
    let archives = rows(&f, "checkpoint_objects").await;
    f.step().await;
    assert!(retention(&f).await.is_none());
    assert_eq!(rows(&f, "checkpoint_objects").await, archives);
    let gate = rows(&f, "checkpoint_archive_gates").await;
    assert_eq!(gate.len(), 1);
    assert!(!gate[0].get_bool("retired").unwrap());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_refuses_missing_checkpoint_archive_and_unproved_home() {
    for change in ["missing_archive", "unproved_home"] {
        let f = with_checkpoint().await;
        if change == "missing_archive" {
            f.db()
                .collection::<Document>("checkpoint_objects")
                .delete_many(doc! {})
                .await
                .unwrap();
        } else {
            let mut sandbox = f.sandbox().await;
            sandbox.lifecycle.home = Some(Box::new(
                layer2_core::sandbox::SandboxHome::reserve(
                    &sandbox.id,
                    layer3_adapter::HomeVolumePolicy {
                        storage_class: "fixture".into(),
                        capacity_gib: 1,
                        template_name: "fixture".into(),
                        template_generation: "fixture".into(),
                    },
                )
                .unwrap(),
            ));
            replace_sandbox(&f, sandbox).await;
        }
        f.step().await;
        assert!(retention(&f).await.is_none(), "{change}");
        assert!(!rows(&f, "checkpoint_archive_gates").await[0]
            .get_bool("retired")
            .unwrap());
        assert!(f.sandbox().await.retired_at.is_none());
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retention_rejects_changed_inventory_or_reopened_gate_without_replacing_receipt() {
    for change in ["archive", "gate"] {
        let f = with_checkpoint().await;
        f.step().await;
        let retained = retention(&f).await.unwrap();
        if change == "archive" {
            f.db()
                .collection::<Document>("checkpoint_objects")
                .update_one(
                    doc! {"_id":&retained.archives[0].manifest_id},
                    doc! {"$set":{"size_bytes":retained.archives[0].size_bytes as i64 + 1}},
                )
                .await
                .unwrap();
        } else {
            f.db()
                .collection::<Document>("checkpoint_archive_gates")
                .update_one(
                    doc! {"_id":retained.sandbox_id.as_str()},
                    doc! {"$set":{"retired":false}},
                )
                .await
                .unwrap();
            assert!(f
                .f
                .original
                .resources
                .checkpoint_objects
                .for_sandbox(retained.sandbox_id.as_str())
                .unwrap()
                .upload(upload(b"forged reopen must not publish"))
                .await
                .is_err());
        }
        let before = rows(&f, "checkpoint_objects").await;
        f.step().await;
        assert_eq!(retention(&f).await, Some(retained));
        assert_eq!(rows(&f, "checkpoint_objects").await, before);
        assert_eq!(
            f.db()
                .collection::<Document>("channel_retirement_audits")
                .count_documents(doc! {"action":"storage_retained"})
                .await
                .unwrap(),
            1
        );
        assert_eq!(f.sandbox().await.status, SandboxStatus::Retiring);
        f.close().await;
    }
}
