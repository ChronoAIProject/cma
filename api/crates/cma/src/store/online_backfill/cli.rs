use super::{current, sources::SOURCES, start, step, Cohort, Result, COLLECTION, IO};
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client,
};
use serde_json::Value;

pub(crate) fn is_command(arguments: &[String]) -> bool {
    arguments
        .first()
        .is_some_and(|value| value == "--online-backfill-progress")
}

fn role(database: &str) -> Document {
    let mut privileges = Vec::from(SOURCES.map(|collection| {
        doc! {"resource":{"db":database,"collection":collection},"actions":["find"]}
    }));
    privileges.push(doc! {
        "resource":{"db":database,"collection":COLLECTION},
        "actions":["find","insert","update"]
    });
    doc! {"createRole":"cma_am_online_backfill_progress","privileges":privileges,"roles":[]}
}

pub(crate) async fn command(arguments: &[String]) -> Result<Value> {
    tokio::time::timeout(std::time::Duration::from_secs(25), execute(arguments))
        .await
        .map_err(|_| {
            "backfill progress deadline reached. Read the current revision before retrying"
        })?
}

async fn execute(arguments: &[String]) -> Result<Value> {
    let [_, action, rest @ ..] = arguments else {
        return Err("supply a backfill progress action");
    };
    let database_name =
        std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| "database is required")?;
    if database_name.is_empty()
        || database_name.len() > 63
        || matches!(database_name.as_str(), "admin" | "local" | "config")
        || database_name
            .chars()
            .any(|value| value.is_whitespace() || "/\\.\"$\0".contains(value))
    {
        return Err("invalid database name");
    }
    if action == "role" && rest.is_empty() {
        return serde_json::to_value(role(&database_name))
            .map_err(|_| "cannot encode backfill role");
    }
    let run = rest.first().ok_or("supply a backfill run UUID")?;
    let id = uuid::Uuid::parse_str(run)
        .map_err(|_| "invalid backfill run UUID")?
        .to_string();
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let mut options = ClientOptions::parse(uri)
        .await
        .map_err(|_| "database configuration unavailable")?;
    crate::store::transaction_barrier::bind(&mut options, &database_name);
    options.server_selection_timeout = Some(IO);
    options.connect_timeout = Some(IO);
    let database = Client::with_options(options)
        .map_err(|_| "database configuration unavailable")?
        .database(&database_name);
    let grants = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    cma_service_auth::mongodb_authority::verify_shared_schema(
        &grants,
        &database_name,
        &role(&database_name),
    )
    .map_err(|_| "backfill progress authority required")?;
    match (action.as_str(), rest) {
        ("status", [_]) => Ok(current(&database, &id).await?.public_view()),
        ("start", [_, kind]) => start(&database, &id, &cohort(kind, &database_name)?).await,
        ("step", [_, kind, revision]) => {
            let revision = revision
                .parse::<i64>()
                .ok()
                .filter(|value| *value >= 0)
                .ok_or("invalid checkpoint revision")?;
            step(&database, &id, &cohort(kind, &database_name)?, revision).await
        }
        _ => Err("unsupported backfill progress command"),
    }
}

fn cohort(kind: &str, database_name: &str) -> Result<Cohort> {
    let mut lookup = |name: &str| match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err("invalid backfill cohort environment".into()),
    };
    match kind {
        "routing" => {
            let deployment = crate::config::deployment_id_from_lookup(&mut lookup)
                .map_err(|_| "valid routing deployment identity required")?;
            let allocate_native = match std::env::var("CHRONO_SANDBOX_PROVIDER").as_deref() {
                Ok("agent-sandbox") => true,
                Ok("opensandbox") | Err(std::env::VarError::NotPresent) => false,
                _ => return Err("an active Sandbox provider is required for routing progress"),
            };
            Ok(Cohort::Routing {
                deployment,
                allocate_native,
            })
        }
        "endpoints" => {
            let migration = crate::config::parse_agent_access_migration(&mut lookup)
                .map_err(|_| "invalid access transport migration cohort")?
                .ok_or("active access transport migration cohort required")?;
            let public_origin = lookup("CHRONO_SERVER_PUBLIC_URL")
                .map_err(|_| "invalid Agent access origin")?
                .ok_or("Agent access origin is required")?;
            let origin =
                url::Url::parse(&public_origin).map_err(|_| "invalid Agent access origin")?;
            if origin.as_str().trim_end_matches('/') != migration.origin.trim_end_matches('/') {
                return Err("endpoint migration origin must match the Agent access origin");
            }
            let retained = crate::config::parse_agent_access_retained_origins(&mut lookup)
                .map_err(|_| "invalid retained Agent access origins")?;
            let encoded_key = std::env::var("CHRONO_AUTH_ENCRYPTION_KEY")
                .map_err(|_| "Agent access signing authority is required")?;
            let key = crate::store::AuthEncryptionKey::from_base64(&encoded_key)
                .map_err(|_| "invalid Agent access signing authority")?;
            let authority = super::super::agent_access_tools::AccessRecordAuthority::new(
                database_name,
                &public_origin,
                &retained,
                &key,
            )
            .map_err(|_| "invalid Agent access signing authority")?;
            Ok(Cohort::Endpoints {
                migration,
                authority,
            })
        }
        _ => Err("unsupported backfill cohort"),
    }
}
