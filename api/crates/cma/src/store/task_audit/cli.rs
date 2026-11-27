//! Explicit private artifact query; no serving initialization or production writes.
use super::*;
use crate::store::ledger_census::files;
use mongodb::{options::ClientOptions, Client};
use std::path::Path;

pub(crate) async fn run_if_requested(arguments: &[String]) -> bool {
    if arguments.first().map(String::as_str) != Some("--task-audit") {
        return false;
    }
    let [_, request, output] = arguments else {
        eprintln!("usage: cma --task-audit PRIVATE_SELECTION PRIVATE_NEW_FILE");
        std::process::exit(2);
    };
    if let Err(error) = command(Path::new(request), Path::new(output)).await {
        eprintln!("task audit refused: {error}");
        std::process::exit(2);
    }
    true
}

async fn command(request: &Path, output: &Path) -> Result<()> {
    let input = files::read(request)?;
    let selection: Selection = serde_json::from_slice(&input).map_err(|_| "invalid_selection")?;
    files::require_parent(output)?;
    let report = tokio::time::timeout(Duration::from_secs(20), async {
        let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database")?;
        let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database")?;
        let mode = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
            "CHRONO_MONGODB_AUTHORITY_MODE",
        )?;
        let mut options = ClientOptions::parse(uri).await.map_err(|_| "database")?;
        options.server_selection_timeout = Some(Duration::from_secs(3));
        options.connect_timeout = Some(Duration::from_secs(3));
        let client = Client::with_options(options).map_err(|_| "database")?;
        crate::store::isolation::verify_with_authority(&client.database(&name), mode)
            .await
            .map_err(|_| "database")?;
        observe(
            &MongoSandboxStore::audit_handles(&client, &name),
            &selection,
        )
        .await
    })
    .await
    .map_err(|_| "timeout")??;
    let bytes = serde_json::to_vec(&report).map_err(|_| "invalid_snapshot")?;
    files::publish(output, &bytes)?;
    println!(
        "task audit saved: accepted={} exceptions={} writes=false",
        report.accepted,
        report.exceptions.len()
    );
    Ok(())
}
