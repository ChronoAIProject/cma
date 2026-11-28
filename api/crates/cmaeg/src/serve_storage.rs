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
        records = channel_create_authority(
            records,
            &database_handle,
            service_identity.issuer(),
            writer_floor_required,
            cancellation,
        )
        .await?;
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
            let verifier = cma_service_auth::ServiceKeyAuth::new(
                service_identity.clone(),
                owner_key_settings
                    .service_id
                    .as_deref()
                    .ok_or("owner key service UUID required")?,
            )?;
            let manager =
                crate::providers::WorkerClassifierClient::new(config, service_auth.clone())?;
            if owner_key_cutover_valid(tracking.history().as_ref(), &cutover_id).await? {
                creator_auth =
                    creator_auth.with_owner_keys(auth::owner_keys::OwnerKeyAuthenticator::new(
                        verifier,
                        tracking.history(),
                        Arc::new(manager),
                        cutover_id,
                    ));
            }
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

async fn channel_create_authority(
    records: records::RecordClient,
    database: &mongodb::Database,
    issuer: &str,
    required: bool,
    cancellation: bool,
) -> Result<records::RecordClient, records::RecordError> {
    use records::RecordError;
    let reason = match records::mongo::verify_create_fence_floor(database, issuer).await {
        Ok(true) => return Ok(records.with_channel_create_fences(issuer, cancellation)),
        Ok(false) if !required => return Ok(records),
        Ok(false) => "required writer floor missing",
        Err(RecordError::Malformed(_)) => "incompatible writer floor",
        Err(error) => return Err(error),
    };
    tracing::error!(
        record = "writer_floor",
        reason,
        "Channel create authority quarantined"
    );
    Ok(records.quarantine_channel_creates())
}

pub(crate) async fn owner_key_cutover_valid(
    history: &dyn crate::worker_credentials::WorkerHistory,
    cutover_id: &str,
) -> Result<bool, records::RecordError> {
    use records::RecordError;
    let reason = match history.cutover().await {
        Ok(Some(cutover)) if cutover.admits(cutover_id, u64::MAX) => return Ok(true),
        Ok(Some(_)) => "incompatible attestation",
        Ok(None) => "attestation missing",
        Err(RecordError::Malformed(_)) => "attestation decode failed",
        Err(error) => return Err(error),
    };
    tracing::error!(
        record = "owner-lane",
        reason,
        "Owner-key cutover quarantined"
    );
    Ok(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{doc, Document};
    use records::RecordError;

    #[tokio::test]
    #[ignore = "requires an authenticated disposable Mongo replica set"]
    async fn writer_floor_quarantine_distinguishes_absence_and_preserves_corrupt_records() {
        let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let db = client.database(&format!(
            "floor_quarantine_{}",
            uuid::Uuid::new_v4().simple()
        ));
        records::mongo::migrate(&db).await.unwrap();
        let records = records::RecordClient::new("https://cma.example", "fixture-service").unwrap();
        let issuer = "https://issuer.example";
        let legacy = channel_create_authority(records.clone(), &db, issuer, false, false)
            .await
            .unwrap();
        assert!(legacy.require_channel_create_authority().is_ok());
        let required = channel_create_authority(records.clone(), &db, issuer, true, true)
            .await
            .unwrap();
        assert!(required.require_channel_create_authority().is_err());

        records::mongo::migrate_create_fence_floor(&db, issuer)
            .await
            .unwrap();
        let floors = db.collection::<Document>("eg_channel_create_fences");
        let floor = doc! {"_id": "writer_floor"};
        floors
            .update_one(
                floor.clone(),
                doc! {"$set": {"issuer": "https://other.example"}},
            )
            .await
            .unwrap();
        let retained_floor = floors.find_one(floor.clone()).await.unwrap();
        let wrong_issuer = channel_create_authority(records.clone(), &db, issuer, false, false)
            .await
            .unwrap();
        assert!(wrong_issuer.require_channel_create_authority().is_err());
        assert_eq!(floors.find_one(floor).await.unwrap(), retained_floor);
        let markers = db.collection::<Document>(records::mongo::MIGRATIONS);
        let marker = doc! {"_id":"channel_create_fences"};
        markers
            .update_one(marker.clone(), doc! {"$set":{"version":"invalid"}})
            .bypass_document_validation(true)
            .await
            .unwrap();
        let preserved = markers.find_one(marker.clone()).await.unwrap();
        for required in [false, true] {
            let degraded =
                channel_create_authority(records.clone(), &db, issuer, required, required)
                    .await
                    .unwrap();
            assert!(matches!(
                degraded.require_channel_create_authority(),
                Err(RecordError::Unavailable(_))
            ));
            assert!(!degraded.channel_cancellation_enabled());
            assert_eq!(markers.find_one(marker.clone()).await.unwrap(), preserved);
        }
        db.drop().await.unwrap();
    }

    #[tokio::test]
    async fn owner_key_quarantine_keeps_service_keys_out_of_oauth() {
        use crate::worker_credentials::tests::Memory;
        use axum::http::{header, HeaderMap, HeaderValue};

        assert!(!owner_key_cutover_valid(&Memory::default(), "missing")
            .await
            .unwrap());
        let authenticator = auth::CreatorAuthenticator::new("https://issuer.example")
            .unwrap()
            .with_service(
                cma_service_auth::NyxIdAuth::new(
                    "https://issuer.example",
                    cma_service_auth::Service::EventGateway,
                    "fixture-client",
                    "fixture-secret".into(),
                )
                .unwrap(),
            );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer nyx_fixture"),
        );
        assert!(matches!(
            authenticator.authenticate(&headers).await,
            Err(auth::CreatorAuthError::NotAuthenticated)
        ));
    }
}
