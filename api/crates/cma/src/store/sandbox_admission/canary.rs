//! Bind canary Sandbox admission to the verified lifecycle operation.
use super::*;
use crate::agent_lifecycle::canary_sandbox_content;
use crate::store::agent_lifecycle_operations::{
    canary_admission::{bind_sandbox, image_digest, MANIFEST_DIGEST, SANDBOX_PROFILE_ID},
    LifecycleLease, LifecycleRecord, COLLECTION,
};
use layer2_core::profiles::{content_digest, private_sandbox_profile_id};
use layer2_core::sandbox::SandboxRuntimeImageOrigin;
use mongodb::{ClientSession, Database};

pub(super) enum ImageAdmission {
    Canary(LifecycleLease),
    Ordinary(Option<SandboxRuntimeImageOrigin>),
}

impl ImageAdmission {
    pub(super) async fn record_origin(
        &self,
        store: &MongoSandboxStore,
        sandbox: &mut Sandbox,
        session: &mut ClientSession,
    ) -> Result<(), AdmissionError> {
        if sandbox.lifecycle.runtime_image_origin.is_some() {
            return Err(refused());
        }
        if let Self::Ordinary(Some(origin)) = self {
            crate::store::work_schema::floor::retain_runtime_image_origin(store, session)
                .await
                .map_err(AdmissionError::Driver)?;
            sandbox.lifecycle.runtime_image_origin = Some(*origin);
        }
        Ok(())
    }
}

pub(super) async fn verify(
    database: &Database,
    admission: &SandboxAdmission,
    record_origin: bool,
    session: &mut ClientSession,
) -> Result<ImageAdmission, AdmissionError> {
    let sandbox = &admission.sandbox;
    let operations = database.collection::<LifecycleRecord>(COLLECTION);
    // Match by profile ID across revisions and consumers. A private profile
    // reference alone must never authorize another canary Sandbox.
    let by_profile = operations
        .find_one(doc! { SANDBOX_PROFILE_ID: sandbox.profile.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    let lifecycle_id = match &sandbox.origin {
        SandboxOrigin::Lifecycle { operation_id } => Some(operation_id),
        _ => None,
    };
    let row = match (by_profile, lifecycle_id) {
        (Some(row), Some(id)) if row.id == *id => Some(row),
        (Some(_), _) => return Err(refused()),
        (None, Some(id)) => operations
            .find_one(doc! {"_id": id})
            .session(&mut *session)
            .await
            .map_err(AdmissionError::Driver)?,
        (None, None) => None,
    };
    if let Some(row) = row.as_ref() {
        if let Some(profiles) = row.verify_canary_worker().map_err(|_| refused())? {
            return verify_canary(admission, row, profiles).map(ImageAdmission::Canary);
        }
    }
    verify_ordinary_pin(database, sandbox, row.as_ref(), session).await?;
    Ok(ImageAdmission::Ordinary(
        row.as_ref()
            .filter(|_| record_origin)
            .and_then(|row| super::runtime_image_origin::selection(admission, row)),
    ))
}

fn verify_canary(
    admission: &SandboxAdmission,
    row: &LifecycleRecord,
    profiles: crate::store::CanaryProfiles,
) -> Result<LifecycleLease, AdmissionError> {
    let sandbox = &admission.sandbox;
    let profile = &sandbox.profile;
    let expected = canary_sandbox_content(&row.input, profiles.workspace).map_err(|_| refused())?;
    if row.owner != sandbox.owner.as_str()
        || row.status != "running"
        || row.sandbox_id.is_some()
        || admission.binding.is_some()
        || profile.id != profiles.sandbox.id
        || profile.revision != profiles.sandbox.revision
        || profile.always_on.is_some()
        || profile.content != expected
        || content_digest(&expected).map_err(|_| refused())? != profile.digest
    {
        return Err(refused());
    }
    let token = admission.lifecycle_lease().ok_or_else(refused)?;
    if row.lease_token.as_deref() != Some(token) {
        return Err(refused());
    }
    Ok(LifecycleLease {
        id: row.id.clone(),
        token: token.to_owned(),
    })
}

async fn verify_ordinary_pin(
    database: &Database,
    sandbox: &Sandbox,
    operation: Option<&LifecycleRecord>,
    session: &mut ClientSession,
) -> Result<(), AdmissionError> {
    let Some(image) = sandbox.profile.content.runtime_image.as_deref() else {
        return Ok(());
    };
    let Some(digest) = image_digest(image) else {
        return Ok(());
    };
    let Some(witness) = database
        .collection::<LifecycleRecord>(COLLECTION)
        .find_one(doc! { MANIFEST_DIGEST: digest })
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?
    else {
        return Ok(());
    };
    witness
        .verify_canary_worker()
        .map_err(|_| refused())?
        .ok_or_else(refused)?;
    let ordinary_authority = match &sandbox.origin {
        SandboxOrigin::Manual => false,
        SandboxOrigin::Trigger { launch_id } => {
            crate::store::profile_pin_admission::frozen::inline_image(
                database, sandbox, launch_id, session,
            )
            .await
            .map_err(AdmissionError::Driver)?
        }
        SandboxOrigin::Lifecycle { .. } => {
            operation.is_some_and(|record| ordinary_lifecycle_pin(sandbox, record, image))
        }
    };
    if ordinary_authority {
        Ok(())
    } else {
        Err(refused())
    }
}

fn ordinary_lifecycle_pin(sandbox: &Sandbox, record: &LifecycleRecord, image: &str) -> bool {
    if record.owner != sandbox.owner.as_str()
        || record.input.resolved_image.as_deref() != Some(image)
        || !(record.input.request.machine_included
            || record.input.request.image.is_some()
            || (record.input.request.desktop && record.input.request.initial_message.is_some()))
    {
        return false;
    }
    let key = format!("agent-manager:{}:sandbox", record.key);
    private_sandbox_profile_id(&sandbox.owner, &key)
        .is_ok_and(|id| id == sandbox.profile.id && sandbox.profile.revision == 1)
}

pub(super) async fn bind(
    database: &Database,
    lease: &LifecycleLease,
    id: &layer2_core::SandboxId,
    session: &mut ClientSession,
) -> Result<(), AdmissionError> {
    if bind_sandbox(&database.collection(COLLECTION), lease, id, session)
        .await
        .map_err(AdmissionError::Driver)?
    {
        Ok(())
    } else {
        Err(refused())
    }
}

fn refused() -> AdmissionError {
    AdmissionError::Domain(SandboxStoreError::SandboxOriginConflict)
}
