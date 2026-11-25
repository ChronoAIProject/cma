//! Explicit operator-only commands; output contains only aggregate state.
use super::{quota::*, quota_baseline::*};
use mongodb::{bson::doc, options::ClientOptions, Client};
use std::io::Read;

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments
        .first()
        .is_some_and(|v| v.starts_with("--document-quota-"))
}

pub(crate) async fn command(arguments: &[String]) -> Result<serde_json::Value, &'static str> {
    tokio::time::timeout(std::time::Duration::from_secs(60), execute(arguments)).await
        .map_err(|_| "quota command deadline; retain the original manifest and resume after inspecting status")?
}

async fn execute(arguments: &[String]) -> Result<serde_json::Value, &'static str> {
    enum Action {
        Status,
        Prepare(String),
        Bind(String, String, String),
        Install(Inventory, bool),
    }
    let action = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--document-quota-status"] => Action::Status,
        ["--document-quota-bind-storage", activation, inventory, namespace, "--writers-drained"] => Action::Bind((*activation).into(), (*inventory).into(), (*namespace).into()),
        ["--document-quota-prepare", id] if valid_activation(id) => Action::Prepare((*id).into()),
        ["--document-quota-install", path, mode @ ("--dry-run" | "--apply")] => {
            let file = std::fs::File::open(path).map_err(|_| "inventory file unavailable")?;
            let mut bytes = Vec::new();
            file.take(MAX_MANIFEST_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|_| "inventory file unreadable")?;
            let inventory = Inventory::parse(&bytes).map_err(|_| "invalid or incomplete bounded inventory")?;
            Action::Install(inventory, *mode == "--apply")
        },
        _ => return Err("unsupported document quota command; installation requires explicit --dry-run or --apply"),
    };
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
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
    let database = Client::with_options(options)
        .map_err(|_| "database configuration")?
        .database(&name);
    crate::store::isolation::verify_with_authority(&database, mode)
        .await
        .map_err(|_| "Agent Manager authority required")?;
    if !database
        .list_collection_names()
        .await
        .map_err(|_| "database collection inventory unavailable")?
        .iter()
        .any(|n| n == QUOTA_COLLECTION)
    {
        return Err("deploy compatible readers and initialize the quota collection first");
    }
    let store = MongoDocumentQuotaStore::new(&database, true);
    let dry_run = matches!(&action, Action::Install(_, false));
    match action {
        Action::Status => {},
        Action::Bind(activation, inventory, namespace) => store.bind_storage(&activation, &inventory, &namespace).await.map_err(|_| "storage binding refused; verify the original inventory and storage namespace")?,
        Action::Prepare(id) => store.prepare(&id).await.map_err(|_| "source-write fence refused; inspect retained activation state")?,
        Action::Install(inventory, apply) => store.install(&database, &inventory, apply).await.map_err(|_| "baseline import refused; keep source writes fenced and retain the original inventory")?,
    }
    let state = store
        .rows
        .find_one(doc! {"_id": STATE})
        .max_time(IO)
        .await
        .map_err(|_| "quota status unavailable")?;
    match state {
        None => Ok(serde_json::json!({"phase": "unprepared", "dry_run": dry_run})),
        Some(state) => {
            validate_row(&state, "activation").map_err(|_| "unsupported quota status")?;
            let phase = state
                .get_str("phase")
                .map_err(|_| "unsupported quota phase")?;
            if !matches!(phase, "preparing" | "ready") {
                return Err("unsupported quota phase");
            }
            Ok(serde_json::json!({"phase": phase, "dry_run": dry_run,
                "installed_rows": state.get_i64("next_row").unwrap_or(0),
                "baseline_rows": state.get_i64("baseline_rows").unwrap_or(0)}))
        }
    }
}
