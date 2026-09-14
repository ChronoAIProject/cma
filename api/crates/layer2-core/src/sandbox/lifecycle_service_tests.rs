use std::sync::Arc;

use crate::profiles::store::InMemoryProfileStore;
use crate::{Authorization, Caller, Capability, SubjectId};

use super::lifecycle_test_support::{checkpoint, sandbox};
use super::store::SandboxStore;
use super::*;

#[tokio::test]
async fn owner_pause_and_revive_expose_typed_idempotent_outcomes() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profiles,
        SandboxPolicy::default(),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::UseOwnSandboxes]);
    let active = stored_sandbox();
    store.create_sandbox(active.clone()).await.unwrap();

    let scheduled = service.pause(&owner, &active.id).await.unwrap();
    assert_eq!(
        scheduled.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(scheduled.sandbox.status, SandboxStatus::Pausing);
    let replay = service.pause(&owner, &active.id).await.unwrap();
    assert_eq!(
        replay.disposition,
        SandboxLifecycleCommandDisposition::AlreadyScheduled
    );
    assert_eq!(replay.sandbox, scheduled.sandbox);

    let mut paused = stored_sandbox();
    paused.id.0 = "sbx_owner_revive".into();
    paused.name = "owner-revive".into();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.lifecycle.checkpoints.latest = Some(checkpoint(3, "2026-09-05T00:29:00Z"));
    store.create_sandbox(paused.clone()).await.unwrap();
    let revived = service.revive(&owner, &paused.id).await.unwrap();
    assert_eq!(
        revived.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(revived.sandbox.status, SandboxStatus::Reviving);
    assert!(matches!(
        revived.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::ExplicitRequest,
            ..
        })
    ));
}

#[tokio::test]
async fn lifecycle_authorization_happens_before_lookup_and_foreign_ids_are_hidden() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profiles,
        SandboxPolicy::default(),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let value = stored_sandbox();
    store.create_sandbox(value.clone()).await.unwrap();

    assert_eq!(
        service.pause(&caller("owner", []), &value.id).await,
        Err(SandboxLifecycleCommandError::Forbidden)
    );
    assert_eq!(
        service
            .pause(
                &caller("stranger", [Capability::UseOwnSandboxes]),
                &value.id,
            )
            .await,
        Err(SandboxLifecycleCommandError::NotFound)
    );
    assert_eq!(
        service
            .pause(
                &caller("stranger", [Capability::UseOwnSandboxes]),
                &crate::SandboxId("sbx_missing".into()),
            )
            .await,
        Err(SandboxLifecycleCommandError::NotFound)
    );
    assert_eq!(store.get_sandbox(&value.id).await.unwrap(), Some(value));
}

#[tokio::test]
async fn ineligible_lifecycle_states_have_route_specific_errors() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profiles,
        SandboxPolicy::default(),
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::UseOwnSandboxes]);
    let mut retiring = stored_sandbox();
    retiring.status = SandboxStatus::Retiring;
    retiring
        .work
        .schedule(SandboxWorkStep::StopAgents, "2026-09-05T00:31:00Z".into());
    store.create_sandbox(retiring.clone()).await.unwrap();

    assert_eq!(
        service.pause(&owner, &retiring.id).await,
        Err(SandboxLifecycleCommandError::SandboxNotActive {
            status: SandboxStatus::Retiring,
        })
    );
    assert_eq!(
        service.revive(&owner, &retiring.id).await,
        Err(SandboxLifecycleCommandError::SandboxNotPaused {
            status: SandboxStatus::Retiring,
        })
    );
}

#[test]
fn lifecycle_command_errors_have_stable_rest_codes() {
    assert_eq!(SandboxLifecycleCommandError::Forbidden.code(), "forbidden");
    assert_eq!(
        SandboxLifecycleCommandError::NotFound.code(),
        "sandbox_not_found"
    );
    assert_eq!(
        SandboxLifecycleCommandError::SandboxNotActive {
            status: SandboxStatus::Retiring,
        }
        .code(),
        "sandbox_not_active"
    );
    assert_eq!(
        SandboxLifecycleCommandError::SandboxNotPaused {
            status: SandboxStatus::Active,
        }
        .code(),
        "sandbox_not_paused"
    );
    assert_eq!(
        SandboxLifecycleCommandError::Corrupt("bad".into()).code(),
        "stored_sandbox_corrupt"
    );
    assert_eq!(
        SandboxLifecycleCommandError::Unavailable("down".into()).code(),
        "sandbox_lifecycle_unavailable"
    );
}

fn caller(subject: &str, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(SubjectId(subject.into()), Authorization::new(capabilities))
}

fn stored_sandbox() -> Sandbox {
    let mut value = sandbox();
    value.revision = 1;
    value
}
