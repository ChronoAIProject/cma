use std::{sync::Arc, time::Duration};

use cmaeg_core::seal::MasterKeyRing;
use mongodb::{bson::doc, Database};

use super::{schema, Config, Gateway};

pub(super) async fn initialize(database: Database, keys: Arc<MasterKeyRing>) -> Option<Gateway> {
    let initialization = async {
        if schema::migration_required(&database)
            .await
            .map_err(|_| "schema_marker_unavailable_or_unsupported")?
        {
            let status = database
                .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
                .await
                .map_err(|_| "database_authority_unavailable")?;
            cma_service_auth::mongodb_authority::verify_shared_schema(
                &status,
                database.name(),
                &schema::migration_role(database.name()),
            )
            .map_err(|_| "startup_migration_permission_required")?;
            schema::migrate(&database)
                .await
                .map_err(|_| "startup_migration_failed")?;
        }
        Gateway::open(database, keys, Config::default())
            .await
            .map_err(|_| "storage_verification_failed")
    };
    match tokio::time::timeout(Duration::from_secs(30), initialization).await {
        Ok(Ok(gateway)) => {
            tracing::info!("GitHub integration storage is ready");
            Some(gateway)
        }
        result => {
            let reason = match result {
                Ok(Err(reason)) => reason,
                _ => "startup_initialization_timeout",
            };
            tracing::warn!(reason, "GitHub initialization did not complete; integration disabled, retry on next startup");
            None
        }
    }
}
