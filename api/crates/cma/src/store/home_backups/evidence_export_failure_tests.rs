use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct ReplaceScope {
    admin: Client,
    database: String,
}
#[async_trait::async_trait]
impl export::PageHook for ReplaceScope {
    async fn after_page(&self, collection: &str, page: usize) -> export::Result<()> {
        if collection == BACKUPS_COLLECTION && page == 1 {
            let db = self.admin.database(&self.database);
            db.collection::<Document>("sandboxes").drop().await.unwrap();
            db.create_collection("sandboxes").await.unwrap();
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn catalog_export_rejects_collection_replacement_after_reading_to_eof() {
    let fixture = ExportFixture::new().await;
    fixture.insert_home("a").await;
    let hook = ReplaceScope {
        admin: fixture.fixture.client.clone(),
        database: fixture.fixture.database.clone(),
    };
    assert_eq!(
        fixture.export(Limits::default(), &hook).await.unwrap_err(),
        "catalog_export_scope_changed"
    );
    fixture.assert_read_only();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn unauthorized_export_and_unprepared_schema_refuse_before_catalog_reads() {
    let fixture = ExportFixture::new().await;
    let db = fixture.fixture.client.database(&fixture.fixture.database);
    db.run_command(doc! {"updateUser":"catalog-reader", "roles": []})
        .await
        .unwrap();
    assert_eq!(
        fixture.export(Limits::default(), &()).await.unwrap_err(),
        "catalog_export_authority_refused"
    );
    assert!(!fixture
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|(name, _)| name == "find"));
    fixture.assert_read_only();
    fixture.cleanup().await;
    let fixture = ExportFixture::new().await;
    fixture
        .fixture
        .client
        .database(&fixture.fixture.database)
        .collection::<Document>(SCHEDULES_COLLECTION)
        .drop()
        .await
        .unwrap();
    assert_eq!(
        fixture.export(Limits::default(), &()).await.unwrap_err(),
        "catalog_export_schema_unprepared"
    );
    assert!(!fixture
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|(name, _)| name == "find"));
    fixture.assert_read_only();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn real_export_preserves_all_catalog_states_and_refuses_unknown_or_lossy_bson() {
    let fixture = ExportFixture::new().await;
    fixture.insert_home("a").await;
    fixture.ready_history("a").await;
    let db = fixture.fixture.client.database(&fixture.fixture.database);
    let backups = db.collection(BACKUPS_COLLECTION);
    for (i, state) in [
        HomeBackupState::Requested,
        HomeBackupState::Creating,
        HomeBackupState::Ready,
        HomeBackupState::Failed,
        HomeBackupState::Expiring,
        HomeBackupState::Deleted,
    ]
    .into_iter()
    .enumerate()
    {
        let mut b = standalone_backup(state);
        b.sandbox_id = "historical".into();
        b.epoch = (i + 1) as u64;
        b.id = format!("hb-historical-{}", b.epoch);
        b.storage.source_reference = b
            .storage
            .source_reference
            .map(|_| format!("historical-snapshot-{i}"));
        backups
            .insert_one(backup_document(&b).unwrap())
            .await
            .unwrap();
    }
    let bytes = fixture.export(Limits::default(), &()).await.unwrap();
    assert_eq!(
        payload(&bytes)["collections"][BACKUPS_COLLECTION]["count"],
        8
    );
    assert_eq!(fixture.coverage(&bytes)["status"], "PASS");
    let normal = backups
        .find_one(doc! {"_id":"hb-a-1"})
        .await
        .unwrap()
        .unwrap();
    let mut exact = normal.clone();
    exact.insert("next_fence", (1_i64 << 53) - 1);
    backups
        .replace_one(doc! {"_id":"hb-a-1"}, exact)
        .await
        .unwrap();
    assert_eq!(
        fixture.coverage(&fixture.export(Limits::default(), &()).await.unwrap())["status"],
        "PASS"
    );
    for field in ["unknown_nested", "schema", "lossy"] {
        let mut invalid = normal.clone();
        match field {
            "unknown_nested" => {
                invalid
                    .get_document_mut("source")
                    .unwrap()
                    .insert("payload-sentinel", true);
            }
            "schema" => {
                invalid.insert("document_schema_version", 999);
            }
            _ => {
                invalid.insert("next_fence", 1_i64 << 53);
            }
        }
        backups
            .replace_one(doc! {"_id":"hb-a-1"}, invalid)
            .await
            .unwrap();
        assert!(fixture.export(Limits::default(), &()).await.is_err());
    }
    backups
        .replace_one(doc! {"_id":"hb-a-1"}, normal)
        .await
        .unwrap();
    // An ObjectId sorts after string IDs; keyset type bracketing must not hide it.
    db.collection("sandboxes")
        .insert_one(doc! {"_id": mongodb::bson::oid::ObjectId::new()})
        .await
        .unwrap();
    assert_eq!(
        fixture
            .export(
                Limits {
                    page_size: 1,
                    ..Limits::default()
                },
                &()
            )
            .await
            .unwrap_err(),
        "catalog_export_invalid_identity"
    );
    fixture.assert_read_only();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn real_export_private_publication_preserves_old_evidence_and_rejects_partial_output() {
    let fixture = ExportFixture::new().await;
    fixture.insert_home("a").await;
    let bytes = fixture.export(Limits::default(), &()).await.unwrap();
    let output = fixture.directory.join("existing.json");
    let receipt = export::cli::publish(&output, &bytes).unwrap();
    assert!(export::cli::publish(&output, &bytes).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    assert!(!receipt.contains("source-fixture"));
    assert!(!receipt.contains(&fixture.fixture.database));
    let link = fixture.directory.join("link.json");
    symlink(&output, &link).unwrap();
    assert!(export::cli::publish(&link, &bytes).is_err());
    let partial = fixture.directory.join("partial.json");
    assert!(export::cli::publish(&partial, &bytes[..bytes.len() / 2]).is_err());
    assert!(!partial.exists());
    std::fs::set_permissions(&fixture.directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(export::cli::publish(&partial, &bytes).is_err());
    std::fs::set_permissions(&fixture.directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    fixture.cleanup().await;
}
