//! Explicit Agent Manager migration commands; never composed into service startup.
use std::time::Duration;

use mongodb::{bson::doc, options::ClientOptions, Client};
use serde_json::{json, Value};

mod activity_indexes;
mod authority;
mod ledger;
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, &'static str>;
const DEADLINE: Duration = Duration::from_secs(45);
const IO: Duration = Duration::from_secs(5);

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments.first().is_some_and(|argument| {
        argument.starts_with("--migration-") || argument == "--print-migration-role"
    })
}

pub(crate) async fn command(arguments: &[String]) -> Result<Value> {
    tokio::time::timeout(DEADLINE, execute(arguments))
        .await
        .map_err(|_| "deadline exceeded; inspect the durable attempt before retry")?
}

async fn execute(arguments: &[String]) -> Result<Value> {
    let apply = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--migration-plan" | "--migration-status" | "--print-migration-role"] => false,
        ["--migration-apply", _, _] => true,
        _ => return Err("unsupported migration command or arguments"),
    };
    let database = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    if database.is_empty()
        || database.len() > 63
        || database
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    if arguments[0] == "--print-migration-role" {
        return serde_json::to_value(authority::role(&database, true)).map_err(|_| "role encoding");
    }
    let identity = if apply {
        if arguments[1] != activity_indexes::checksum() {
            return Err("manifest checksum mismatch");
        }
        Some(ledger::Identity::new(
            option_env!("CHRONO_BUILD_SHA").unwrap_or("development"),
            &arguments[2],
        )?)
    } else {
        None
    };
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        "CHRONO_MONGODB_AUTHORITY_MODE",
    )?;
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "database configuration")?;
    options.server_selection_timeout = Some(IO);
    options.connect_timeout = Some(IO);
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    let database = client.database(&database);
    let authority = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    authority::verify(&authority, database.name(), mode, apply)?;
    let ledger = ledger::Ledger::new(client, database.clone());
    ledger.preflight().await?;
    if arguments[0] == "--migration-status" {
        return ledger.status().await;
    }
    let indexes = activity_indexes::inspect(&database).await?;
    if let Some(identity) = identity {
        apply_indexes(&ledger, &database, &identity).await
    } else {
        Ok(
            json!({"migration":activity_indexes::ID,"checksum":activity_indexes::checksum(),
            "prerequisites":[],"phase":"expand","missing_indexes":indexes.len(),
            "effects":"add nonunique query indexes only","writes":false}),
        )
    }
}

async fn apply_indexes(
    ledger: &ledger::Ledger,
    database: &mongodb::Database,
    identity: &ledger::Identity,
) -> Result<Value> {
    super::sandbox_mongo::require_transaction_topology(database)
        .await
        .map_err(|_| "migration requires a supported replica set or mongos")?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if !activity_indexes::inspect(database).await?.is_empty() {
            return Err("completed migration postcondition drift");
        }
        return Ok(json!({"migration":activity_indexes::ID,"outcome":"already_completed"}));
    }
    let lease = ledger.claim(identity).await?;
    let outcome = async {
        let pending = activity_indexes::inspect(database).await?;
        let mut changed = 0_i64;
        for index in pending {
            ledger.renew(&lease).await?;
            // Only identical additive DDL is allowed: submitted commands can
            // outlive a lost lease, so lease ownership cannot fence a TTL/drop.
            let effect = activity_indexes::create(database, index);
            tokio::pin!(effect);
            let mut renewal = tokio::time::interval(Duration::from_secs(5));
            let created = loop {
                tokio::select! {
                    result = &mut effect => { break result?; }
                    _ = renewal.tick() => ledger.renew(&lease).await?,
                }
            };
            changed += i64::from(created);
        }
        if !activity_indexes::inspect(database).await?.is_empty() {
            return Err("migration postcondition unavailable");
        }
        Ok(changed)
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    let changed = outcome?;
    Ok(json!({"migration":activity_indexes::ID,"outcome":"completed","created_indexes":changed}))
}
