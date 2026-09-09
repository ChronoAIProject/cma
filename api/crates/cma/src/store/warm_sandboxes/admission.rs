use layer2_core::profiles::ProfileVisibility;
use layer2_core::sandbox::Sandbox;
use mongodb::bson::doc;

use super::{document, read};
use crate::store::sandbox_admission::AdmissionError;
use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) async fn claim_ready(
    store: &MongoSandboxStore,
    sandbox: &mut Sandbox,
    at: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(), AdmissionError> {
    let Some(runtime_version) = &store.warm_runtime_version else {
        return Ok(());
    };
    let Some(profile_document) = store
        .sandbox_profiles
        .find_one(doc! { "_id": sandbox.profile.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?
    else {
        return Ok(());
    };
    let profile =
        crate::store::sandbox_profiles::read_profile(&profile_document).map_err(|error| {
            AdmissionError::Domain(layer2_core::sandbox::SandboxStoreError::Corrupt(
                error.to_string(),
            ))
        })?;
    if profile.visibility != ProfileVisibility::Public
        || profile.retired_at.is_some()
        || !profile.availability.warmup_enabled
        || profile_document.get_i64("last_revision").ok()
            != Some(i64::from(sandbox.profile.revision))
    {
        return Ok(());
    }
    // A policy edit/unshare/publication must conflict with an in-flight claim.
    store
        .sandbox_profiles
        .update_one(
            doc! { "_id": profile.id.as_str() },
            doc! { "$inc": { "warmup_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    let mut reaches = sandbox.credential.reaches.clone();
    reaches.sort();
    reaches.dedup();
    let Some(candidate) = store.warm_sandboxes.find_one(doc! {
        "profile_id": profile.id.as_str(), "state": "ready",
        "record.plan.profile.revision": i64::from(sandbox.profile.revision),
        "record.plan.profile.digest": &sandbox.profile.digest,
        "record.plan.runtime_version": runtime_version,
        "record.plan.reaches": crate::store::sandbox_mongo::record_bson(&reaches)
            .map_err(|error| AdmissionError::Domain(layer2_core::sandbox::SandboxStoreError::Corrupt(error)))?,
        "lease_until": { "$lte": at },
        "record.updated_at": { "$gte": recent_ready_cutoff(at).map_err(AdmissionError::Domain)?, "$lte": at },
    }).sort(doc! { "_id": 1 }).session(&mut *session).await.map_err(AdmissionError::Driver)? else { return Ok(()); };
    let mut reservation = read(&candidate).map_err(AdmissionError::Domain)?;
    reservation
        .claim(sandbox, runtime_version, at)
        .map_err(AdmissionError::Domain)?;
    store
        .warm_sandboxes
        .replace_one(
            doc! { "_id": reservation.id.as_str(), "state": "ready" },
            document(&reservation).map_err(AdmissionError::Domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    Ok(())
}

fn recent_ready_cutoff(at: &str) -> Result<String, layer2_core::sandbox::SandboxStoreError> {
    let seconds = layer2_core::seconds_from_rfc3339(at)
        .ok_or(layer2_core::sandbox::SandboxStoreError::Conflict)?;
    Ok(layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds.saturating_sub(45)),
    ))
}
