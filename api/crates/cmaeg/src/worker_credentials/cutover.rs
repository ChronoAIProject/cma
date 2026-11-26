//! Explicit operator cutover. A retained-record scan is not historical proof.
use super::{
    config::Settings,
    mongo::{failure, options, CUTOVERS, CUTOVER_BARRIER},
    Cutover, WorkerCredentials,
};
use crate::records::{
    mongo::{MIGRATIONS, RECORDS},
    RecordError,
};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, DateTime, Document},
    Database,
};

pub async fn attest(
    db: &Database,
    writer_release: &str,
    all_writers_drained: bool,
) -> Result<Cutover, RecordError> {
    let settings = Settings::from_env()?;
    if !all_writers_drained
        || !settings.tracking
        || settings.owner_keys
        || writer_release.len() != 40
        || !writer_release
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(RecordError::Rejected("cutover requires tracked writers, owner keys disabled, exact release and explicit old-writer drain attestation".into()));
    }
    let tracking = WorkerCredentials::from_environment(db)
        .await?
        .ok_or_else(|| RecordError::Rejected("worker tracking required".into()))?;
    let controls = db.collection_with_options::<Cutover>(CUTOVERS, options());
    let previous = controls
        .find_one(doc! {"_id":"owner-lane"})
        .await
        .map_err(failure)?;
    backfill(db, &tracking).await?;
    let hello = db.run_command(doc! {"hello":1}).await.map_err(failure)?;
    let server = hello
        .get_datetime("localTime")
        .map_err(|_| RecordError::Unavailable("database cutover clock unavailable".into()))?
        .timestamp_millis()
        / 1000;
    let local = DateTime::now().timestamp_millis() / 1000;
    if server <= 0 || (server - local).abs() > 5 {
        return Err(RecordError::Unavailable(
            "cutover clocks differ; no attestation written".into(),
        ));
    }
    // Reject the cutover second and a conservative cross-service clock window.
    // A child credential retains its parent's NyxID iat and cannot evade this.
    let not_before = (server.max(local) + 5).max(
        previous
            .as_ref()
            .map_or(0, |v| v.not_before.saturating_add(1)),
    );
    let revision = previous
        .as_ref()
        .map_or(Some(1), |v| v.revision.checked_add(1))
        .ok_or_else(|| RecordError::Rejected("cutover revision exhausted".into()))?;
    let next = Cutover {
        id: "owner-lane".into(),
        attestation_id: uuid::Uuid::new_v4().to_string(),
        not_before,
        revision,
        writer_release: writer_release.into(),
        all_writers_tracked: true,
        retained_records_backfilled: true,
    };
    // Preserve immutable audit receipts even if a concurrent pointer CAS loses.
    let archived = Cutover {
        id: next.attestation_id.clone(),
        ..next.clone()
    };
    controls.insert_one(&archived).await.map_err(failure)?;
    db.collection_with_options::<Document>(MIGRATIONS, options())
        .update_one(
            doc! {"_id":CUTOVER_BARRIER},
            doc! {"$setOnInsert":{"version":1}},
        )
        .upsert(true)
        .await
        .map_err(failure)?;
    match previous {
        None => {
            controls.insert_one(&next).await.map_err(failure)?;
        }
        Some(old) => {
            let result=controls.replace_one(doc!{"_id":"owner-lane","revision":old.revision,"not_before":old.not_before,"attestation_id":old.attestation_id},&next).await.map_err(failure)?;
            if result.modified_count != 1 {
                return Err(RecordError::Rejected(
                    "cutover changed concurrently; inspect before retry".into(),
                ));
            }
        }
    }
    Ok(next)
}

async fn backfill(db: &Database, tracking: &WorkerCredentials) -> Result<(), RecordError> {
    let records = db.collection::<Document>(RECORDS);
    let mut after = None;
    loop {
        let filter = after
            .as_ref()
            .map_or_else(|| doc! {}, |id: &String| doc! {"_id":{"$gt":id}});
        let page: Vec<Document> = records
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(128)
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        if page.is_empty() {
            return Ok(());
        }
        for row in page {
            let id = row
                .get_str("_id")
                .map_err(|_| RecordError::Malformed("invalid channel record ID".into()))?;
            let blob = row
                .get_binary_generic("blob")
                .map_err(|_| RecordError::Malformed("invalid channel record blob".into()))?;
            let version = row
                .get_i64("version")
                .ok()
                .and_then(|version| u64::try_from(version).ok())
                .ok_or_else(|| RecordError::Malformed("invalid channel record version".into()))?;
            tracking.record_blob(id, version, blob).await?;
            after = Some(id.to_owned());
        }
    }
}
