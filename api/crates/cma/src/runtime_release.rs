//! Explicit release authority is independent of the AM replica feature flag.
mod bootstrap;
mod cli;
mod files;
mod http;
mod migration;
mod pools;
mod readonly;
mod selection;
mod session;
pub(crate) mod settings;
mod standby;
mod verifier;

pub(crate) use http::guard as guard_http;
pub(crate) use migration::verify_floor as verify_pool_floor;
pub(crate) use selection::target_matches;
pub(crate) use session::Session;

use mongodb::Client;
use std::sync::Arc;

use crate::{config::Config, release_authority::AuthorityStore};

pub(crate) async fn before_composition(config: &Config) -> Result<Option<Arc<Session>>, String> {
    match &config.runtime_release {
        // The existing pool initializer refuses the migrated schema3/4 floor.
        // Bootstrap also requires actual legacy/restart-source closure; a flag
        // or a new empty permit collection cannot substitute for that proof.
        settings::Settings::Disabled => {
            migration::verify_uninstalled(config).await?;
            Ok(None)
        }
        settings::Settings::Managed { .. } => {
            if config.agent_manager_replicas.is_some()
                || !config.runtime.allocate_agent_sandbox
                || config.runtime.retain_opensandbox
            {
                return Err(
                    "managed release requires the qualified native replicas-OFF topology".into(),
                );
            }
            let authenticated = files::authenticate(config).await?;
            let client = connect(config).await?;
            let database = client.database(&config.mongodb_database);
            let store = Arc::new(
                AuthorityStore::open(client, database)
                    .await
                    .map_err(|error| error.to_string())?,
            );
            standby::wait_for_writer(config, store, authenticated)
                .await
                .map(Some)
        }
    }
}

pub(super) async fn connect(config: &Config) -> Result<Client, String> {
    let mut options = mongodb::options::ClientOptions::parse(&config.mongodb_uri)
        .await
        .map_err(|_| "runtime release database configuration is unavailable")?;
    options.selection_criteria = Some(mongodb::options::SelectionCriteria::ReadPreference(
        mongodb::options::ReadPreference::Primary,
    ));
    options.read_concern = Some(mongodb::options::ReadConcern::majority());
    options.server_selection_timeout = Some(std::time::Duration::from_secs(10));
    Client::with_options(options)
        .map_err(|_| "runtime release database connection is unavailable".into())
}

pub(crate) fn is_command(arguments: &[String]) -> bool {
    cli::is_command(arguments)
}
pub(crate) async fn command(arguments: &[String]) -> Result<String, String> {
    cli::command(arguments).await
}
