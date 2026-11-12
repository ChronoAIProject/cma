use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn full_data_archive_hold_keeps_unconfirmed_uploads_and_refuses_purge() {
    use layer2_core::sandbox::{retirement_archive::RetirementArchiveRequirement, SandboxStatus};
    let (client, _, database) = test_store("archive_hold").await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let mut parent = sandbox("sbx_full_archive_hold");
    parent.status = SandboxStatus::Retiring;
    parent.work = SandboxWorkState::new(SandboxWorkStep::StopAgents, layer2_core::now_rfc3339());
    parent.lifecycle.retirement_archive = Some(RetirementArchiveRequirement {
        format: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        plan_sha256: "a".repeat(64),
        source_sha256: "b".repeat(64),
        exclusion_sha256: "c".repeat(64),
        verified: None,
    });
    resources.create_sandbox(parent.clone()).await.unwrap();
    let objects = &resources.checkpoint_objects;
    let original = b"retain incomplete source upload until full archive is verified";
    let (reference, sha256) = sealed(original);
    let identity =
        ArchiveIdentity::from_input(reference.clone(), sha256, original.len() as u64).unwrap();
    let mut stage = ManifestDoc::new(parent.id.as_str(), &identity, DateTime::now());
    stage.cleanup_after = DateTime::from_millis(1);
    super::super::publication::admit(objects, &stage)
        .await
        .unwrap();
    objects
        .objects
        .upload(&stage.object_key, upload(original.to_vec()))
        .await
        .unwrap();
    for seconds in [0, 61] {
        objects
            .cleanup_due(std::time::SystemTime::now() + std::time::Duration::from_secs(seconds))
            .await
            .unwrap();
        let retained = objects
            .manifests
            .find_one(doc! {"_id":&stage.id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.get_str("state").unwrap(), "uploading");
        assert!(objects
            .tombstones
            .find_one(doc! {"_id":&stage.id})
            .await
            .unwrap()
            .is_none());
        let retained = objects
            .objects
            .download(
                &stage.object_key,
                &reference,
                &identity.sha256,
                original.len() as u64,
            )
            .await
            .unwrap();
        assert_eq!(bytes(retained).await.unwrap(), original);
        assert!(objects.purge_sandbox(parent.id.as_str()).await.is_err());
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn channel_markers_retain_published_bytes_without_losing_uncertain_upload_cleanup() {
    let (client, _, database) = test_store("retain").await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let channel = retired_sandbox("sbx_channel_storage_retained");
    let ordinary = retired_sandbox("sbx_ordinary_storage_released");
    resources.create_sandbox(channel.clone()).await.unwrap();
    resources.create_sandbox(ordinary.clone()).await.unwrap();
    let published = stored_checkpoint(&resources, &channel, 1, b"retained published bytes").await;
    let ordinary_archive =
        stored_checkpoint(&resources, &ordinary, 1, b"ordinary retired bytes").await;
    // A malformed Channel marker must not turn into generic delete authority.
    resources
        .sandboxes
        .update_one(
            doc! {"_id":channel.id.as_str()},
            doc! {"$set":{"channel_creation":mongodb::bson::Bson::Null}},
        )
        .await
        .unwrap();
    let objects = &resources.checkpoint_objects;
    let late_bytes = b"uncertain remote upload may finish later";
    let (reference, sha256) = sealed(late_bytes);
    let identity =
        ArchiveIdentity::from_input(reference.clone(), sha256, late_bytes.len() as u64).unwrap();
    let mut stage = ManifestDoc::new(channel.id.as_str(), &identity, DateTime::now());
    stage.cleanup_after = DateTime::from_millis(1);
    super::super::publication::admit(objects, &stage)
        .await
        .unwrap();
    objects
        .gates
        .update_one(
            doc! {"_id":channel.id.as_str()},
            doc! {"$set":{"retired":true,"channel_storage_retention":"retained-fixture"}},
        )
        .await
        .unwrap();
    objects
        .objects
        .upload(&stage.object_key, upload(late_bytes.to_vec()))
        .await
        .unwrap();
    assert!(
        super::super::publication::publish(objects, &stage, &identity)
            .await
            .is_err()
    );
    assert!(objects.purge_sandbox(channel.id.as_str()).await.is_err());
    let now = std::time::SystemTime::now();
    objects.cleanup_due(now).await.unwrap();
    assert_archive_present(&resources, &channel, &published.object_reference).await;
    assert!(objects
        .for_sandbox(ordinary.id.as_str())
        .unwrap()
        .download(&ordinary_archive.object_reference)
        .await
        .is_err());
    let tombstone = objects
        .tombstones
        .find_one(doc! {"_id":&stage.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tombstone.get_str("object_key").unwrap(), stage.object_key);
    assert!(objects
        .manifests
        .find_one(doc! {"_id":&stage.id})
        .await
        .unwrap()
        .is_none());
    // Simulate the original remote write finishing after the observed deletion.
    objects
        .objects
        .upload(&stage.object_key, upload(late_bytes.to_vec()))
        .await
        .unwrap();
    objects
        .cleanup_due(now + std::time::Duration::from_secs(61))
        .await
        .unwrap();
    assert!(objects
        .objects
        .download(
            &stage.object_key,
            &reference,
            &identity.sha256,
            late_bytes.len() as u64
        )
        .await
        .is_err());
    assert!(objects
        .tombstones
        .find_one(doc! {"_id":&stage.id})
        .await
        .unwrap()
        .is_some());
    assert_archive_present(&resources, &channel, &published.object_reference).await;
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn retained_roots_do_not_starve_ordinary_reused_parent_archive_cleanup() {
    let (client, _, database) = test_store("retention_page").await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let allocations = client
        .database(&database)
        .collection::<mongodb::bson::Document>("bridge_allocations");
    for index in 0..65 {
        let root = retired_sandbox(&format!("sbx_a_retained_{index:03}"));
        resources.create_sandbox(root.clone()).await.unwrap();
        stored_checkpoint(&resources, &root, 1, b"retain each root archive").await;
        if index % 2 == 0 {
            resources
                .sandboxes
                .update_one(
                    doc! {"_id":root.id.as_str()},
                    doc! {"$set":{"channel_creation":mongodb::bson::Bson::Null}},
                )
                .await
                .unwrap();
        } else {
            // Missing root markers must still honor retained creation evidence.
            allocations.insert_one(doc! {"_id":format!("retained-{index}"),
                "creates_sandbox":true,
                "record":{"placement":{"kind":"channel_sandbox","target":{"sandbox_id":root.id.as_str()}}},
            }).await.unwrap();
        }
    }
    let ordinary = retired_sandbox("sbx_z_ordinary_reused_parent");
    resources.create_sandbox(ordinary.clone()).await.unwrap();
    let archive = stored_checkpoint(&resources, &ordinary, 1, b"ordinary parent archive").await;
    allocations.insert_one(doc! {"_id":"ordinary-reuse", "creates_sandbox":false,
        "record":{"placement":{"kind":"reuse_parent","target":{"sandbox_id":ordinary.id.as_str()}}},
    }).await.unwrap();
    resources
        .checkpoint_objects
        .cleanup_due(std::time::SystemTime::now())
        .await
        .unwrap();
    assert!(resources
        .checkpoint_objects
        .for_sandbox(ordinary.id.as_str())
        .unwrap()
        .download(&archive.object_reference)
        .await
        .is_err());
    assert_eq!(
        resources
            .checkpoint_objects
            .manifests
            .count_documents(doc! {"state":"published"})
            .await
            .unwrap(),
        65
    );
    client.database(&database).drop().await.unwrap();
}
