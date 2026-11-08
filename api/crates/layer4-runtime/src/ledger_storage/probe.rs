//! A create-only, synthetic storage probe for exact-image qualification.
//! It cannot open an existing directory or initialize a runtime ledger.
use std::path::Path;

use super::{connection, storage_error, Storage};
use crate::failure::Result;

pub(crate) async fn run(directory: &Path) -> Result<()> {
    let storage = Storage::open(directory.to_owned(), true, 256).await?;
    storage
        .call(|connection| {
            let transaction = connection.transaction().map_err(|_| storage_error())?;
            transaction
                .execute_batch(
                    "CREATE TABLE probe (singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                     fence BLOB NOT NULL CHECK(typeof(fence) = 'blob' AND length(fence) = 8)) STRICT;
                     INSERT INTO probe VALUES (1, x'FFFFFFFFFFFFFFFF');",
                )
                .map_err(|_| storage_error())?;
            transaction.commit().map_err(|_| storage_error())
        })
        .await?;
    storage.close().await?;
    crate::mutation_ledger::probe(&directory.with_extension("indexed")).await?;
    let storage = Storage::open(directory.to_owned(), false, 256).await?;
    let version = storage
        .call(|connection| {
            connection::verify(connection, 256)?;
            let fence = connection
                .query_row("SELECT fence FROM probe WHERE singleton = 1", [], |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .map_err(|_| storage_error())?;
            if fence != u64::MAX.to_be_bytes() {
                return Err(super::invalid());
            }
            Ok(rusqlite::version().to_owned())
        })
        .await?;
    storage.close().await?;
    // Only library version and fixed predicates; never print paths or identities.
    println!("sqlite={version} journal=delete synchronous=extra create_commit_reopen=passed");
    Ok(())
}
