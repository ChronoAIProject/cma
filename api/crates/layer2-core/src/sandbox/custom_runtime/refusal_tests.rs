use super::test_support::*;
use crate::sandbox::lifecycle_test_support::NOW;
use crate::sandbox::*;

#[test]
fn custom_runtime_rejects_unapproved_mutable_wrong_family_and_stale_requests_atomically() {
    let original = custom();
    for case in 0..12 {
        let mut value = original.clone();
        let mut command = request(&value, 'b', "migration-1");
        let mut approved = image('b');
        match case {
            0 => approved = image('c'),
            1 => command.target_image = "registry.example/runtime:latest".into(),
            2 => command.target_image = image('b').replace("/runtime@", "/desktop@"),
            3 => command.target_image = image('b').replace("/runtime@", "/runtime:tag@"),
            4 => command.expected_image = image('c'),
            5 => command.expected_revision -= 1,
            6 => command.origin.owner.0 = "other".into(),
            7 => command.origin.profile_digest = "f".repeat(64),
            8 => command.origin.home.receipt.storage_generation = "foreign".into(),
            9 => command.request_key = "\n".into(),
            10 => command.origin.profile_revision += 1,
            11 => command.target_image = image('b').replace("sha256", "sha512"),
            _ => unreachable!(),
        }
        assert!(
            value
                .request_custom_runtime_migration(command, &approved, NOW.into())
                .is_err(),
            "case {case}"
        );
        assert_eq!(value, original, "case {case}");
    }
}

#[test]
fn custom_runtime_rejects_competing_request_or_same_key_with_new_payload() {
    let mut value = custom();
    let accepted = queue(&mut value, 'b', "migration-1");
    let queued = value.clone();
    let mut competing = accepted.clone();
    competing.request_key = "migration-2".into();
    assert!(value
        .request_custom_runtime_migration(competing, &image('b'), NOW.into())
        .is_err());
    let mut replaced = accepted;
    replaced.target_image = image('c');
    assert!(value
        .request_custom_runtime_migration(replaced, &image('c'), NOW.into())
        .is_err());
    assert_eq!(value, queued);
}

#[test]
fn custom_runtime_never_enters_managed_daily_owner_or_unadmitted_rotation_paths() {
    let mut value = custom();
    let original = value.clone();
    assert!(!value.supports_retained_runtime_update());
    assert!(!value.check_daily_runtime_update(&image('b'), NOW).unwrap());
    assert!(value
        .request_runtime_update(image('b'), false, NOW.into())
        .is_err());
    assert!(value
        .request_runtime_update(image('b'), true, NOW.into())
        .is_err());
    let identity = SandboxRotationIdentity::capture(&value, &[]).unwrap();
    assert!(value
        .begin_image_rotation(image('b'), identity, NOW.into())
        .is_err());
    assert_eq!(value, original);
    queue(&mut value, 'b', "migration-1");
    let queued = value.clone();
    assert!(value
        .request_runtime_update(image('b'), false, NOW.into())
        .is_err());
    assert_eq!(value, queued);
}

#[test]
fn custom_runtime_fails_closed_on_malformed_retained_authority() {
    let mut queued = custom();
    queue(&mut queued, 'b', "migration-1");
    for case in 0..8 {
        let mut value = queued.clone();
        let state = value.lifecycle.custom_runtime.as_mut().unwrap();
        match case {
            0 => state.effective_image = "".into(),
            1 => state.migration.origin.profile_image = image('f'),
            2 => state.migration.origin.home.receipt.native_authority = None,
            3 => state.source_generation = 0,
            4 => state.completed_at = Some("invalid".into()),
            5 => value.runtime.as_mut().unwrap().runtime_version = image('b'),
            6 => {
                value
                    .lifecycle
                    .runtime_update
                    .as_mut()
                    .unwrap()
                    .request
                    .as_mut()
                    .unwrap()
                    .automatic = true
            }
            7 => {
                value.runtime.as_mut().unwrap().runtime_id = "agent-sandbox:other:other-uid".into()
            }
            _ => unreachable!(),
        }
        assert!(value.validate().is_err(), "case {case}");
        assert!(
            value.custom_runtime_image_for_binding().is_err(),
            "case {case}"
        );
    }
    let mut encoded = serde_json::to_value(&queued).unwrap();
    encoded["lifecycle"]["custom_runtime"]["unrecognized_authority"] = true.into();
    assert!(serde_json::from_value::<Sandbox>(encoded).is_err());
}

#[test]
fn custom_runtime_requires_idle_work_and_eligible_lifecycle() {
    for case in 0..6 {
        let mut value = custom();
        let command = request(&value, 'b', "migration-1");
        match case {
            0 => {
                value.begin_pause(NOW.into(), None).unwrap();
            }
            1 => {
                value
                    .work
                    .schedule(SandboxWorkStep::MintRotatedCredential, NOW.into());
                value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
            }
            2 => {
                value
                    .work
                    .schedule(SandboxWorkStep::ProvisionRuntime, NOW.into());
            }
            3 => {
                value.status = SandboxStatus::Failed;
                value.failure = Some(TypedFailure {
                    class: "runtime_lost".into(),
                    detail: "fixture".into(),
                });
            }
            4 => {
                value.status = SandboxStatus::Retiring;
            }
            5 => {
                value.status = SandboxStatus::Retired;
                value.retired_at = Some(NOW.into());
                value.runtime = None;
                let home = value.lifecycle.home.as_mut().unwrap();
                home.released = true;
                let native = home.native_pool.as_mut().unwrap();
                native.cleanup = Some(layer3_adapter::agent_sandbox::NativePoolCleanup {
                    request: native.request.clone(),
                    claim: native.claim.clone(),
                    members: vec![layer3_adapter::agent_sandbox::NativePoolMember {
                        name: "member".into(),
                        uid: "sandbox-uid".into(),
                        pvc_uid: Some("pvc-uid".into()),
                    }],
                });
                native.cleaned = true;
            }
            _ => unreachable!(),
        }
        value.validate().unwrap();
        let before = value.clone();
        assert!(
            value
                .request_custom_runtime_migration(command, &image('b'), NOW.into())
                .is_err(),
            "case {case}"
        );
        assert_eq!(value, before);
    }
}

#[test]
fn custom_runtime_legacy_profile_fallback_remains_readable() {
    let value = custom();
    let encoded = serde_json::to_value(&value).unwrap();
    assert!(encoded["lifecycle"].get("custom_runtime").is_none());
    let decoded: Sandbox = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        decoded.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(
        decoded.custom_runtime_image_for_binding().unwrap(),
        Some(image('a').as_str())
    );
}
