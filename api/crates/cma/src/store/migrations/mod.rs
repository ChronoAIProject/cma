//! Explicit Agent Manager migration commands; never composed into service startup.
use std::time::Duration;

use mongodb::{bson::doc, options::ClientOptions, Client};
use serde_json::{json, Value};

pub(in crate::store) mod activity_audit_index;
mod activity_indexes;
mod activity_page_index;
pub(in crate::store) mod agent_log_active_proofs;
mod agent_log_segments;
pub(in crate::store) mod agent_log_source_keys;
mod agent_owner_index;
mod billing_authority;
mod binding_indexes;
mod collection_indexes;
pub(in crate::store) mod command_cohort_index;
pub(in crate::store) mod home_backup_catalog;
mod index_inventory;
mod index_plan;
mod index_toolkit;
pub(in crate::store) mod measured_batches;
pub(in crate::store) mod measured_custody;
pub(in crate::store) mod measured_index;
pub(in crate::store) mod opening_index;
mod sandbox_usage_capture;
mod steps;
mod turn_policy_index;
use steps::Step;
mod authority;
mod ledger;
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, &'static str>;
const DEADLINE: Duration = Duration::from_secs(45);
const IO: Duration = Duration::from_secs(5);

/// Qualify the reviewed access path and identify the collection incarnation.
/// A hinted query alone can return an empty result for a nonexistent collection.
pub(in crate::store) async fn require_command_cohort_index(
    database: &mongodb::Database,
) -> Result<mongodb::bson::Binary> {
    let response = database
        .run_command(doc! {
            "listCollections": 1,
            "filter": {"name": "agent_commands"},
            "cursor": {"batchSize": 2},
            "maxTimeMS": 5000,
        })
        .await
        .map_err(|_| "Command cohort collection unavailable")?;
    crate::store::agent_log_segments::capture::preview_budget::catalog_response(&response)?;
    let specifications = index_inventory::entries(&response)?;
    let [specification] = specifications.as_slice() else {
        return Err("Command cohort collection is missing");
    };
    if specification.get_str("name").ok() != Some("agent_commands")
        || specification.get_str("type").ok() != Some("collection")
    {
        return Err("Command cohort collection is invalid");
    }
    let Some(mongodb::bson::Bson::Binary(uuid)) = specification
        .get_document("info")
        .ok()
        .and_then(|info| info.get("uuid"))
    else {
        return Err("Command cohort collection identity unavailable");
    };
    if uuid.subtype != mongodb::bson::spec::BinarySubtype::Uuid || uuid.bytes.len() != 16 {
        return Err("Command cohort collection identity invalid");
    }
    if !command_cohort_index::inspect(database).await?.is_empty() {
        return Err("Command cohort index is missing");
    }
    Ok(uuid.clone())
}

pub(in crate::store) async fn measured_batches_ready(database: &mongodb::Database) -> Result<bool> {
    Ok(measured_batches::inspect(database).await?.is_empty())
}

pub(in crate::store) async fn measured_custody_ready(database: &mongodb::Database) -> Result<bool> {
    Ok(measured_custody::inspect(database).await?.is_empty())
}

pub(crate) fn is_command(arguments: &[String]) -> bool {
    if index_toolkit::is_command(arguments) {
        return true;
    }
    arguments.first().is_some_and(|argument| {
        argument.starts_with("--migration-") || argument == "--print-migration-role"
    })
}

/// Keep CLI output and exit semantics separate from the API startup path.
pub(crate) async fn run_command_line(arguments: &[String]) {
    match command(arguments).await {
        Ok(result) => {
            println!("{result}");
            if ["verification_passed", "preflight_passed"]
                .into_iter()
                .any(|field| result.get(field).and_then(serde_json::Value::as_bool) == Some(false))
            {
                std::process::exit(1);
            }
        }
        Err(reason) => {
            eprintln!("migration refused: {reason}");
            std::process::exit(2);
        }
    }
}

pub(crate) async fn command(arguments: &[String]) -> Result<Value> {
    if let Some(result) = billing_authority::command(arguments) {
        return result;
    }
    if let Some(result) = home_backup_catalog::command(arguments) {
        return result;
    }
    if index_toolkit::is_command(arguments) {
        return index_toolkit::command(arguments).await;
    }
    tokio::time::timeout(DEADLINE, execute(arguments))
        .await
        .map_err(|_| "deadline exceeded; inspect the durable attempt before retry")?
}

