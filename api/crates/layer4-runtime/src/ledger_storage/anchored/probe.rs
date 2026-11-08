//! Exact-image qualification includes the descriptor-rooted import writer and
//! reopening its bytes using the ordinary serving SQLite VFS.
use super::*;
use crate::ledger_storage::Storage;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

pub(crate) async fn run(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|_| storage_error())?;
    let directory = Arc::new(File::open(path).map_err(|_| storage_error())?);
    let storage = Storage::open_anchored(directory, 256).await?;
    let storage = storage.validate(|db| {
        connection::verify(db, 256)?;
        let tx = db.transaction().map_err(|_| storage_error())?;
        tx.execute_batch("CREATE TABLE anchored_probe (fence BLOB CHECK(length(fence)=8)); INSERT INTO anchored_probe VALUES(x'FFFFFFFFFFFFFFFF')")
            .map_err(|_| storage_error())?;
        tx.commit().map_err(|_| storage_error())
    }).await?;
    storage.close().await?;
    let storage = Storage::open(path.to_owned(), false, 256).await?;
    let storage = storage
        .validate(|db| {
            connection::verify(db, 256)?;
            let bytes: Vec<u8> = db
                .query_row("SELECT fence FROM anchored_probe", [], |r| r.get(0))
                .map_err(|_| storage_error())?;
            if bytes != u64::MAX.to_be_bytes() {
                return Err(invalid());
            }
            Ok(())
        })
        .await?;
    storage.close().await
}
