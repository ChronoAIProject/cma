use mongodb::{bson::doc, options::ClientOptions, Client};

use super::*;

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments.first().is_some_and(|value| {
        value.starts_with("--provenance-backfill-") || value == "--print-provenance-progress-role"
    })
}

pub(crate) fn role(database: &str) -> Document {
    doc! {"createRole":"cma_am_provenance_progress","privileges":[
        {"resource":{"db":database,"collection":COLLECTION},"actions":ACTIONS},
        {"resource":{"db":database,"collection":""},"actions":["listCollections"]}
    ],"roles":[]}
}

pub(crate) async fn command(
    arguments: &[String],
) -> std::result::Result<serde_json::Value, &'static str> {
    tokio::time::timeout(std::time::Duration::from_secs(30), execute(arguments))
        .await
        .map_err(|_| "provenance progress deadline; inspect status before retry")?
}

async fn execute(arguments: &[String]) -> std::result::Result<serde_json::Value, &'static str> {
    let [argument] = arguments else {
        return Err("unsupported provenance progress command");
    };
    if ![
        "--provenance-backfill-init",
        "--provenance-backfill-status",
        "--print-provenance-progress-role",
    ]
    .contains(&argument.as_str())
    {
        return Err("unsupported provenance progress command");
    }
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$\0".contains(c))
    {
        return Err("invalid database name");
    }
    if argument == "--print-provenance-progress-role" {
        return serde_json::to_value(role(&name)).map_err(|_| "role encoding");
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
    let client = Client::with_options(options).map_err(|_| "database configuration")?;
    let database = client.database(&name);
    // This operator command uses the runtime credential plus its optional,
    // narrowly scoped progress role. It never creates roles or users.
    crate::store::isolation::verify_with_authority(&database, mode)
        .await
        .map_err(|_| "Agent Manager authority required")?;
    let grants = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    cma_service_auth::mongodb_authority::verify_shared_schema(&grants, &name, &role(&name))
        .map_err(|_| "provenance progress authority required")?;
    if argument == "--provenance-backfill-init" {
        initialize(&database).await.map_err(|_| {
            "provenance activation refused; inspect retained progress and collection structure"
        })?;
    }
    status(&database)
        .await
        .map_err(|_| "provenance progress is unavailable or unsupported")
}
