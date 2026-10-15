//! Gateway-owned Mongo records and the portal browser session composition.
use crate::{
    auth, records, service_auth, Config, CMAEG_MONGODB_AUTHORITY_MODE, CMAEG_MONGODB_DATABASE,
    CMAEG_MONGODB_URI, CMAEG_NYXID_CLIENT_ID, CMAEG_NYXID_CLIENT_SECRET,
};
use std::sync::Arc;
type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) async fn compose(
    config: &Config,
    service_auth: service_auth::ServiceAuth,
) -> Result<
    (
        Arc<records::RecordClient>,
        Arc<auth::CreatorAuthenticator>,
        Option<cma_service_auth::NyxIdAuth>,
    ),
    Error,
> {
    let independent_store_enabled = [
        CMAEG_MONGODB_URI,
        CMAEG_MONGODB_DATABASE,
        CMAEG_MONGODB_AUTHORITY_MODE,
        CMAEG_NYXID_CLIENT_ID,
        CMAEG_NYXID_CLIENT_SECRET,
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some());
    let (records, creator_auth, browser) = if independent_store_enabled {
        let mongo_uri = std::env::var(CMAEG_MONGODB_URI).map_err(|_| {
            format!("{CMAEG_MONGODB_URI} is required when independent Gateway storage is enabled")
        })?;
        let database_name = std::env::var(CMAEG_MONGODB_DATABASE).map_err(|_| {
            format!(
                "{CMAEG_MONGODB_DATABASE} is required when independent Gateway storage is enabled"
            )
        })?;
        let client_id = std::env::var(CMAEG_NYXID_CLIENT_ID).map_err(|_| {
            format!(
                "{CMAEG_NYXID_CLIENT_ID} is required when independent Gateway storage is enabled"
            )
        })?;
        let client_secret = std::env::var(CMAEG_NYXID_CLIENT_SECRET)
            .map_err(|_| format!("{CMAEG_NYXID_CLIENT_SECRET} is required when independent Gateway storage is enabled"))?;
        let mongo = mongodb::Client::with_uri_str(mongo_uri).await?;
        let database_handle = mongo.database(&database_name);
        let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
            CMAEG_MONGODB_AUTHORITY_MODE,
        )?;
        let database =
            records::mongo::MongoRecords::open_with_authority(database_handle.clone(), authority)
                .await?;
        let service_identity = cma_service_auth::NyxIdAuth::new(
            &config.management.nyxid_base_url,
            cma_service_auth::Service::EventGateway,
            &client_id,
            client_secret,
        )?;
        let records = std::sync::Arc::new(
            records::RecordClient::new(
                &config.management.cma_base_url,
                config.management.cma_service_token.as_str(),
            )?
            .with_database(database),
        );
        let creator_auth = std::sync::Arc::new(
            auth::CreatorAuthenticator::new(&config.management.nyxid_base_url)?
                .with_service(service_identity.clone()),
        );
        (records, creator_auth, Some(service_identity))
    } else {
        let records = std::sync::Arc::new(
            records::RecordClient::new(
                &config.management.cma_base_url,
                config.management.cma_service_token.as_str(),
            )?
            .with_service_auth(service_auth),
        );
        let creator_auth = std::sync::Arc::new(auth::CreatorAuthenticator::new(
            &config.management.nyxid_base_url,
        )?);
        (records, creator_auth, None)
    };
    Ok((records, creator_auth, browser))
}

/// Operator audits must use the same Gateway database authority as the server.
pub(crate) async fn record_client(
    client: records::RecordClient,
) -> Result<records::RecordClient, records::RecordError> {
    if ![
        CMAEG_MONGODB_URI,
        CMAEG_MONGODB_DATABASE,
        CMAEG_MONGODB_AUTHORITY_MODE,
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some())
    {
        return Ok(client);
    }
    let uri = std::env::var(CMAEG_MONGODB_URI)
        .map_err(|_| records::RecordError::Rejected("Gateway database URI is required".into()))?;
    let name = std::env::var(CMAEG_MONGODB_DATABASE)
        .map_err(|_| records::RecordError::Rejected("Gateway database name is required".into()))?;
    let mongo = mongodb::Client::with_uri_str(uri)
        .await
        .map_err(|_| records::RecordError::Unavailable("Gateway database unavailable".into()))?;
    let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        CMAEG_MONGODB_AUTHORITY_MODE,
    )
    .map_err(|_| records::RecordError::Rejected("Gateway database authority is invalid".into()))?;
    Ok(client.with_database(
        records::mongo::MongoRecords::open_with_authority(mongo.database(&name), authority).await?,
    ))
}
