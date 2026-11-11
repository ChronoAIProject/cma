use rusqlite::{params, Connection};

use super::{invalid, storage_error, Version};
use crate::config::RuntimeConfig;
use crate::failure::Result;

const APPLICATION_ID: i64 = 0x434d4152;

pub(super) fn open(
    db: &mut Connection,
    config: &RuntimeConfig,
    archive_id: &str,
    create: bool,
    version: Version,
) -> Result<()> {
    let id = uuid::Uuid::parse_str(archive_id).map_err(|_| invalid())?;
    if id.to_string() != archive_id || config.home_identity.is_none() {
        return Err(invalid());
    }
    if create {
        let tx = db.transaction().map_err(|_| storage_error())?;
        tx.execute_batch(include_str!("schema.sql"))
            .map_err(|_| storage_error())?;
        if matches!(version, Version::Indexed | Version::Collection) {
            tx.execute_batch(include_str!("indexed.sql"))
                .map_err(|_| storage_error())?;
        }
        if matches!(version, Version::Collection | Version::FileCollection) {
            tx.execute_batch(include_str!("collection.sql"))
                .map_err(|_| storage_error())?;
        }
        if version == Version::FileCollection {
            tx.execute_batch(include_str!("file_collection.sql"))
                .map_err(|_| storage_error())?;
        }
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|_| storage_error())?;
        tx.pragma_update(None, "user_version", version as i64)
            .map_err(|_| storage_error())?;
        tx.execute(
            "INSERT INTO archive_metadata VALUES (1,?,?,?,?,?,?)",
            params![
                archive_id,
                config.sandbox_id,
                config.home_identity,
                config.state_root.to_str().ok_or_else(invalid)?,
                config.home.to_str().ok_or_else(invalid)?,
                config.checkpoint_root.to_str().ok_or_else(invalid)?,
            ],
        )
        .map_err(|_| storage_error())?;
        tx.commit().map_err(|_| storage_error())?;
    }
    for (name, expected) in [
        ("application_id", APPLICATION_ID),
        ("user_version", version as i64),
    ] {
        let actual: i64 = db
            .pragma_query_value(None, name, |row| row.get(0))
            .map_err(|_| storage_error())?;
        if actual != expected {
            return Err(invalid());
        }
    }
    let sql = match version {
        Version::FileCollection => concat!(
            include_str!("schema.sql"),
            "\n",
            include_str!("collection.sql"),
            "\n",
            include_str!("file_collection.sql")
        ),
        Version::Collection => concat!(
            include_str!("schema.sql"),
            "\n",
            include_str!("indexed.sql"),
            "\n",
            include_str!("collection.sql")
        ),
        Version::Indexed => concat!(
            include_str!("schema.sql"),
            "\n",
            include_str!("indexed.sql")
        ),
        Version::Legacy => include_str!("schema.sql"),
    };
    let mut expected = sql
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
        .map(|sql| {
            let mut words = sql.split_whitespace();
            words.find(|word| matches!(*word, "TABLE" | "INDEX"));
            let name = words.next().unwrap().split('(').next().unwrap();
            (name.to_owned(), sql.to_owned())
        })
        .collect::<Vec<_>>();
    expected.sort();
    let actual = db
        .prepare("SELECT name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT ?")
        .map_err(|_| invalid())?
        .query_map([(expected.len() + 1) as i64], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|_| invalid())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| invalid())?;
    if actual != expected {
        return Err(invalid());
    }
    let matches: bool = db.query_row(
        "SELECT archive_id=? AND sandbox=? AND home=? AND state_root=? AND home_root=? AND transfer_root=? FROM archive_metadata WHERE singleton=1",
        params![archive_id, config.sandbox_id, config.home_identity, config.state_root.to_str().ok_or_else(invalid)?, config.home.to_str().ok_or_else(invalid)?, config.checkpoint_root.to_str().ok_or_else(invalid)?],
        |row| row.get(0),
    ).map_err(|_| invalid())?;
    if !matches {
        return Err(invalid());
    }
    Ok(())
}
