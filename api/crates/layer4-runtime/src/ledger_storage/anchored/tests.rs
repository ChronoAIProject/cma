use super::*;
use crate::ledger_storage::Storage;
use std::io::{BufRead, Read, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Command, Stdio};

#[tokio::test]
async fn anchored_import_real_write_failure_holds_and_reopens_prior_commit() {
    const CHILD: &str = "CMA_ANCHORED_WRITE_FAILURE_FIXTURE";
    if std::env::var_os(CHILD).is_none() {
        // The resource limit and signal disposition belong to this child only;
        // unrelated tests in the parent process keep their ordinary limits.
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ledger_storage::anchored::tests::anchored_import_real_write_failure_holds_and_reopens_prior_commit", "--nocapture"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
        return;
    }
    let path = std::env::temp_dir().join(format!("anchored-write-fault-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Storage::open_anchored(Arc::new(File::open(&path).unwrap()), 32_768)
        .await
        .unwrap();
    store
        .call(|db| {
            db.execute_batch("CREATE TABLE retained(value INTEGER); INSERT INTO retained VALUES(1)")
                .map_err(|_| storage_error())
        })
        .await
        .unwrap();
    let mut previous = unsafe { std::mem::zeroed::<libc::rlimit>() };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut previous) },
        0
    );
    let signal = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
    assert_ne!(signal, libc::SIG_ERR);
    let limited = libc::rlimit {
        rlim_cur: 0,
        rlim_max: previous.rlim_max,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limited) }, 0);
    let failed = store
        .call(|db| {
            db.execute_batch("INSERT INTO retained VALUES(2)")
                .map_err(|_| storage_error())
        })
        .await;
    let restored = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &previous) };
    let restored_signal = unsafe { libc::signal(libc::SIGXFSZ, signal) };
    assert_eq!(restored, 0);
    assert_ne!(restored_signal, libc::SIG_ERR);
    assert_eq!(failed.unwrap_err().code, "mutation_ledger_storage_hold");
    assert_eq!(
        std::fs::metadata(path.join("emergency.reserve"))
            .unwrap()
            .len(),
        0
    );
    store.close().await.unwrap();
    let db = Connection::open_with_flags(
        path.join("ledger.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT sum(value) FROM retained", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn anchored_import_preserves_sqlite_locks_across_directory_replacement() {
    const CHILD: &str = "CMA_ANCHORED_SQLITE_READER_FIXTURE";
    if let Some(path) = std::env::var_os(CHILD) {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db.execute_batch("BEGIN; SELECT value FROM retained")
            .unwrap();
        println!("fixture-reader-locked");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
        db.execute_batch("COMMIT").unwrap();
        return;
    }
    let root = std::env::temp_dir().join(format!("anchored-ledger-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("database");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Storage::open_anchored(Arc::new(File::open(&path).unwrap()), 32_768)
        .await
        .unwrap();
    store
        .call(|db| {
            db.execute_batch("CREATE TABLE retained(value INTEGER); INSERT INTO retained VALUES(1)")
                .map_err(|_| storage_error())
        })
        .await
        .unwrap();
    let moved = root.join("moved");
    std::fs::rename(&path, &moved).unwrap();
    let source = root.join("source");
    std::fs::create_dir(&source).unwrap();
    symlink(&source, &path).unwrap();
    store
        .call(|db| {
            db.execute_batch("INSERT INTO retained VALUES(2)")
                .map_err(|_| storage_error())
        })
        .await
        .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ledger_storage::anchored::tests::anchored_import_preserves_sqlite_locks_across_directory_replacement", "--nocapture"])
        .env(CHILD, moved.join("ledger.sqlite3")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    loop {
        if lines
            .next()
            .expect("reader must announce its lock")
            .unwrap()
            == "fixture-reader-locked"
        {
            break;
        }
    }
    let blocked = store
        .call(|db| {
            db.execute_batch("INSERT INTO retained VALUES(3)")
                .map_err(|_| storage_error())
        })
        .await;
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(blocked.unwrap_err().code, "mutation_ledger_storage_hold");
    assert_eq!(
        std::fs::metadata(moved.join("emergency.reserve"))
            .unwrap()
            .len(),
        0
    );
    store.close().await.unwrap();
    assert_eq!(std::fs::read_dir(source).unwrap().count(), 0);
    let db = Connection::open_with_flags(
        moved.join("ledger.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT sum(value) FROM retained", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
