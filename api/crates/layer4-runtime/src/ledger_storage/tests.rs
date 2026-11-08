use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;

use super::{connection, invalid, probe, storage_error, Storage};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-ledger-storage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn database(&self) -> PathBuf {
        self.0.join("database")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn probe_commits_reopens_and_refuses_an_existing_destination() {
    let fixture = Fixture::new();
    probe(&fixture.database()).await.unwrap();
    let before = std::fs::read(fixture.database().join("ledger.sqlite3")).unwrap();
    assert!(probe(&fixture.database()).await.is_err());
    assert_eq!(
        before,
        std::fs::read(fixture.database().join("ledger.sqlite3")).unwrap()
    );
    assert!(before.windows(6).any(|value| value == b"SQLite"));
}

#[tokio::test]
async fn connection_pragmas_are_verified_and_wrong_durability_is_refused() {
    let fixture = Fixture::new();
    let storage = Storage::open(fixture.database(), true, 32).await.unwrap();
    storage
        .call(|db| {
            connection::verify(db, 32)?;
            db.pragma_update(None, "synchronous", "FULL").unwrap();
            assert!(connection::verify(db, 32).is_err());
            db.pragma_update(None, "synchronous", "EXTRA").unwrap();
            connection::verify(db, 32)?;
            db.pragma_update(None, "foreign_keys", false).unwrap();
            assert!(connection::verify(db, 32).is_err());
            Ok(())
        })
        .await
        .unwrap();
    storage.close().await.unwrap();
    let reopened = Storage::open(fixture.database(), false, 32).await.unwrap();
    reopened
        .call(|db| connection::verify(db, 32))
        .await
        .unwrap();
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn lifetime_writer_lock_prevents_a_second_connection_and_releases_on_close() {
    let fixture = Fixture::new();
    let storage = Storage::open(fixture.database(), true, 32).await.unwrap();
    let error = Storage::open(fixture.database(), false, 32)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "mutation_ledger_writer_held");
    storage.close().await.unwrap();
    Storage::open(fixture.database(), false, 32)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn strict_values_unsigned_blob_fences_and_failed_transactions_survive_reopen() {
    let fixture = Fixture::new();
    probe(&fixture.database()).await.unwrap();
    let storage = Storage::open(fixture.database(), false, 256).await.unwrap();
    storage
        .call(|db| {
            assert!(db.execute("UPDATE probe SET fence = 1", []).is_err());
            assert!(db.execute("UPDATE probe SET fence = x'01'", []).is_err());
            let transaction = db.transaction().unwrap();
            transaction
                .execute("UPDATE probe SET fence = x'8000000000000000'", [])
                .unwrap();
            drop(transaction);
            let fence: Vec<u8> = db
                .query_row("SELECT fence FROM probe", [], |row| row.get(0))
                .unwrap();
            assert_eq!(fence, u64::MAX.to_be_bytes());
            Ok(())
        })
        .await
        .unwrap();
    storage.close().await.unwrap();
}

#[tokio::test]
async fn page_and_value_limits_fail_without_losing_committed_data() {
    let fixture = Fixture::new();
    let storage = Storage::open(fixture.database(), true, 16).await.unwrap();
    storage
        .call(|db| {
            db.execute_batch(
                "CREATE TABLE values_test (bytes BLOB NOT NULL) STRICT;
                              INSERT INTO values_test VALUES (x'01');",
            )
            .unwrap();
            assert!(db
                .execute("INSERT INTO values_test VALUES (zeroblob(100000))", [])
                .is_err());
            assert!(db
                .query_row("SELECT zeroblob(2097153)", [], |row| row
                    .get::<_, Vec<u8>>(0))
                .is_err());
            let count: i64 = db
                .query_row("SELECT count(*) FROM values_test", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
            Ok(())
        })
        .await
        .unwrap();
    storage.close().await.unwrap();
    Storage::open(fixture.database(), false, 16)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn unsafe_paths_unknown_files_and_invalid_limits_refuse() {
    let fixture = Fixture::new();
    assert!(Storage::open(fixture.database(), true, 0).await.is_err());
    assert!(!fixture.database().exists());
    assert!(Storage::open(fixture.database(), false, 32).await.is_err());
    probe(&fixture.database()).await.unwrap();
    symlink(fixture.database(), fixture.0.join("alias")).unwrap();
    assert!(Storage::open(fixture.0.join("alias"), false, 256)
        .await
        .is_err());
    std::fs::write(fixture.database().join("unexpected"), b"content").unwrap();
    assert!(Storage::open(fixture.database(), false, 256).await.is_err());
    std::fs::remove_file(fixture.database().join("unexpected")).unwrap();
    std::fs::set_permissions(fixture.database(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Storage::open(fixture.database(), false, 256).await.is_err());
}

#[tokio::test]
async fn symlinked_or_hardlinked_database_and_journal_are_not_opened() {
    for name in ["ledger.sqlite3", "ledger.sqlite3-journal", "writer.lock"] {
        let fixture = Fixture::new();
        probe(&fixture.database()).await.unwrap();
        let destination = fixture.database().join(name);
        if destination.exists() {
            std::fs::rename(&destination, fixture.0.join("outside")).unwrap();
        } else {
            std::fs::write(fixture.0.join("outside"), b"kept").unwrap();
        }
        symlink(fixture.0.join("outside"), &destination).unwrap();
        assert!(Storage::open(fixture.database(), false, 256).await.is_err());
        std::fs::remove_file(&destination).unwrap();
        std::fs::hard_link(fixture.0.join("outside"), &destination).unwrap();
        assert!(Storage::open(fixture.database(), false, 256).await.is_err());
    }
}

#[tokio::test]
async fn cancelled_wait_does_not_cancel_an_admitted_transaction() {
    let fixture = Fixture::new();
    let storage = Storage::open(fixture.database(), true, 32).await.unwrap();
    let (started, observed) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let mut transaction = Box::pin(storage.call(move |db| {
        started.send(()).unwrap();
        wait.recv().unwrap();
        db.execute_batch(
            "CREATE TABLE committed (value INTEGER); INSERT INTO committed VALUES (1);",
        )
        .map_err(|_| storage_error())
    }));
    tokio::select! {
        result = &mut transaction => panic!("unexpected early result {result:?}"),
        result = observed => result.unwrap(),
    }
    // Drop the caller future after acceptance, then let the worker finish.
    drop(transaction);
    release.send(()).unwrap();
    storage
        .call(|db| {
            let value: i64 = db
                .query_row("SELECT value FROM committed", [], |r| r.get(0))
                .map_err(|_| storage_error())?;
            if value != 1 {
                return Err(invalid());
            }
            Ok(())
        })
        .await
        .unwrap();
    storage.close().await.unwrap();
}

#[tokio::test]
async fn worker_queue_is_bounded_and_never_reports_raw_storage_errors() {
    let fixture = Fixture::new();
    let storage = Storage::open(fixture.database(), true, 32).await.unwrap();
    let (started, observed) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    storage
        .send(super::Message::Work(Box::new(move |_| {
            let _ = started.send(());
            wait.recv().unwrap();
        })))
        .unwrap();
    observed.await.unwrap();
    for _ in 0..super::QUEUE_LIMIT {
        storage
            .send(super::Message::Work(Box::new(|_| {})))
            .unwrap();
    }
    let error = storage.call(|_| Ok(())).await.unwrap_err();
    assert_eq!(error.code, "mutation_ledger_busy");
    assert!(!error.retryable);
    release.send(()).unwrap();
    // Wait for all queued work before requesting close.
    loop {
        match storage.call(|_| Err::<(), _>(storage_error())).await {
            Err(error) if error.code == "mutation_ledger_busy" => tokio::task::yield_now().await,
            Err(error) => {
                assert_eq!(
                    error.detail,
                    "runtime mutation ledger storage requires verification"
                );
                break;
            }
            Ok(()) => panic!("unexpected success"),
        }
    }
    storage.close().await.unwrap();
}
