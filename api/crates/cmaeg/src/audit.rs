//! Operator audit entry points, which read channel records without serving.

use crate::{channels, serve::record_client, service_auth, Config};

/// Audit every stored channel definition for a custom `attachments` setting
/// and, with `clear`, remove it, so an image older than the setting can read
/// every record. It binds no listener and starts no worker. Every error is a
/// classified [`channels::audit::AuditError`], so printing it never reveals a
/// URL, token or upstream body.
pub async fn audit_custom_attachments(
    clear: bool,
) -> Result<channels::audit::AuditReport, Box<dyn std::error::Error + Send + Sync>> {
    use channels::audit::AuditError;
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
    let records = record_client(&config, service_auth).map_err(AuditError::from)?;
    Ok(
        channels::audit::audit_custom_attachments(&records, &config.management.master_keys, clear)
            .await?,
    )
}
