use std::sync::Arc;

use super::lifecycle_test_support::sandbox;
use super::store::{InMemorySandboxStore, SandboxStore};
use super::*;
use crate::{Authorization, Caller, Capability};

fn admitted_sandbox(origin: SandboxOrigin, always_on: Option<bool>) -> store::SandboxAdmission {
    let mut value = sandbox();
    value.revision = 1;
    value.origin = origin;
    value.status = SandboxStatus::Provisioning;
    value.profile.always_on = always_on;
    value.profile.digest = crate::profiles::content_digest(&value.profile.content).unwrap();
    store::SandboxAdmission::new(
        value.clone(),
        store::SandboxAdmissionName::Exact(value.name),
        3,
        lifecycle_test_support::NOW.into(),
    )
    .unwrap()
}

#[tokio::test]
async fn always_on_changes_preserve_exact_trigger_and_lifecycle_admission_replay() {
    use store::{SandboxAdmissionResult, SandboxAdmissionStore, SandboxStoreError};
    for origin in [
        SandboxOrigin::Trigger {
            launch_id: "lch_policy".into(),
        },
        SandboxOrigin::Lifecycle {
            operation_id: "lop_policy".into(),
        },
    ] {
        for original_policy in [None, Some(false), Some(true)] {
            let store = InMemorySandboxStore::default();
            let original = admitted_sandbox(origin.clone(), original_policy);
            let SandboxAdmissionResult::Created(mut current) =
                store.admit_sandbox(original.clone()).await.unwrap()
            else {
                panic!("new admission must create");
            };
            for policy in [true, false, true] {
                let stale_revision = current.revision;
                current = store
                    .set_sandbox_always_on(current.id.clone(), current.revision, policy)
                    .await
                    .unwrap();
                assert_eq!(current.profile.always_on, Some(policy));
                assert_eq!(
                    store
                        .set_sandbox_always_on(current.id.clone(), stale_revision, !policy)
                        .await,
                    Err(SandboxStoreError::Conflict)
                );
                let replay = store
                    .sandbox_admission_replay(&current.owner, &current.origin)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(replay.sandbox, current);
                assert_eq!(replay.admission_fingerprint, original.admission_fingerprint);
                assert_eq!(
                    store.admit_sandbox(original.clone()).await.unwrap(),
                    SandboxAdmissionResult::Existing(current.clone())
                );
                // A different initial policy remains a different admission,
                // even when it happens to equal the current mutable policy.
                for initial in [None, Some(false), Some(true)] {
                    if initial == original_policy {
                        continue;
                    }
                    let changed = admitted_sandbox(origin.clone(), initial);
                    assert_eq!(
                        store.admit_sandbox(changed).await,
                        Err(SandboxStoreError::SandboxOriginConflict)
                    );
                }
            }
        }
    }
}

#[test]
fn always_on_replay_still_rejects_changed_immutable_identity() {
    use store::SandboxAdmissionReplay;
    let original = admitted_sandbox(
        SandboxOrigin::Trigger {
            launch_id: "lch_policy".into(),
        },
        None,
    );
    for drift in 0..9 {
        let mut changed = original.sandbox.clone();
        changed.profile.always_on = Some(true);
        let mut name = original.name.clone();
        let mut fingerprint = original.admission_fingerprint.clone();
        match drift {
            0 => changed.owner = crate::SubjectId::new("different-owner"),
            1 => {
                changed.origin = SandboxOrigin::Trigger {
                    launch_id: "different-origin".into(),
                }
            }
            2 => changed.profile.id = crate::ids::SandboxProfileId("different-profile".into()),
            3 => changed.profile.revision += 1,
            4 => {
                changed.profile.content.cpu_millis += 1;
                changed.profile.digest =
                    crate::profiles::content_digest(&changed.profile.content).unwrap();
            }
            5 => changed
                .credential
                .reaches
                .push(crate::ServiceId::new("different-service")),
            6 => changed.limits.max_agents_per_workspace += 1,
            7 => {
                changed.name = "different-name".into();
                name = store::SandboxAdmissionName::Exact(changed.name.clone());
            }
            8 => fingerprint = "0".repeat(64),
            _ => unreachable!(),
        }
        assert!(
            SandboxAdmissionReplay::new(changed, name, fingerprint).is_err(),
            "drift {drift}"
        );
    }
    let mut altered_request = original;
    altered_request.sandbox.profile.always_on = Some(true);
    assert!(
        altered_request.validate().is_err(),
        "fresh admission must still match its exact policy"
    );
}

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
        cause: Default::default(),
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
