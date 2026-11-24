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
    let custody = crate::channels::mutation::enabled_from_env()?;
    let cancellation = crate::config::channel_cancel_create_enabled()?;
    let writer_floor_required = crate::config::channel_create_fence_required()?;
    if cancellation && !writer_floor_required {
        return Err(
            "Channel create cancellation requires the retained writer floor setting".into(),
        );
    }
    let retained_origins = crate::config::cma_record_origin_aliases()?;
    let owner_key_settings = crate::worker_credentials::config::Settings::from_env()?;
    let independent_store_enabled = [
        CMAEG_MONGODB_URI,
        CMAEG_MONGODB_DATABASE,
        CMAEG_MONGODB_AUTHORITY_MODE,
        CMAEG_NYXID_CLIENT_ID,
        CMAEG_NYXID_CLIENT_SECRET,
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some());
    if (owner_key_settings.tracking || owner_key_settings.owner_keys || writer_floor_required)
        && !independent_store_enabled
    {
        return Err("worker tracking, owner keys, and Channel create cancellation require Gateway-owned Mongo storage".into());
    }
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
        let mut records = records::RecordClient::new(
            &config.management.cma_base_url,
            config.management.cma_service_token.as_str(),
        )?
        .with_retained_origins(&retained_origins)?
        .with_database(database)
        .with_channel_mutation_custody(custody);
        let protected =
            records::mongo::verify_create_fence_floor(&database_handle, service_identity.issuer())
                .await?;
        if writer_floor_required && !protected {
            return Err(
                "Channel create cancellation requires an installed Gateway writer floor".into(),
            );
        }
        if protected {
            records = records.with_channel_create_fences(service_identity.issuer(), cancellation);
        }
        let records = Arc::new(records);
        let mut creator_auth = auth::CreatorAuthenticator::new(&config.management.nyxid_base_url)?
            .with_service(service_identity.clone());
        if owner_key_settings.owner_keys {
            let tracking = records
                .worker_credentials()
                .ok_or("owner keys require durable worker tracking")?;
            let cutover_id = owner_key_settings
                .cutover_id
                .ok_or("owner key cutover ID required")?;
            let cutover = tracking
                .history()
                .cutover()
                .await?
                .ok_or("owner key cutover attestation missing")?;
            if !cutover.admits(&cutover_id, u64::MAX) {
                return Err("owner key cutover attestation does not match".into());
            }
            let verifier = cma_service_auth::ServiceKeyAuth::new(
                service_identity.clone(),
                owner_key_settings
                    .service_id
                    .as_deref()
                    .ok_or("owner key service UUID required")?,
            )?;
            let manager =
                crate::providers::WorkerClassifierClient::new(config, service_auth.clone())?;
            creator_auth =
                creator_auth.with_owner_keys(auth::owner_keys::OwnerKeyAuthenticator::new(
                    verifier,
                    tracking.history(),
                    Arc::new(manager),
                    cutover_id,
                ));
        }
        let creator_auth = Arc::new(creator_auth);
        (records, creator_auth, Some(service_identity))
    } else {
        let records = std::sync::Arc::new(
            records::RecordClient::new(
                &config.management.cma_base_url,
                config.management.cma_service_token.as_str(),
            )?
            .with_retained_origins(&retained_origins)?
            .with_service_auth(service_auth)
            .with_channel_mutation_custody(custody),
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
    let client = client
        .with_channel_mutation_custody(crate::channels::mutation::enabled_from_env().map_err(
            |_| records::RecordError::Rejected("channel custody flag is invalid".into()),
        )?)
        .with_retained_origins(&crate::config::cma_record_origin_aliases().map_err(|_| {
            records::RecordError::Rejected("retained CMA record origins are invalid".into())
        })?)?;
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
