use std::sync::Arc;

use super::lifecycle_test_support::sandbox;
use super::store::{InMemorySandboxStore, SandboxStore};
use super::*;
use crate::{Authorization, Caller, Capability};

#[tokio::test]
async fn owner_always_on_refusal_and_relinquishment_preserve_revisions_and_absence() {
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        Arc::new(crate::profiles::InMemoryProfileStore::default()),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let mut original = sandbox();
    original.revision = 1;
    let caller = Caller::new(
        original.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let mut other = caller.clone();
    other.subject = crate::SubjectId::new("another-owner");
    for preference in [None, Some(false), Some(true)] {
        let mut current = original.clone();
        current.profile.always_on = preference;
        // Separate identities prove absence is retained on ordinary saves.
        current.id = crate::SandboxId(format!("sbx_{preference:?}"));
        current.name = format!("preference-{preference:?}");
        store.create_sandbox(current.clone()).await.unwrap();
        for revision in [current.revision, current.revision + 1] {
            let error = service
                .set_always_on(&caller, &current.id, revision, true)
                .await
                .unwrap_err();
            assert_eq!(error, SandboxServiceError::AlwaysOnApprovalRequired);
            assert_eq!(error.code(), "always_on_approval_required");
        }
        assert_eq!(
            store.get_sandbox(&current.id).await.unwrap(),
            Some(current.clone())
        );
        for value in [false, true] {
            assert_eq!(
                service
                    .set_always_on(&other, &current.id, current.revision, value)
                    .await,
                Err(SandboxServiceError::NotFound)
            );
        }
        assert_eq!(
            service
                .set_always_on(&caller, &current.id, current.revision + 1, false)
                .await,
            Err(SandboxServiceError::InvalidState)
        );
        let saved = service
            .set_always_on(&caller, &current.id, current.revision, false)
            .await
            .unwrap();
        if preference == Some(true) {
            assert_eq!(saved.profile.always_on, Some(false));
            assert_eq!(saved.revision, current.revision + 1);
        } else {
            assert_eq!(saved, current);
        }
        assert_eq!(
            service
                .set_always_on(&caller, &saved.id, saved.revision, false)
                .await
                .unwrap(),
            saved
        );
        assert_eq!(store.get_sandbox(&saved.id).await.unwrap(), Some(saved));
    }
}

#[tokio::test]
async fn memory_maintenance_skips_always_on_then_pauses_after_revocation() {
    use super::lifecycle_test_support::{agent, IDLE_AT, NOW};
    use super::store::{AgentLogStore, AgentStore};
    let store = InMemorySandboxStore::default();
    let mut current = sandbox();
    current.revision = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    let mut ready = agent("agt_exception", "conversation-exception", 1);
    ready.revision = 1;
    store.create_agent(ready.clone()).await.unwrap();
    store
        .append_agent_log(
            &ready.id,
            IDLE_AT.into(),
            AgentLogKind::AgentMessage {
                content: "Idle evidence".into(),
            },
        )
        .await
        .unwrap();
    let approved = store
        .set_sandbox_always_on(current.id.clone(), current.revision, true)
        .await
        .unwrap();
    let request = SandboxInactivityPauseRequest {
        sandbox_id: current.id.clone(),
        requested_at: NOW.into(),
        pause_after_seconds: 900,
        auto_pause_enabled: true,
    };
    assert_eq!(
        store
            .schedule_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap(),
        Some(approved.clone())
    );
    store
        .set_sandbox_always_on(current.id.clone(), approved.revision, false)
        .await
        .unwrap();
    assert_eq!(
        store.schedule_inactivity_pause(request).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        store
            .get_sandbox(&current.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
}
