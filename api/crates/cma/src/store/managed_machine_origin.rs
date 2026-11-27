//! Recover managed Machine provenance from the original retained creation.
use layer2_core::profiles::{
    content_digest, private_sandbox_profile_id, private_workspace_profile_id,
};
use layer2_core::released_runtime::ReleasedLane;
use layer2_core::sandbox::{
    Sandbox, SandboxOrigin, SandboxRuntimeImageOrigin, SandboxStatus, SandboxStoreError,
    SandboxWorkStep,
};
use mongodb::{bson::doc, ClientSession};

use super::agent_lifecycle_operations::{LifecycleRecord, COLLECTION};
use super::sandbox_events::TransactionEventError::{self, Domain, Driver};
use super::sandbox_mongo::MongoSandboxStore;

pub(super) async fn qualify(
    store: &MongoSandboxStore,
    current: &Sandbox,
    at: &str,
) -> Result<Option<Sandbox>, SandboxStoreError> {
    if !store.managed_machine_updates.includes(&current.id) || original_image(current).is_none() {
        return Ok(None);
    }
    let next = match qualify_successor(current, at) {
        Ok(next) => next,
        Err(SandboxStoreError::InvalidTransaction(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    match super::sandbox_usage::save::save(store, current.revision, &next, None, None).await {
        Ok(()) => {
            tracing::info!(sandbox_id = %next.id, revision = next.revision, "Retained Machine runtime origin qualified");
            Ok(Some(next))
        }
        Err(SandboxStoreError::InvalidTransaction(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) async fn fence(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), TransactionEventError> {
    if !store.managed_machine_updates.includes(&current.id)
        || !matches!(qualify_successor(current, &next.updated_at), Ok(expected) if expected == *next)
    {
        return Err(Domain(SandboxStoreError::Conflict));
    }
    let SandboxOrigin::Lifecycle { operation_id } = &current.origin else {
        return Err(Domain(unproved()));
    };
    let rows = store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection::<LifecycleRecord>(COLLECTION);
    let row = rows
        .find_one(doc! {
            "_id": operation_id,
            "owner": current.owner.as_str(),
            "sandbox_id": current.id.as_str(),
        })
        .session(&mut *session)
        .await
        .map_err(Driver)?
        .ok_or_else(|| Domain(unproved()))?;
    verify_creation(current, &row).map_err(Domain)?;
    super::native_prepared::verify_machine_origin(store, current, operation_id, session).await?;
    super::work_schema::floor::retain_managed_machine_origin(store, session)
        .await
        .map_err(Driver)
}

fn qualify_successor(current: &Sandbox, at: &str) -> Result<Sandbox, SandboxStoreError> {
    original_image(current).ok_or_else(unproved)?;
    let mut next = current.clone();
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    next.updated_at = at.into();
    next.lifecycle.runtime_image_origin = Some(SandboxRuntimeImageOrigin::ManagedMachine);
    next.validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    if !next.supports_retained_runtime_update() || !next.managed_desktop_runtime_confirmed() {
        return Err(unproved());
    }
    Ok(next)
}

fn original_image(sandbox: &Sandbox) -> Option<&str> {
    // Managed credentials schedule their next rotation during activation.
    // An unclaimed wait is idle for retained image rotation as well.
    if !matches!(
        sandbox.status,
        SandboxStatus::Active | SandboxStatus::Paused
    ) || sandbox.lifecycle.runtime_image_origin.is_some()
        || sandbox.lifecycle.custom_runtime.is_some()
        || sandbox.lifecycle.runtime_migration_admission.is_some()
        || sandbox.lifecycle.operation.is_some()
        || sandbox.work.lease.is_some()
        || !matches!(
            sandbox.work.step,
            SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
        )
    {
        return None;
    }
    let home = sandbox.lifecycle.home.as_ref()?;
    let native = home.native_pool.as_ref()?;
    let original = sandbox.profile.content.runtime_image.as_deref()?;
    let binding = sandbox.retained_runtime_binding()?;
    (native.claim.is_some()
        && !native.cleaned
        && native.cleanup.is_none()
        && binding.runtime_version == original
        && native.request.pool.spec.image.uri == original
        && crate::agent_lifecycle::valid_immutable_image(original))
    .then_some(original)
}

fn verify_creation(sandbox: &Sandbox, row: &LifecycleRecord) -> Result<(), SandboxStoreError> {
    let original = original_image(sandbox).ok_or_else(unproved)?;
    let sandbox_key = format!("agent-manager:{}:sandbox", row.key);
    let workspace_key = format!("agent-manager:{}:workspace", row.key);
    let [reference] = sandbox.profile.content.workspace_profiles.as_slice() else {
        return Err(unproved());
    };
    if row.admitted_machine_image() != Some(original)
        || row
            .released_lane
            .is_some_and(|lane| lane != ReleasedLane::Machine)
        || private_sandbox_profile_id(&sandbox.owner, &sandbox_key)
            .ok()
            .as_ref()
            != Some(&sandbox.profile.id)
        || private_workspace_profile_id(&sandbox.owner, &workspace_key)
            .ok()
            .as_ref()
            != Some(&reference.id)
        || sandbox.profile.revision != 1
        || reference.revision != 1
    {
        return Err(unproved());
    }
    let expected = crate::agent_lifecycle::canary_sandbox_content(&row.input, reference.clone())
        .map_err(|_| unproved())?;
    if expected != sandbox.profile.content
        || content_digest(&expected).ok().as_ref() != Some(&sandbox.profile.digest)
    {
        return Err(unproved());
    }
    Ok(())
}

fn unproved() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("retained Machine creation authority is unproved")
}
