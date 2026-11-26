//! Explicit operator commands. Production activation is a separate release action.
use super::{repository::CaptureBoundary, ActivityHistory, Config, Error, MongoHistory};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{open_blob, MasterKeyRing},
};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::sync::Arc;

/// Attest only after every active/draining writer supports capture. Stopping
/// capture must close the interval before an old writer is restored. Reopening
/// starts a new conservative boundary; old ranges remain partial.
pub async fn run_attestation(from: &str, until: &str, expected: &str) -> Result<(), Error> {
    let from = super::paging::timestamp(from)?;
    let until = if until == "open" {
        None
    } else {
        Some(super::paging::timestamp(until)?)
    };
    let expected = expected.parse::<i64>().map_err(|_| Error::InvalidRequest)?;
    if expected < 1 {
        return Err(Error::InvalidRequest);
    }
    let db = database().await?;
    super::schema::attest(
        &db,
        CaptureBoundary {
            revision: expected.checked_add(1).ok_or(Error::InvalidRequest)?,
            retention_days: Config::from_env()?.retention_days,
            since_ms: Some(from),
            until_ms: until,
        },
        expected,
    )
    .await
}
async fn database() -> Result<Database, Error> {
    let uri = std::env::var(crate::CMAEG_MONGODB_URI).map_err(|_| Error::InvalidConfiguration)?;
    let name =
        std::env::var(crate::CMAEG_MONGODB_DATABASE).map_err(|_| Error::InvalidConfiguration)?;
    let client = mongodb::Client::with_uri_str(uri)
        .await
        .map_err(|_| Error::Unavailable)?;
    Ok(client.database(&name))
}
/// Bounded, resumable channel scan. No raw owner/channel identifiers or sealed
/// rows are printed. The operator may repeat the entire scan safely.
pub async fn run_backfill(after: Option<String>) -> Result<usize, Error> {
    let db = database().await?;
    let spec = crate::config::secrets::read(
        crate::config::CMAEG_MASTER_KEYS,
        crate::CMAEG_MASTER_KEYS_FILE,
    )
    .map_err(|_| Error::InvalidConfiguration)?;
    let keys = Arc::new(MasterKeyRing::parse(&spec).map_err(|_| Error::InvalidConfiguration)?);
    let settings = Config::from_env()?;
    let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::from_env(
        crate::CMAEG_MONGODB_AUTHORITY_MODE,
    )
    .map_err(|_| Error::InvalidConfiguration)?;
    super::schema::verify_grants(&db).await?;
    let history = Arc::new(ActivityHistory::new(
        Some(Arc::new(MongoHistory::open(db.clone()).await?)),
        keys.clone(),
        Config {
            enabled: true,
            capture: false,
            ..settings
        },
    ));
    let mongo = crate::records::mongo::MongoRecords::open_with_authority(db.clone(), authority)
        .await
        .map_err(|_| Error::Unavailable)?;
    let records =
        crate::records::RecordClient::new("http://127.0.0.1:1", uuid::Uuid::new_v4().to_string())
            .map_err(|_| Error::InvalidConfiguration)?
            .with_database(mongo);
    let store =
        crate::activity::ActivityStore::new(records.clone(), keys.clone()).with_history(history);
    let mut after = after;
    let mut count = 0;
    loop {
        let heads = records
            .list(None, after.as_deref(), 100)
            .await
            .map_err(|_| Error::Unavailable)?;
        if heads.is_empty() {
            break;
        }
        for head in heads {
            let value = records
                .get(
                    records
                        .record_url(&head.record_id)
                        .map_err(|_| Error::Unreadable)?
                        .as_str(),
                )
                .await
                .map_err(|_| Error::Unavailable)?;
            let opened = open_blob(&keys, &value.blob).map_err(|_| Error::Unreadable)?;
            let channel: ChannelDefinition =
                serde_json::from_slice(&opened.payload).map_err(|_| Error::Unreadable)?;
            if channel.record_id.as_str() != head.record_id || channel.owner.subject != head.owner {
                return Err(Error::Unreadable);
            }
            count += store
                .backfill_history(&head.owner, &head.record_id)
                .await
                .map_err(|_| Error::Unavailable)?;
            after = Some(head.record_id);
        }
    }
    // Verify the marker is still readable rather than implying this scan
    // attested all-writer completeness. The scan never writes the marker.
    db.collection::<Document>(super::schema::CONTROL)
        .find_one(doc! {"_id":"capture"})
        .await
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Unavailable)?;
    Ok(count)
}
