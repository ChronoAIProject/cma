use super::*;
use crate::sandbox::lifecycle_test_support::{sandbox, NOW};

fn admission(always_on: Option<bool>) -> SandboxAdmission {
    let mut value = sandbox();
    value.origin = SandboxOrigin::Lifecycle {
        operation_id: "policy-proof".into(),
    };
    value.status = SandboxStatus::Provisioning;
    value.runtime = None;
    value.revision = 1;
    value.profile.always_on = always_on;
    value.profile.digest = content_digest(&value.profile.content).unwrap();
    SandboxAdmission::new(
        value.clone(),
        SandboxAdmissionName::Exact(value.name),
        3,
        NOW.into(),
    )
    .unwrap()
}

#[test]
fn retained_policy_proof_preserves_all_original_and_current_states() {
    for original_policy in [None, Some(false), Some(true)] {
        let original = admission(original_policy);
        for current_policy in [None, Some(false), Some(true)] {
            let mut current = original.sandbox.clone();
            current.profile.always_on = current_policy;
            current.revision += 1;
            let replay = SandboxAdmissionReplay::from_retained(
                current.clone(),
                original.name.clone(),
                original.admission_fingerprint.clone(),
            )
            .unwrap();
            assert_eq!(replay.admitted_always_on, original_policy);
            assert_eq!(replay.sandbox, current);
            assert_eq!(replay.admission_fingerprint, original.admission_fingerprint);
            replay.validate().unwrap();
            replay.clone().validate().unwrap();
        }
    }
}

#[test]
fn fresh_admission_and_fresh_replay_require_the_exact_initial_policy() {
    for original_policy in [None, Some(false), Some(true)] {
        let original = admission(original_policy);
        for supplied_policy in [None, Some(false), Some(true)] {
            let mut changed = original.clone();
            changed.sandbox.profile.always_on = supplied_policy;
            assert_eq!(
                changed.validate().is_ok(),
                original_policy == supplied_policy
            );
            assert_eq!(
                SandboxAdmissionReplay::new(
                    changed.sandbox,
                    original.name.clone(),
                    original.admission_fingerprint.clone(),
                )
                .is_ok(),
                original_policy == supplied_policy,
            );
            let new = admission(supplied_policy);
            assert_eq!(
                original.admission_fingerprint == new.admission_fingerprint,
                original_policy == supplied_policy,
            );
        }
    }
}

#[test]
fn retained_proof_remains_bound_during_later_service_revalidation() {
    let original = admission(None);
    let mut current = original.sandbox.clone();
    current.profile.always_on = Some(true);
    let replay = SandboxAdmissionReplay::from_retained(
        current,
        original.name,
        original.admission_fingerprint,
    )
    .unwrap();
    let mut new_policy = replay.clone();
    new_policy.sandbox.profile.always_on = Some(false);
    new_policy.validate().unwrap();
    for mutation in 0..5 {
        let mut changed = replay.clone();
        match mutation {
            0 => changed.admitted_always_on = Some(true),
            1 => changed.sandbox.owner = SubjectId::new("another-owner"),
            2 => changed.sandbox.profile.revision += 1,
            3 => changed.admission_fingerprint = "0".repeat(64),
            _ => changed.name = SandboxAdmissionName::Exact("another-name".into()),
        }
        assert!(changed.validate().is_err(), "mutation {mutation}");
    }
}

#[test]
fn retained_policy_matching_does_not_hide_invalid_current_state_or_digest() {
    let original = admission(None);
    for mutation in 0..6 {
        let mut changed = original.sandbox.clone();
        changed.profile.always_on = Some(true);
        match mutation {
            0 => changed.status = SandboxStatus::Failed,
            1 => changed.revision = 0,
            2 => changed.profile.digest = "0".repeat(64),
            3 => changed.profile.content.cpu_millis += 1,
            4 => changed.origin = SandboxOrigin::Manual,
            _ => changed.limits.max_workspaces = 0,
        }
        assert!(
            SandboxAdmissionReplay::from_retained(
                changed,
                original.name.clone(),
                original.admission_fingerprint.clone(),
            )
            .is_err(),
            "mutation {mutation}",
        );
    }
}

