use super::*;
use std::{io::Write, os::unix::fs::OpenOptionsExt};

#[tokio::test]
async fn file_completion_requires_the_latest_absent_authority_exact_completed_operation() {
    let files = Files::new();
    files.add("uncertain-remove", true);
    let newer = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        "agent:uncertain-remove:home:remove:v2",
        AgentSlot::new("uncertain-remove"),
    ));
    let record = Authority::new(&files.fixture.config, 2, &newer);
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(record.inventory_name(&files.fixture.config))
        .unwrap();
    file.write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    let archive = files.open(true).await;
    let revision = archive.observe_file_collection().await.unwrap();
    let group = CollectionGroup::Agent {
        agent_id: "uncertain-remove".into(),
    };
    let (binding, bytes) = super::super::super::collection_probe::file_artifacts(
        &files.fixture.config,
        &files.fixture.id,
        revision,
        group.clone(),
    )
    .unwrap();
    let evidence = CollectionEvidence::parse(&binding, &group.key(), bytes).unwrap();
    assert!(archive
        .prepare_file_collection(evidence, "a".repeat(64))
        .await
        .is_err());
    assert!(archive.pending_collection(binding).await.unwrap().is_some());
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
    assert_eq!(files.counts(), [1, 2]);
}

#[tokio::test]
async fn file_completion_refuses_source_census_damage_without_moving_selected_records() {
    let files = Files::new();
    files.add("unchanged", true);
    let archive = files.open(true).await;
    let (binding, reference, proof) = files.select(&archive, "unchanged").await;
    archive
        .storage
        .call(|db| {
            db.execute(
                "UPDATE file_collection_snapshot SET payload=zeroblob(length(payload))",
                [],
            )
            .map_err(|_| invalid())?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        complete_file_collection_held(archive, binding, reference, proof, ())
            .await
            .is_err()
    );
    assert_eq!(files.counts(), [1, 1]);
    let archive = files.open(false).await;
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
}

#[tokio::test]
async fn file_completion_census_refuses_unexpected_and_symlinked_generation_sources() {
    for symlink in [false, true] {
        let files = Files::new();
        files.add("untouched", true);
        let path = files.fixture.config.state_root.join("process-generation-1");
        if symlink {
            std::os::unix::fs::symlink(&files.fixture.config.home, &path).unwrap();
        } else {
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("unrecognized-record"), b"uncertain").unwrap();
        }
        let archive = files.open(true).await;
        assert!(archive.observe_file_collection().await.is_err());
        archive.close().await.unwrap();
        assert_eq!(files.counts(), [1, 1]);
    }
}