async fn execute(arguments: &[String]) -> Result<Value> {
    let (step, command, proof) = invocation(arguments)?;
    let apply = proof.is_some();
    let database = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    if database.is_empty()
        || database.len() > 63
        || database
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    if command == "--print-migration-role" {
        return serde_json::to_value(authority::role_for(&database, true, step))
            .map_err(|_| "role encoding");
    }
    let identity = if let Some((checksum, image)) = proof {
        if checksum != step.checksum() {
            return Err("manifest checksum mismatch");
        }
        Some(ledger::Identity::new(
            crate::build_stamp().1.unwrap_or("development"),
            image,
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
    crate::store::transaction_barrier::bind(&mut options, &database);
    options.server_selection_timeout = Some(IO);
    options.connect_timeout = Some(IO);
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    let database = client.database(&database);
    let authority = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    authority::verify_for(&authority, database.name(), mode, apply, step)?;
    let ledger = ledger::Ledger::for_step(client, database.clone(), step);
    ledger.preflight().await?;
    if command == "--migration-status" {
        return ledger.status().await;
    }
    if step == Step::AgentLogSegments {
        return match identity {
            Some(identity) => agent_log_segments::apply(&ledger, &database, &identity).await,
            None => agent_log_segments::inspect(&database).await,
        };
    }
    if step == Step::SandboxUsageCapture {
        return match identity {
            Some(identity) => sandbox_usage_capture::apply(&ledger, &database, &identity).await,
            None => sandbox_usage_capture::inspect(&database).await,
        };
    }
    let indexes = step.inspect(&database).await?;
    if let Some(identity) = identity {
        apply_indexes(&ledger, &database, &identity).await
    } else if step == Step::BindingIndexes {
        Ok(binding_indexes::plan(indexes.len()))
    } else {
        Ok(json!({"migration":step.id(),"checksum":step.checksum(),
            "prerequisites":[],"phase":"expand","missing_indexes":indexes.len(),
            "effects":"add nonunique query indexes only","writes":false}))
    }
}

async fn apply_indexes(
    ledger: &ledger::Ledger,
    database: &mongodb::Database,
    identity: &ledger::Identity,
) -> Result<Value> {
    let step = ledger.step;
    super::sandbox_mongo::require_transaction_topology(database)
        .await
        .map_err(|_| "migration requires a supported replica set or mongos")?;
    if matches!(
        step,
        Step::BindingIndexes
            | Step::MeasuredCustody
            | Step::MeasuredBatches
            | Step::MeasuredIndex
            | Step::AgentLogActiveProofs
    ) {
        step.inspect(database).await?;
    }
    ledger.initialize().await?;
    if ledger.completed().await? {
        if !step.inspect(database).await?.is_empty() {
            return Err("completed migration postcondition drift");
        }
        return Ok(json!({"migration":step.id(),"outcome":"already_completed"}));
    }
    let lease = ledger.claim(identity).await?;
    let outcome = async {
        let pending = step.inspect(database).await?;
        let mut changed = 0_i64;
        for (collection, index) in pending {
            ledger.renew(&lease).await?;
            // Only identical additive DDL is allowed: submitted commands can
            // outlive a lost lease, so lease ownership cannot fence a TTL/drop.
            let effect = step.create(database, collection, index);
            tokio::pin!(effect);
            let mut renewal = tokio::time::interval(Duration::from_secs(5));
            let created = loop {
                tokio::select! {
                    result = &mut effect => { break result?; }
                    _ = renewal.tick() => ledger.renew(&lease).await?,
                }
            };
            #[cfg(test)]
            if created && step == Step::BindingIndexes {
                tests::startup_ddl::pause_after_binding_index().await?;
            }
            changed += i64::from(created);
        }
        if !step.inspect(database).await?.is_empty() {
            return Err("migration postcondition unavailable");
        }
        Ok(changed)
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    let changed = outcome?;
    let mut result = json!({"migration":step.id(),"outcome":"completed"});
    result[step.effect_field()] = changed.into();
    Ok(result)
}

// Keep the original one-step CLI spelling compatible; explicit IDs select new steps.
type Invocation<'a> = (Step, &'a str, Option<(&'a str, &'a str)>);

fn invocation(arguments: &[String]) -> Result<Invocation<'_>> {
    let values: Vec<_> = arguments.iter().map(String::as_str).collect();
    match values.as_slice() {
        [command @ ("--migration-plan" | "--migration-status" | "--print-migration-role")] => {
            Ok((Step::Activity, command, None))
        }
        [command @ ("--migration-plan" | "--migration-status" | "--print-migration-role"), id] => {
            Ok((Step::parse(id)?, command, None))
        }
        [command @ "--migration-apply", checksum, image] => {
            Ok((Step::Activity, command, Some((checksum, image))))
        }
        [command @ "--migration-apply", id, checksum, image] => {
            Ok((Step::parse(id)?, command, Some((checksum, image))))
        }
        _ => Err("unsupported migration command or arguments"),
    }
}

// Reuse the immutable activity manifest without changing its checksum.
pub(in crate::store) fn missing_activity_indexes(
    existing: &[mongodb::bson::Document],
) -> Result<Vec<mongodb::bson::Document>> {
    activity_indexes::missing(existing)
}

#[cfg(test)]
pub(in crate::store) use tests::startup_ddl::{
    apply_binding_indexes_for_test, apply_binding_indexes_with_pause_for_test,
    prepare_bindings_for_test,
};
