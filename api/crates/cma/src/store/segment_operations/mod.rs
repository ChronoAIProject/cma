//! Explicit operator entry points. Never invoked by service startup or a GET.
use std::time::Duration;

use mongodb::{bson::doc, options::ClientOptions, Client};
use serde_json::{json, Value};

use super::agent_log_segments::{audit, command as gate, schema, writer_control};

mod authority;
mod parse;
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, &'static str>;
use parse::{AuditAction, Operation};

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments.first().is_some_and(|value| {
        value.starts_with("--agent-log-") || value == "--print-agent-log-operator-role"
    })
}

pub(crate) async fn command(arguments: &[String]) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(45), execute(arguments))
        .await
        .map_err(|_| "operator deadline; inspect the same audit ID or gate revision before retry")?
}

async fn execute(arguments: &[String]) -> Result<Value> {
    let operation = parse::operation(arguments)?;
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    validate_database(&name)?;
    let access = operation.access();
    if matches!(operation, Operation::Role(_)) {
        return serde_json::to_value(authority::role(&name, access)).map_err(|_| "role encoding");
    }
    // A linked writer control is not permission to invent its missing DDL.
    authority::require_inventory(access)?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        "CHRONO_MONGODB_AUTHORITY_MODE",
    )?;
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "database configuration")?;
    crate::store::transaction_barrier::bind(&mut options, &name);
    options.server_selection_timeout = Some(Duration::from_secs(5));
    options.connect_timeout = Some(Duration::from_secs(5));
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    let database = client.database(&name);
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    authority::verify(&status, &name, mode, access)?;
    super::sandbox_mongo::require_transaction_topology(&database)
        .await
        .map_err(|_| "operator requires a supported replica set or mongos")?;
    // Unlike startup verify(), an operator action requires a complete schema.
    // Inspection must succeed before any audit/control transaction can write.
    if schema::pending_count(&database)
        .await
        .map_err(|_| "operator schema is unavailable or incompatible")?
        != 0
    {
        return Err("operator requires the completed additive schema migration");
    }
    match operation {
        Operation::Gate(command) => {
            let status = writer_control::execute(&client, &database, command.name, command.action)
                .await
                .map_err(|_| "gate refused or commit uncertain; inspect the original revision")?;
            serde_json::to_value(status).map_err(|_| "gate status encoding")
        }
        Operation::ActiveProof(agent) => {
            if !crate::store::migrations::agent_log_active_proofs::inspect(&database)
                .await?
                .is_empty()
            {
                return Err("active log proof preparation requires its completed schema migration");
            }
            let revision =
                super::agent_log_segments::active_proof::prepare(&client, &database, &agent)
                    .await?;
            Ok(json!({"operation":"active_proof_prepare","head_revision":revision}))
        }
        Operation::Audit { action, agent, id } => {
            let result = match action {
                AuditAction::RunUnit => {
                    audit::run_unit(&client, &database, &agent, &id, false).await
                }
                AuditAction::Cancel => audit::run_unit(&client, &database, &agent, &id, true).await,
                AuditAction::Install => audit::install(&client, &database, &agent, &id).await,
            };
            let status = result
                .map_err(|_| "audit refused or commit uncertain; resume the original audit ID")?;
            let operation = match action {
                AuditAction::RunUnit => "audit_unit",
                AuditAction::Cancel => "audit_cancel",
                AuditAction::Install => "audit_install",
            };
            Ok(json!({"operation":operation,"status":status}))
        }
        Operation::Role(_) => unreachable!("role print returned before database access"),
    }
}

fn validate_database(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    Ok(())
}
