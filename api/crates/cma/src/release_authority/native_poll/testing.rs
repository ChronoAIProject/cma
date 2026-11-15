//! Test-only opaque values for actual caller state/cancellation tests. These
//! model database receipts, not qualified runtime or provider finality evidence.
use mongodb::bson::DateTime;

use super::{
    super::{validation, ReleaseTarget, WriterIdentity},
    model::*,
};
use crate::warm_pools::PoolTarget;

pub(crate) fn fixture(
    lane: NativePollLane,
    target: Option<PoolTarget>,
) -> (NativePollCommand, NativePollReceipt) {
    let writer = WriterIdentity {
        deployment: target
            .as_ref()
            .map_or("test-deployment", |t| t.plan.deployment.as_str())
            .into(),
        instance_id: "test-instance".into(),
        config_sha256: "a".repeat(64),
        epoch: 1,
        fence: 1,
    };
    let selected_target = match (lane, target.as_ref()) {
        (NativePollLane::Background, _) => None,
        (_, Some(target)) => {
            let binding = target
                .binding
                .as_ref()
                .expect("startup fixture needs original binding");
            Some(ReleaseTarget {
                image: target.plan.spec.image.uri.clone(),
                plan_sha256: target.plan.config_sha256.clone(),
                target_id: target.id.clone(),
                target_revision: target.revision,
                pool_name: binding.name.clone(),
                pool_uid: binding.uid.clone(),
                template_name: binding.template_name.clone(),
                template_uid: binding.template_uid.clone(),
                qualification_sha256: "a".repeat(64),
            })
        }
        (NativePollLane::StartupAgent, None) => Some(super::super::tests::head().pair.agent),
        (NativePollLane::StartupMachine, None) => super::super::tests::head().pair.machine,
    };
    let command = CommandRecord {
        key: command_key(&writer, lane, 1).unwrap(),
        writer,
        lane,
        sequence: 1,
        previous_receipt_sha256: None,
        selected_target,
    };
    command.validate().unwrap();
    let outcome = match target {
        Some(mut target) => {
            target.lease = Some(lease(&command));
            target.lease_until = DateTime::from_millis(60_001);
            NativePollOutcome::Claimed(Box::new(target))
        }
        None => NativePollOutcome::Empty,
    };
    let mut receipt = ReceiptRecord {
        key: command.key.clone(),
        kind: "native_target_poll_receipt".into(),
        schema_version: 1,
        command: command.clone(),
        outcome,
        admission_sequence: 1,
        control_revision: 1,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt.digest().unwrap();
    receipt.validate().unwrap();
    (NativePollCommand(command), NativePollReceipt(receipt))
}

pub(super) fn target() -> PoolTarget {
    let mut target = crate::warm_pools::tests::target();
    target.id = format!("wp_{}", "a".repeat(64));
    target.plan.deployment = "test-deployment".into();
    target.document_schema_version = 3;
    target.lease = None;
    target.lease_until = DateTime::from_millis(0);
    // Also checks fixture serialization fits the production record bound.
    validation::hash(&target).unwrap();
    target
}

pub(super) fn bound_target() -> PoolTarget {
    let selected = super::super::tests::head().pair.agent;
    let mut target = target();
    target.id = selected.target_id;
    target.plan.spec.image.uri = selected.image;
    target.plan.config_sha256 = selected.plan_sha256;
    target.binding = Some(layer3_adapter::agent_sandbox::NativePoolBinding {
        spec: target.plan.spec.clone(),
        pod_policy: target.plan.pod_policy.clone(),
        config_sha256: target.plan.config_sha256.clone(),
        name: selected.pool_name,
        uid: selected.pool_uid,
        template_name: selected.template_name,
        template_uid: selected.template_uid,
    });
    target
}
