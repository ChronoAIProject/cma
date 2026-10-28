use crate::{records, service_auth, Config};

/// The CMA channel-record client, authenticated as the gateway service.
pub(crate) async fn record_client(
    config: &Config,
    service_auth: service_auth::ServiceAuth,
) -> Result<records::RecordClient, records::RecordError> {
    let client = records::RecordClient::new(
        &config.management.cma_base_url,
        config.management.cma_service_token.as_str(),
    )?
    .with_service_auth(service_auth);
    crate::serve_storage::record_client(client).await
}
