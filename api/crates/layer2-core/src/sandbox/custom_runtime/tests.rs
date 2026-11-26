use super::test_support::*;
use crate::sandbox::lifecycle_test_support::NOW;
use crate::sandbox::*;

#[test]
fn custom_runtime_retains_source_until_verified_publication_across_restart() {
    let mut value = custom();
    let original = value.clone();
    let legacy_record = serde_json::to_value(&original).unwrap();
    assert!(legacy_record["lifecycle"].get("custom_runtime").is_none());
    assert_eq!(
        serde_json::from_value::<Sandbox>(legacy_record).unwrap(),
        original
    );
    let command = queue(&mut value, 'b', "migration-1");
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(
        value.custom_runtime_image_for_binding().unwrap(),
        Some(image('a').as_str())
    );
    assert!(!value
        .request_custom_runtime_migration(command.clone(), &image('b'), NOW.into())
        .unwrap());
    admit(&mut value, 'b');
    suspend(&mut value);
    let pending_record = serde_json::to_value(&value).unwrap();
    value = serde_json::from_value(pending_record).unwrap();
    value.validate().unwrap();
    assert_eq!(
        value.custom_runtime_image_for_binding().unwrap(),
        Some(image('a').as_str())
    );
    provision(&mut value, 'b');
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(
        value.custom_runtime_image_for_binding().unwrap(),
        Some(image('b').as_str())
    );
    assert_eq!(value.profile, original.profile);
    assert_eq!(value.lifecycle.home, original.lifecycle.home);
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('b').as_str())
    );
    assert!(!value
        .request_custom_runtime_migration(command, &image('b'), NOW.into())
        .unwrap());
    assert_eq!(value.profile, original.profile);
    assert_eq!(value.lifecycle.home, original.lifecycle.home);
}

#[test]
fn custom_runtime_paused_migration_returns_to_pause_and_resumes_new_pin() {
    let mut value = custom();
    pause(&mut value);
    queue(&mut value, 'b', "migration-1");
    admit(&mut value, 'b');
    provision(&mut value, 'b');
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Pausing);
    suspend(&mut value);
    step(&mut value, |v, f| {
        v.publish_lifecycle_paused(HOLDER, f, NOW.into())
    });
    value
        .begin_revive_with_managed_runtime(
            SandboxReviveCause::ExplicitRequest,
            Some(&image('f')),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(value.admitted_runtime_update_target(), None);
    provision(&mut value, 'b');
    finish(&mut value);
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('b').as_str())
    );
    assert_eq!(value.runtime.as_ref().unwrap().runtime_version, image('b'));
}

#[test]
fn custom_runtime_second_migration_uses_new_effective_source_and_original_lineage() {
    let mut value = custom();
    let origin = SandboxCustomRuntimeOrigin::capture(&value).unwrap();
    queue(&mut value, 'b', "migration-1");
    admit(&mut value, 'b');
    suspend(&mut value);
    provision(&mut value, 'b');
    finish(&mut value);
    let second = queue(&mut value, 'c', "migration-2");
    assert_eq!(second.expected_image, image('b'));
    assert_eq!(second.origin, origin);
    admit(&mut value, 'c');
    suspend(&mut value);
    provision(&mut value, 'c');
    finish(&mut value);
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('c').as_str())
    );
    assert_eq!(SandboxCustomRuntimeOrigin::capture(&value).unwrap(), origin);
}

#[test]
fn custom_runtime_failed_phase_retries_exact_request_and_target() {
    let mut value = custom();
    let command = queue(&mut value, 'b', "migration-1");
    admit(&mut value, 'b');
    suspend(&mut value);
    let operation = value.lifecycle.operation.clone();
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    value
        .park_failed_runtime_update(
            TypedFailure {
                class: "runtime_update_failed".into(),
                detail: "fixture".into(),
            },
            HOLDER,
            fence,
            NOW.into(),
        )
        .unwrap();
    let parked = value.clone();
    assert!(value
        .request_custom_runtime_migration(command.clone(), &image('c'), NOW.into())
        .is_err());
    assert!(value
        .request_runtime_update(image('b'), false, NOW.into())
        .is_err());
    assert_eq!(value, parked);
    assert!(value
        .request_custom_runtime_migration(command, &image('b'), NOW.into())
        .unwrap());
    assert_eq!(value.lifecycle.operation, operation);
    provision(&mut value, 'b');
    finish(&mut value);
}

#[test]
fn custom_runtime_runtime_loss_keeps_exact_target_without_early_pin_publication() {
    let mut value = custom();
    queue(&mut value, 'b', "migration-1");
    admit(&mut value, 'b');
    value.recover_runtime_loss(&[], true, NOW.into()).unwrap();
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(
        value.admitted_runtime_update_target(),
        Some(image('b').as_str())
    );
    provision(&mut value, 'b');
    finish(&mut value);
    assert_eq!(
        value.effective_custom_runtime_image(),
        Some(image('b').as_str())
    );
}

#[test]
fn custom_runtime_pending_successor_remains_readable_for_supported_retirement() {
    for bound in [false, true] {
        let mut value = custom();
        queue(&mut value, 'b', "migration-1");
        admit(&mut value, 'b');
        if bound {
            suspend(&mut value);
            provision(&mut value, 'b');
        }
        let authority = value.lifecycle.custom_runtime.clone();
        value.begin_retirement(NOW.into()).unwrap();
        value.validate().unwrap();
        assert_eq!(value.lifecycle.custom_runtime, authority);
        assert_eq!(
            value.custom_runtime_image_for_binding().unwrap(),
            Some(image(if bound { 'b' } else { 'a' }).as_str())
        );
        assert_eq!(
            value.effective_custom_runtime_image(),
            Some(image('a').as_str())
        );
        assert!(value
            .request_custom_runtime_migration(
                request(&value, 'c', "migration-2"),
                &image('c'),
                NOW.into()
            )
            .is_err());
    }
}
