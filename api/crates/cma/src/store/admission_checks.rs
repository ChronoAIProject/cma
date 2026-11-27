//! Admission checks at durable operation boundaries whose resource steps
//! subsequently retain the existing platform spend checks.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::{admission::AdmissionCeilings, SubjectId};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

/// Existing transactions retain their in-snapshot admission read.
pub(super) async fn require_fresh(
    settings: &Collection<Document>,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    require_fresh_home(settings, false, session).await
}

/// Fresh replacements read home policy in their existing admission snapshot.
pub(super) async fn require_fresh_home(
    settings: &Collection<Document>,
    creates_home: bool,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let document = settings
        .find_one(doc! {"_id": "platform"})
        .session(session)
        .await
        .map_err(|_| SandboxStoreError::Unavailable("admission control read failed".into()))?;
    require_readable_home(document.as_ref(), creates_home)
}

/// Check capacity without reserving it. Sandbox admission serializes the
/// capacity decision with its insert after the lifecycle worker claims work.
pub(super) async fn require_fresh_lifecycle(
    settings: &Collection<Document>,
    new_home: Option<(&SubjectId, u16)>,
) -> Result<(), SandboxStoreError> {
    use super::launch_home_capacity::{check_room, Room};
    use super::registration_capacity::activation;

    let document = settings
        .find_one(doc! {"_id": "platform"})
        .await
        .map_err(|_| SandboxStoreError::Unavailable("admission control read failed".into()))?;
    require_readable_open(document.as_ref())?;
    let Some((owner, deployment_limit)) = new_home else {
        return Ok(());
    };
    let policy = document.map_or_else(
        || Ok(AdmissionCeilings::default()),
        |document| super::admission_ceilings::from_document(&document).map(|state| state.ceilings),
    );
    if matches!(&policy, Ok(policy) if policy.effective_retained_homes(deployment_limit).is_none())
    {
        return Ok(());
    }

    let database = settings.client().database(&settings.namespace().db);
    let admissions =
        super::durability::majority_collection(&database, super::sandbox_mongo::SANDBOX_ADMISSIONS);
    let capacity = SandboxStoreError::GlobalCapacityReached;
    let mut session = settings
        .client()
        .start_session()
        .await
        .map_err(|_| capacity.clone())?;
    if !activation::read(&admissions, Some(&mut session))
        .await
        .map_err(|_| capacity.clone())?
    {
        return Ok(());
    }
    let policy = policy.map_err(|_| capacity.clone())?;
    let Some(limit) = policy.effective_retained_homes(deployment_limit) else {
        return Ok(());
    };
    let sandboxes =
        super::durability::majority_collection(&database, super::sandbox_mongo::SANDBOXES);
    match check_room(
        &admissions,
        &sandboxes,
        limit,
        policy.existing_reserve.retained_homes.unwrap_or(0),
        owner,
        &mut session,
    )
    .await
    .map_err(|_| capacity.clone())?
    {
        Room::Available => Ok(()),
        Room::Full | Room::Unreadable => Err(capacity),
    }
}

/// A newly admitted home needs readable ceilings before its continuation can
/// rely on the durable binding. Reuse the existing control/settings read.
pub(super) async fn require_fresh_home_read(
    settings: &Collection<Document>,
    creates_home: bool,
) -> Result<(), SandboxStoreError> {
    let document = settings
        .find_one(doc! {"_id": "platform"})
        .await
        .map_err(|_| SandboxStoreError::Unavailable("admission control read failed".into()))?;
    require_readable_home(document.as_ref(), creates_home)
}

fn require_readable_home(
    document: Option<&Document>,
    creates_home: bool,
) -> Result<(), SandboxStoreError> {
    require_readable_open(document)?;
    if let Some(document) = document.filter(|_| creates_home) {
        super::admission_ceilings::from_document(document)
            .map_err(|_| SandboxStoreError::GlobalCapacityReached)?;
    }
    Ok(())
}

fn require_readable_open(document: Option<&Document>) -> Result<(), SandboxStoreError> {
    let Some(document) = document else {
        return Ok(());
    };
    match super::admission_control::from_document(document) {
        Ok(control) if control.operator_stop.is_none() => Ok(()),
        // Unreadable control fails closed for Fresh work only. Admitted work
        // never calls this operator-only gate.
        _ => Err(SandboxStoreError::AdmissionPaused),
    }
}
