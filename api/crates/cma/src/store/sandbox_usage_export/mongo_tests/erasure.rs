use super::*;
use crate::store::account_erasure::{erase_records, Cursor, Target};
use futures_util::TryStreamExt;
use layer2_core::{account_deletion::ErasureInventory, sandbox::SandboxStatus, SubjectId};

async fn snapshot(fixture: &Fixture, name: &str) -> Vec<Document> {
    fixture
        .db
        .collection(name)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

async fn erase(fixture: &Fixture, inventory: &ErasureInventory) {
    let subject = SubjectId::new("erased-owner");
    let mut cursor = Cursor::default();
    let mut deleted = 0;
    let mut finished = false;
    fixture.commands.lock().unwrap().clear();
    for _ in 0..2000 {
        let batch = erase_records(
            &fixture.db,
            &[Target {
                subject: &subject,
                inventory,
            }],
            &mut cursor,
            "2026-11-20T00:00:00Z",
        )
        .await
        .unwrap();
        assert!(batch.discovered.is_empty());
        assert!(batch.pending_key_ids.is_empty());
        for name in COLLECTIONS {
            assert!(!batch.deleted.contains_key(*name));
            assert!(!batch.redacted.contains_key(*name));
        }
        deleted += batch.deleted.get("users").copied().unwrap_or(0);
        if !batch.has_more {
            finished = true;
            break;
        }
        cursor = mongodb::bson::deserialize_from_document(
            mongodb::bson::serialize_to_document(&cursor).unwrap(),
        )
        .unwrap();
    }
    assert!(finished, "account erasure did not finish");
    assert_eq!(deleted, 1);
    for command in fixture.commands.lock().unwrap().iter() {
        let text = command.to_string();
        assert!(
            !COLLECTIONS.iter().any(|name| text.contains(name)),
            "ordinary erasure accessed retained usage: {text}"
        );
    }
    assert_eq!(
        snapshot(fixture, "users").await,
        vec![doc! {"_id":"foreign-owner"}]
    );
}

async fn users(fixture: &Fixture) {
    fixture
        .db
        .collection::<Document>("users")
        .insert_many([doc! {"_id":"erased-owner"}, doc! {"_id":"foreign-owner"}])
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn account_erasure_with_capture_disabled_never_opens_usage_namespaces() {
    let fixture = Fixture::new(false).await;
    users(&fixture).await;
    erase(&fixture, &ErasureInventory::default()).await;
    assert!(!fixture
        .db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| COLLECTIONS.contains(&name.as_str())));
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn account_erasure_preserves_owned_and_foreign_usage_and_unprojected_sources() {
    let fixture = Fixture::new(true).await;
    users(&fixture).await;
    let mut owned = fixture.parent("erased-owner").await;
    let foreign = fixture.parent("foreign-owner").await;
    let capture = fixture.capture();
    capture.baseline(&fixture.store, MINUTE, 32).await.unwrap();
    for parent in [&owned, &foreign] {
        for revision in 1..=2 {
            assert!(capture
                .project_one(&source_id(parent, revision), now())
                .await
                .unwrap());
        }
    }
    owned.revision += 1;
    owned.updated_at = TWO_MINUTES.into();
    owned.status = SandboxStatus::Retired;
    owned.runtime = None;
    owned.credential.current = None;
    owned.credential.delivered = false;
    owned.retired_at = Some(TWO_MINUTES.into());
    owned.purge_after = Some(TWO_MINUTES.into());
    owned.work = layer2_core::sandbox::SandboxWorkState::new(
        layer2_core::sandbox::SandboxWorkStep::None,
        TWO_MINUTES.into(),
    );
    fixture.store.save_sandbox(1, owned.clone()).await.unwrap();
    let mut before = Vec::new();
    for name in COLLECTIONS {
        let rows = snapshot(&fixture, name).await;
        assert!(!rows.is_empty(), "writer did not populate {name}");
        if *name != INVENTORY {
            for owner in ["erased-owner", "foreign-owner"] {
                assert!(rows
                    .iter()
                    .any(|row| row.get_str("owner").ok() == Some(owner)));
            }
        }
        before.push((*name, rows));
    }
    erase(
        &fixture,
        &ErasureInventory {
            sandboxes: vec![owned.id.to_string()],
            ..Default::default()
        },
    )
    .await;
    for (name, rows) in &before {
        assert_eq!(&snapshot(&fixture, name).await, rows, "{name}");
    }
    assert!(fixture
        .store
        .get_sandbox(&owned.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        fixture.store.get_sandbox(&foreign.id).await.unwrap(),
        Some(foreign)
    );
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), config())
        .await
        .unwrap();
    assert!(reopened
        .project_one(&source_id(&owned, 3), now())
        .await
        .unwrap());
    let frozen = snapshot(&fixture, OUTBOX).await;
    assert!(!reopened
        .project_one(&source_id(&owned, 3), now())
        .await
        .unwrap());
    assert_eq!(snapshot(&fixture, OUTBOX).await, frozen);
    assert_eq!(fixture.count("auth_subjects").await, 0);
    assert_eq!(
        snapshot(&fixture, "users").await,
        vec![doc! {"_id":"foreign-owner"}]
    );
    assert!(fixture
        .store
        .get_sandbox(&owned.id)
        .await
        .unwrap()
        .is_none());
    fixture.db.drop().await.unwrap();
}
