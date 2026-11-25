use std::{sync::Arc, time::Duration};

use cmaeg_core::seal::MasterKeyRing;
use mongodb::{bson::doc, Database};

use super::{schema, Gateway};

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
            crate::migrations::legacy_startup(&database, "--migrate-automation")
                .await
                .map_err(|_| "startup_migration_failed")?;
        }
        Gateway::open(database, keys)
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

/// Bounded optional storage initialization, independent of ordinary HTTP readiness.
pub(crate) struct Initialization(tokio::task::JoinHandle<()>);
impl Initialization {
    pub(crate) fn start(
        state: crate::http::ManagementState,
        ingress: crate::ingress::IngressState,
    ) -> Self {
        Self(tokio::spawn(async move {
            let Some(records_database) = state.records.database() else {
                return;
            };
            match crate::records::mongo::guard_migration_required(&records_database).await {
                Ok(true) => {
                    if crate::migrations::legacy_startup(&records_database, "--migrate")
                        .await
                        .is_err()
                    {
                        tracing::warn!("Gateway RECORDS guard migration did not complete; GitHub remains unavailable");
                        return;
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    tracing::warn!("Gateway RECORDS guard migration did not complete; GitHub remains unavailable");
                    return;
                }
            }
            let Some(mut gateway) =
                Gateway::from_records(&state.records, state.master_keys.clone()).await
            else {
                return;
            };
            gateway.attach_ingress(Arc::new(ingress));
            gateway.attach_activity(state.activity.clone());
            let gateway = Arc::new(gateway);
            let _ = state.github.set(gateway.clone());
            loop {
                gateway.recover_tick().await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }))
    }
    pub(crate) fn stop(&self) {
        self.0.abort();
    }
    pub(crate) async fn drain(&mut self, deadline: std::time::Instant) -> bool {
        self.stop();
        match tokio::time::timeout_at(deadline.into(), &mut self.0).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => error.is_cancelled(),
            Err(_) => false,
        }
    }
}
impl Drop for Initialization {
    fn drop(&mut self) {
        self.stop();
    }
}
