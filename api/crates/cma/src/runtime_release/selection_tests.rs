use super::*;
use crate::release_authority::ReleasePair;

fn target() -> (ReleaseTarget, PoolTarget) {
    let mut target = crate::warm_pools::tests::target();
    target.id = format!("wp_{}", "a".repeat(64));
    target.applied_revision = target.revision;
    target.document_schema_version = 3;
    target.binding = Some(layer3_adapter::agent_sandbox::NativePoolBinding {
        spec: target.plan.spec.clone(),
        pod_policy: target.plan.pod_policy.clone(),
        name: "pool".into(),
        uid: "pool-uid".into(),
        template_name: "template".into(),
        template_uid: "template-uid".into(),
        config_sha256: target.plan.config_sha256.clone(),
    });
    let selected = ReleaseTarget {
        image: target.plan.spec.image.uri.clone(),
        plan_sha256: target.plan.config_sha256.clone(),
        target_id: target.id.clone(),
        target_revision: target.revision,
        pool_name: "pool".into(),
        pool_uid: "pool-uid".into(),
        template_name: "template".into(),
        template_uid: "template-uid".into(),
        qualification_sha256: "b".repeat(64),
    };
    (selected, target)
}

#[test]
fn original_pool_requires_original_uid_revision_and_positive_ready_target() {
    let (selected, original) = target();
    assert!(target_matches(&selected, &original));
    for change in 0..6 {
        let mut target = original.clone();
        match change {
            0 => target.binding.as_mut().unwrap().uid = "replacement".into(),
            1 => target.binding.as_mut().unwrap().template_uid = "replacement".into(),
            2 => target.revision += 1,
            3 => target.retiring = true,
            4 => target.target_replicas = 0,
            _ => target.applied_revision = 0,
        }
        assert!(!target_matches(&selected, &target));
    }
}

#[test]
fn restart_foreign_config_frozen_head_and_mixed_image_pair_cannot_be_writer() {
    let (selected, _) = target();
    let process = ProcessIdentity {
        deployment: "test".into(),
        instance_id: uuid::Uuid::new_v4().to_string(),
        config_sha256: "a".repeat(64),
        binary_image: selected.image.clone(),
        executable_sha256: "a".repeat(64),
        agent_image: selected.image.clone(),
        machine_image: None,
    };
    let writer = WriterIdentity {
        deployment: process.deployment.clone(),
        instance_id: process.instance_id.clone(),
        config_sha256: process.config_sha256.clone(),
        epoch: 1,
        fence: 1,
    };
    let pair = ReleasePair {
        agent: selected,
        machine: None,
    };
    let mut fallback = pair.clone();
    fallback.agent.image = format!("registry.example/runtime@sha256:{}", "c".repeat(64));
    fallback.agent.target_id = format!("wp_{}", "c".repeat(64));
    let head = ReleaseHead {
        deployment: "test".into(),
        schema_version: 2,
        epoch: 1,
        control_revision: 1,
        admission_sequence: 0,
        active_effects: 0,
        phase: Phase::Active,
        pair: pair.clone(),
        writer: Some(writer.clone()),
        writer_fence: 1,
        operation_id: None,
        protected_pairs: vec![pair, fallback],
        committed_at: mongodb::bson::DateTime::from_millis(1),
    };
    assert_eq!(selected_writer(&head, &process).unwrap(), writer);
    for change in 0..3 {
        let mut changed = process.clone();
        match change {
            0 => changed.instance_id = uuid::Uuid::new_v4().to_string(),
            1 => changed.config_sha256 = "f".repeat(64),
            _ => changed.machine_image = Some(changed.agent_image.clone()),
        }
        assert!(selected_writer(&head, &changed).is_err());
    }
    let mut legacy = head.clone();
    legacy.schema_version = 1;
    assert!(selected_writer(&legacy, &process).is_err());
    let mut frozen = head;
    frozen.phase = Phase::Freezing;
    frozen.operation_id = Some("transition".into());
    assert!(selected_writer(&frozen, &process).is_err());
}
