use std::os::unix::fs::{symlink, MetadataExt};

use super::tests::Fixture;
use super::{reserve, storage_error, Storage};

#[tokio::test]
async fn emergency_reserve_has_real_blocks_and_a_spent_hold_survives_reopen() {
    let fixture = Fixture::new();
    let storage = Storage::open(fixture.database(), true, 256).await.unwrap();
    let path = fixture.database().join("emergency.reserve");
    let metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(metadata.len(), reserve::BYTES);
    assert!(metadata.blocks() * 512 >= reserve::BYTES);
    storage
        .call(|db| {
            db.execute_batch("CREATE TABLE intact (value INTEGER); INSERT INTO intact VALUES (7)")
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert!(storage
        .call(|_| Err::<(), _>(storage_error()))
        .await
        .is_err());
    assert_eq!(reserve::remaining(&fixture.database()).unwrap(), 0);
    assert!(reserve::require(&fixture.database()).is_err());
    storage.close().await.unwrap();
    let storage = Storage::open(fixture.database(), false, 256).await.unwrap();
    assert_eq!(reserve::remaining(&fixture.database()).unwrap(), 0);
    storage
        .call(|db| {
            assert_eq!(
                db.query_row("SELECT value FROM intact", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                7
            );
            Ok(())
        })
        .await
        .unwrap();
    storage.close().await.unwrap();
}

#[tokio::test]
async fn sparse_missing_or_partial_reserve_cannot_claim_emergency_capacity() {
    for damage in ["sparse", "missing", "partial"] {
        let fixture = Fixture::new();
        Storage::open(fixture.database(), true, 256)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        let path = fixture.database().join("emergency.reserve");
        if damage == "missing" {
            std::fs::remove_file(&path).unwrap();
        } else {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(0).unwrap();
            file.set_len(if damage == "sparse" {
                reserve::BYTES
            } else {
                1
            })
            .unwrap();
            file.sync_all().unwrap();
        }
        assert!(Storage::open(fixture.database(), false, 256).await.is_err());
    }
}

#[tokio::test]
async fn storage_refuses_an_ancestor_symlink_before_creating_or_opening_files() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("real")).unwrap();
    symlink(fixture.0.join("real"), fixture.0.join("alias")).unwrap();
    assert!(Storage::open(fixture.0.join("alias/database"), true, 256)
        .await
        .is_err());
    assert!(!fixture.0.join("real/database").exists());
    Storage::open(fixture.0.join("real/database"), true, 256)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    assert!(Storage::open(fixture.0.join("alias/database"), false, 256)
        .await
        .is_err());
}
