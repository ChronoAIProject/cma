//! Transfer a fresh Launch's durable home reservation into its Sandbox row.
use super::*;

pub(super) enum Reservation {
    Legacy,
    Reserved(String),
    Reused(String),
}

impl Reservation {
    pub(super) fn checks_operator(&self) -> bool {
        matches!(self, Self::Reused(_))
    }

    pub(super) fn counts_pending(&self) -> bool {
        !matches!(self, Self::Legacy)
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    session: &mut mongodb::ClientSession,
) -> Result<Reservation, AdmissionError> {
    let SandboxOrigin::Trigger { launch_id } = &admission.sandbox.origin else {
        return Ok(Reservation::Legacy);
    };
    let Some(row) = store
        .sandbox_admissions
        .find_one(key(launch_id))
        .session(session)
        .await
        .map_err(AdmissionError::Driver)?
    else {
        return Ok(Reservation::Legacy);
    };
    super::super::super::sandbox_mongo::require_version(&row).map_err(|_| refused())?;
    if row.get_str("owner").ok() != Some(admission.sandbox.owner.as_str()) {
        return Err(refused());
    }
    match row.get_str("state").ok() {
        Some("reserved") => Ok(Reservation::Reserved(launch_id.clone())),
        Some("reused") => Ok(Reservation::Reused(launch_id.clone())),
        // Exact Sandbox replay precedes this lookup. A consumed receipt without
        // that Sandbox is inconsistent and must never authorize a second home.
        _ => Err(refused()),
    }
}

pub(super) async fn complete(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    reservation: &Reservation,
    session: &mut mongodb::ClientSession,
) -> Result<(), AdmissionError> {
    let (launch, state) = match reservation {
        Reservation::Legacy => return Ok(()),
        Reservation::Reserved(launch) => (launch, "reserved"),
        Reservation::Reused(launch) => (launch, "reused"),
    };
    if state == "reserved" {
        let decremented = store.sandbox_admissions.update_one(
            doc! {"_id": {"scope": "global_sandbox_capacity"}, "pending_launch_homes": {"$gte": 1_i64}},
            doc! {"$inc": {"pending_launch_homes": -1_i64}},
        ).session(&mut *session).await.map_err(AdmissionError::Driver)?;
        if decremented.matched_count != 1 {
            return Err(refused());
        }
    }
    let mut filter = key(launch);
    filter.insert("owner", admission.sandbox.owner.as_str());
    filter.insert("state", state);
    filter.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
    let completed = store
        .sandbox_admissions
        .update_one(
            filter,
            doc! {"$set": {"state": "consumed", "sandbox_id": admission.sandbox.id.as_str()}},
        )
        .session(session)
        .await
        .map_err(AdmissionError::Driver)?;
    if completed.matched_count != 1 {
        return Err(refused());
    }
    Ok(())
}

pub(super) async fn pending(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<u64, AdmissionError> {
    let row = store
        .sandbox_admissions
        .find_one(doc! {"_id": {"scope": "global_sandbox_capacity"}})
        .session(session)
        .await
        .map_err(AdmissionError::Driver)?;
    super::super::super::launch_home_capacity::pending_homes(row.as_ref()).map_err(|_| refused())
}

fn key(launch: &str) -> mongodb::bson::Document {
    super::super::super::launch_home_capacity::receipt_key(launch)
}

fn refused() -> AdmissionError {
    AdmissionError::Domain(SandboxStoreError::GlobalCapacityReached)
}