#[test]
fn retained_read_and_revalidation_reject_valid_immutable_identity_changes() {
    use crate::sandbox::SandboxCredentialSource;
    let original = admission(None);
    let mut current = original.sandbox;
    current.profile.always_on = Some(true);
    let replay = SandboxAdmissionReplay::from_retained(
        current,
        original.name,
        original.admission_fingerprint,
    )
    .unwrap();
    for field in [
        "owner",
        "origin",
        "profile_id",
        "profile_revision",
        "profile_content",
        "reaches",
        "credential_source",
        "sign_in",
        "workspace_limit",
        "agent_limit",
        "requested_name",
        "fingerprint",
    ] {
        let mut changed = replay.clone();
        match field {
            "owner" => changed.sandbox.owner = SubjectId::new("another-owner"),
            "origin" => {
                changed.sandbox.origin = SandboxOrigin::Trigger {
                    launch_id: "another-origin".into(),
                }
            }
            "profile_id" => {
                changed.sandbox.profile.id = crate::SandboxProfileId("sbp_other".into())
            }
            "profile_revision" => changed.sandbox.profile.revision += 1,
            "profile_content" => {
                changed.sandbox.profile.content.cpu_millis += 1;
                changed.sandbox.profile.digest =
                    content_digest(&changed.sandbox.profile.content).unwrap();
            }
            "reaches" => changed
                .sandbox
                .credential
                .reaches
                .push(crate::ServiceId::new("another-service")),
            "credential_source" => {
                changed.sandbox.credential.source = SandboxCredentialSource::Provided {
                    key_sha256: "a".repeat(64),
                };
                changed.sandbox.credential.current = None;
                changed.sandbox.credential.delivered = false;
            }
            "sign_in" => changed.sandbox.credential.sign_in = SandboxSignInState::Required,
            "workspace_limit" => changed.sandbox.limits.max_workspaces += 1,
            "agent_limit" => changed.sandbox.limits.max_agents_per_workspace += 1,
            "requested_name" => {
                changed.sandbox.name = "another-name".into();
                changed.name = SandboxAdmissionName::Exact(changed.sandbox.name.clone());
            }
            "fingerprint" => changed.admission_fingerprint = "0".repeat(64),
            _ => unreachable!(),
        }
        changed.sandbox.validate().unwrap();
        assert_eq!(
            content_digest(&changed.sandbox.profile.content).unwrap(),
            changed.sandbox.profile.digest
        );
        assert!(
            matches!(changed.validate(), Err(SandboxStoreError::Corrupt(_))),
            "revalidation accepted {field}"
        );
        assert!(
            matches!(
                SandboxAdmissionReplay::from_retained(
                    changed.sandbox,
                    changed.name,
                    changed.admission_fingerprint,
                ),
                Err(SandboxStoreError::Corrupt(_))
            ),
            "retained reader accepted {field}"
        );
    }
}

#[test]
fn retained_proof_preserves_scope_and_sign_in_normalization() {
    let mut original = admission(None);
    original.sandbox.credential.sign_in = SandboxSignInState::Required;
    original.sandbox.credential.reaches = vec![
        crate::ServiceId::new("service-a"),
        crate::ServiceId::new("service-b"),
    ];
    let original = SandboxAdmission::new(original.sandbox, original.name, 3, NOW.into()).unwrap();
    let mut current = original.sandbox.clone();
    current.profile.always_on = Some(true);
    current.credential.sign_in = SandboxSignInState::Complete;
    current.credential.reaches.reverse();
    current
        .credential
        .reaches
        .push(crate::ServiceId::new("service-a"));
    let replay = SandboxAdmissionReplay::from_retained(
        current.clone(),
        original.name,
        original.admission_fingerprint,
    )
    .unwrap();
    replay.validate().unwrap();
    assert_eq!(replay.sandbox, current);
}

#[tokio::test]
async fn memory_reader_preserves_original_receipt_after_each_policy_write() {
    use super::super::InMemorySandboxStore;
    use crate::sandbox::SandboxMaintenanceStore;
    for origin in [
        SandboxOrigin::Lifecycle {
            operation_id: "memory-policy".into(),
        },
        SandboxOrigin::Trigger {
            launch_id: "memory-policy".into(),
        },
    ] {
        for original_policy in [None, Some(false), Some(true)] {
            let store = InMemorySandboxStore::default();
            let mut original = admission(original_policy);
            original.sandbox.origin = origin.clone();
            let original =
                SandboxAdmission::new(original.sandbox, original.name, 3, NOW.into()).unwrap();
            let SandboxAdmissionResult::Created(mut current) =
                store.admit_sandbox(original.clone()).await.unwrap()
            else {
                panic!("expected creation")
            };
            for policy in [true, false, true] {
                current = store
                    .set_sandbox_always_on(current.id.clone(), current.revision, policy)
                    .await
                    .unwrap();
                let replay = store
                    .sandbox_admission_replay(&current.owner, &origin)
                    .await
                    .unwrap()
                    .unwrap();
                replay.validate().unwrap();
                assert_eq!(replay.sandbox, current);
                assert_eq!(replay.admission_fingerprint, original.admission_fingerprint);
                assert_eq!(
                    store.admit_sandbox(original.clone()).await.unwrap(),
                    SandboxAdmissionResult::Existing(current.clone())
                );
                let mut changed = original.clone();
                changed.sandbox.profile.always_on = Some(!policy);
                if changed.sandbox.profile.always_on != original_policy {
                    let changed =
                        SandboxAdmission::new(changed.sandbox, changed.name, 3, NOW.into())
                            .unwrap();
                    assert_eq!(
                        store.admit_sandbox(changed).await,
                        Err(SandboxStoreError::SandboxOriginConflict)
                    );
                }
            }
        }
    }
}
