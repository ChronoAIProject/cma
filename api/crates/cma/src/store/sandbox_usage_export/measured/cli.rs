use super::*;
use mongodb::{options::ClientOptions, Client};

pub(crate) async fn run_repair_command(arguments: &[String]) -> bool {
    if arguments
        .first()
        .is_none_or(|argument| argument != "--repair-measured-usage")
    {
        return false;
    }
    match command(arguments).await {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
    true
}

async fn command(arguments: &[String]) -> Result<serde_json::Value, &'static str> {
    let [_, source, minute, evidence, predecessor, mode] = arguments else {
        return Err("usage: --repair-measured-usage SOURCE_STREAM MINUTE_SECONDS EVIDENCE_DIGEST PREDECESSOR_EVENT --dry-run|--apply");
    };
    if !matches!(mode.as_str(), "--dry-run" | "--apply") {
        return Err("explicit repair mode required");
    }
    let request = super::repair::MeasuredRepair {
        source_stream: source.clone(),
        minute: minute.parse().map_err(|_| "invalid minute")?,
        evidence_digest: evidence.clone(),
        predecessor_event: predecessor.clone(),
    };
    let lookup = |key| std::env::var(key).map_err(|_| "explicit repair configuration required");
    let name = lookup("CHRONO_MONGODB_DATABASE")?;
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    let config = crate::config::UsageCaptureConfig {
        completeness_enabled: false,
        namespace: lookup("CHRONO_SANDBOX_USAGE_PRODUCER_NAMESPACE")?,
        owner_issuer: lookup("CHRONO_SANDBOX_USAGE_OWNER_ISSUER")?,
    };
    if [&config.namespace, &config.owner_issuer]
        .iter()
        .any(|s| s.is_empty() || s.len() > 256 || s.trim() != *s || s.chars().any(char::is_control))
    {
        return Err("invalid producer identity");
    }
    let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        "CHRONO_MONGODB_AUTHORITY_MODE",
    )?;
    let mut options = ClientOptions::parse(lookup("CHRONO_MONGODB_URI")?)
        .await
        .map_err(|_| "database configuration unavailable")?;
    options.max_pool_size = Some(4);
    options.server_selection_timeout = Some(std::time::Duration::from_secs(5));
    let client = Client::with_options(options).map_err(|_| "database configuration unavailable")?;
    crate::store::isolation::verify_with_authority(&client.database(&name), authority)
        .await
        .map_err(|_| "Agent Manager authority required")?;
    let store = CaptureStore::open(&client, &name, config)
        .await
        .map_err(|_| "usage schema requires explicit preparation")?;
    let complete = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        store.repair_measured(&request, mode == "--apply"),
    )
    .await
    .map_err(|_| "repair outcome uncertain; retry the same request")?
    .map_err(|_| "repair refused or incomplete; retain the exact request and inspect evidence")?;
    Ok(serde_json::json!({"complete":complete,"dry_run":mode=="--dry-run"}))
}
