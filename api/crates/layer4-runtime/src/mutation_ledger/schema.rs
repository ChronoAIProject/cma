use rusqlite::{params, Connection};

use super::{fence, invalid, storage_error};
use crate::config::RuntimeConfig;
use crate::failure::Result;

const APPLICATION_ID: i64 = 0x434d4132;
const VERSION: i64 = 2;

pub(super) fn open(db: &mut Connection, config: &RuntimeConfig, create: bool) -> Result<()> {
    open_guarded(db, config, create, None)
}

pub(super) fn open_guarded(
    db: &mut Connection,
    config: &RuntimeConfig,
    create: bool,
    guard: Option<&crate::ledger_format::Guard>,
) -> Result<()> {
    if create {
        initialize(db, config, "fresh")?;
    }
    let read = |name| {
        db.pragma_query_value(None, name, |row| row.get::<_, i64>(0))
            .map_err(|_| storage_error())
    };
    if read("application_id")? != APPLICATION_ID || read("user_version")? != VERSION {
        return Err(invalid());
    }
    // Compare the complete private schema, including constraints and indexes.
    let mut expected = include_str!("schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
        .map(|sql| {
            (
                sql.split_whitespace().nth(2).unwrap().to_owned(),
                sql.to_owned(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    if definitions(db)? != expected {
        return Err(invalid());
    }
    let (sandbox, home, generation): (String, String, Vec<u8>) = db
        .query_row(
            "SELECT sandbox, home, generation FROM metadata WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| invalid())?;
    if sandbox != config.sandbox_id
        || home != config.home_identity.clone().unwrap_or_default()
        || fence::decode(&generation)? > config.generation
    {
        return Err(invalid());
    }
    let (state, home, transfer, admission): (String, String, String, String) = db
        .query_row(
            "SELECT state_root,home_root,transfer_root,admission FROM metadata WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| invalid())?;
    if std::path::Path::new(&state) != config.state_root
        || std::path::Path::new(&home) != config.home
        || std::path::Path::new(&transfer) != config.checkpoint_root
    {
        return Err(invalid());
    }
    if admission != "fresh" {
        return Err(crate::failure::RuntimeError::rejected(
            "mutation_ledger_import_held",
            "imported runtime mutation ledger requires reviewed release authority",
        ));
    }
    if let Some(guard) = guard {
        let store: Vec<u8> = db
            .query_row(
                "SELECT store_id FROM metadata WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| invalid())?;
        guard.opening(&store, fence::decode(&generation)?)?;
    }
    db.execute(
        "UPDATE metadata SET generation=? WHERE singleton=1",
        [fence::encode(config.generation).as_slice()],
    )
    .map_err(|_| storage_error())?;
    Ok(())
}

fn definitions(db: &Connection) -> Result<Vec<(String, String)>> {
    let mut statement = db.prepare("SELECT name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 17")
        .map_err(|_| invalid())?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| invalid())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| invalid())?;
    if rows.len() > 16
        || rows
            .iter()
            .any(|(name, sql): &(String, String)| name.len() > 256 || sql.len() > 65536)
    {
        return Err(invalid());
    }
    Ok(rows)
}

fn initialize(db: &mut Connection, config: &RuntimeConfig, admission: &str) -> Result<()> {
    let transaction = db.transaction().map_err(|_| storage_error())?;
    transaction
        .execute_batch(include_str!("schema.sql"))
        .map_err(|_| storage_error())?;
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(|_| storage_error())?;
    transaction
        .pragma_update(None, "user_version", VERSION)
        .map_err(|_| storage_error())?;
    transaction
        .execute(
            "INSERT INTO metadata VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                config.sandbox_id,
                config.home_identity.clone().unwrap_or_default(),
                fence::encode(config.generation).as_slice(),
                config.state_root.to_str().ok_or_else(invalid)?,
                config.home.to_str().ok_or_else(invalid)?,
                config.checkpoint_root.to_str().ok_or_else(invalid)?,
                uuid::Uuid::new_v4().as_bytes().as_slice(),
                admission
            ],
        )
        .map_err(|_| storage_error())?;
    transaction
        .execute("INSERT INTO accounting VALUES (1,0,0,0,0,0)", [])
        .map_err(|_| storage_error())?;
    transaction.commit().map_err(|_| storage_error())?;
    Ok(())
}

pub(super) fn initialize_held(db: &mut Connection, config: &RuntimeConfig) -> Result<()> {
    initialize(db, config, "import_held")
}
