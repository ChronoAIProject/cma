//! Explicit named-query index operations; never composed into API startup.
use super::{ledger, Result, IO};
use mongodb::{options::ClientOptions, Client, Database};
use serde_json::{json, Value};
use std::time::Duration;

mod abort;
mod authority;
mod binding;
mod build;
mod candidates;
mod catalog;
mod cli;
mod inspection;
#[cfg(test)]
mod tests;

const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const LEASE_RENEWAL_INTERVAL: Duration = Duration::from_secs(5);

pub(super) fn is_command(arguments: &[String]) -> bool {
    arguments
        .first()
        .is_some_and(|arg| arg.starts_with("--index-") || arg.starts_with("--activity-index-"))
}

pub(super) async fn command(arguments: &[String]) -> Result<Value> {
    use cli::Operation;
    let operation = cli::parse(arguments)?;
    let deadline = operation.deadline();
    tokio::time::timeout(deadline, async {
        match operation {
            Operation::Catalog => Ok(json!({"schema":"index-toolkit-catalog-v1", "queries":catalog::all().iter().map(catalog::Query::describe).collect::<Vec<_>>()})),
            Operation::Preflight(queries) => authority::preflight(&queries).await,
            Operation::Inspect {queries, verify, legacy} => inspection::inspect(&queries, verify, legacy).await,
            Operation::Build {query, checksum, image, timeout} => build::build(&query, &checksum, &image, timeout).await,
            Operation::Abort {query, checksum, image, fence} => abort::abort(&query, &checksum, &image, fence).await,
            Operation::Status(query) => {
                let (client,database,mode)=open_database().await?;
                authority::verify(&database,mode,&[query.collection],true).await?;
                let ledger=ledger::Ledger::for_step(client,database,query.migration()?);
                ledger.preflight().await?;
                ledger.status().await
            }
        }
    }).await.map_err(|_| "index operation deadline exceeded; reconcile database and durable attempt before retry")?
}

async fn open_database() -> Result<(
    Client,
    Database,
    cma_service_auth::mongodb_authority::MongoAuthorityMode,
)> {
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    validate_database(&name)?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        "CHRONO_MONGODB_AUTHORITY_MODE",
    )?;
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "database configuration")?;
    crate::store::transaction_barrier::bind(&mut options, &name);
    options.server_selection_timeout = Some(IO);
    options.connect_timeout = Some(IO);
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    Ok((client.clone(), client.database(&name), mode))
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
