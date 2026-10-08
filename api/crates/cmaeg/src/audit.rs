//! Operator audit entry points, which read channel records without serving.

use crate::{channels, providers, serve::record_client, service_auth, Config};
use channels::{
    audit::{AuditError, AuditReport},
    audit_routing::{RoutingAuditMode, RoutingFinding},
};

/// Audit every stored channel definition for a custom `attachments` setting
/// and, with `clear`, remove it, so an image older than the setting can read
/// every record. It binds no listener and starts no worker. Every error is a
/// classified [`channels::audit::AuditError`], so printing it never reveals a
/// URL, token or upstream body.
pub async fn audit_custom_attachments(
    clear: bool,
) -> Result<channels::audit::AuditReport, Box<dyn std::error::Error + Send + Sync>> {
    let (config, service_auth) = audit_config().await?;
    let records = record_client(&config, service_auth).map_err(AuditError::from)?;
    Ok(
        channels::audit::audit_custom_attachments(&records, &config.management.master_keys, clear)
            .await?,
    )
}

/// Audit every stored channel definition for the multi-consumer extensions
/// an older image cannot read and, in clear mode, return what it safely can
/// to the legacy shape, so the gateway can be rolled back below them. It
/// reads the same environment as the gateway, binds no listener and starts
/// no worker, and every error is a classified [`AuditError`].
pub async fn audit_routing(
    mode: RoutingAuditMode,
) -> Result<AuditReport<RoutingFinding>, Box<dyn std::error::Error + Send + Sync>> {
    let (config, service_auth) = audit_config().await?;
    let records = record_client(&config, service_auth.clone()).map_err(AuditError::from)?;
    // Only a clear that removes a binding needs the provider client, so a
    // report still runs without one; that clear then stops as
    // "provider configuration unavailable".
    let providers = match providers::ProviderClient::from_config(&config) {
        Ok(client) => Some(client.with_service_auth(service_auth)),
        Err(_) => {
            tracing::warn!("provider client unavailable; the audit cannot remove bindings");
            None
        }
    };
    Ok(channels::audit_routing::audit_routing(
        &records,
        &config.management.master_keys,
        providers.as_ref(),
        &config.routing.non_human_providers,
        mode,
    )
    .await?)
}

/// The gateway's configuration and a proven service credential.
async fn audit_config() -> Result<(Config, service_auth::ServiceAuth), AuditError> {
    // Configuration details can name origins; the gateway's own startup log
    // reports them, so the audit only classifies the failure.
    let config = Config::from_env().map_err(|_| AuditError::Configuration)?;
    let service_auth =
        service_auth::ServiceAuth::from_config(&config).map_err(|_| AuditError::ServiceAuth)?;
    // Prove the credential before reading any record, as the gateway does.
    service_auth
        .bearer()
        .await
        .map_err(|_| AuditError::ServiceAuth)?;
    Ok((config, service_auth))
}
