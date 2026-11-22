use super::{worker::*, *};
use std::time::Duration;

fn binding() -> JournalBinding {
    JournalBinding {
        deletion_id: AccountDeletionId::generate(),
        contract: JournalContract {
            version: 1,
            worker: "a".repeat(64).try_into().unwrap(),
            catalogue: "b".repeat(64).try_into().unwrap(),
            policy: "c".repeat(64).try_into().unwrap(),
        },
        sequence: 0,
        inventory_generation: 0,
        step: ErasureStep::Inventory,
    }
}
fn page(count: usize) -> JournalCommand {
    JournalCommand::Prepare {
        page: PlanPage {
            targets: (0..count)
                .map(|i| TargetReference {
                    class: TargetClass::Delegation,
                    namespace: "synthetic".into(),
                    identity: format!("target-{i}"),
                    incarnation: "original".into(),
                })
                .collect(),
            cursor: Some("page-1".into()),
        },
    }
}

#[test]
fn operation_identity_is_stable_and_binds_every_command_field() {
    let binding = binding();
    let first = JournalRequest::new(binding.clone(), page(2)).unwrap();
    assert_eq!(
        first,
        JournalRequest::new(binding.clone(), page(2)).unwrap()
    );
    let encoded = serde_json::to_vec(&first).unwrap();
    let decoded: JournalRequest = serde_json::from_slice(&encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(first, decoded);
    for b in [
        JournalBinding {
            sequence: 1,
            ..binding.clone()
        },
        JournalBinding {
            inventory_generation: 1,
            ..binding.clone()
        },
        JournalBinding {
            step: ErasureStep::ReleaseHomes,
            ..binding.clone()
        },
        JournalBinding {
            deletion_id: AccountDeletionId::generate(),
            ..binding.clone()
        },
    ] {
        assert_ne!(first.key(), JournalRequest::new(b, page(2)).unwrap().key());
    }
    assert_ne!(
        first.key(),
        JournalRequest::new(binding, page(3)).unwrap().key()
    );
    assert!(!format!("{first:?}").contains("target-"));
}

#[test]
fn persisted_same_key_cannot_substitute_payload_or_contract() {
    let request = JournalRequest::new(binding(), page(1)).unwrap();
    for pointer in [
        "/command/page/targets/0/identity",
        "/binding/contract/policy",
    ] {
        let mut altered = serde_json::to_value(&request).unwrap();
        *altered.pointer_mut(pointer).unwrap() = if pointer.ends_with("policy") {
            "d".repeat(64).into()
        } else {
            "replacement".into()
        };
        let altered: JournalRequest = serde_json::from_value(altered).unwrap();
        assert_eq!(altered.validate(), Err(JournalError::BindingMismatch));
    }
}

#[test]
fn plan_capacity_checks_items_fields_and_encoded_bytes() {
    JournalRequest::new(binding(), page(MAX_PLAN_TARGETS)).unwrap();
    assert_eq!(
        JournalRequest::new(binding(), page(MAX_PLAN_TARGETS + 1)),
        Err(JournalError::Capacity)
    );
    let JournalCommand::Prepare { mut page } = page(1) else {
        unreachable!()
    };
    page.targets.push(page.targets[0].clone());
    assert_eq!(
        JournalRequest::new(binding(), JournalCommand::Prepare { page }),
        Err(JournalError::Invalid)
    );
    let JournalCommand::Prepare { mut page } = self::page(MAX_PLAN_TARGETS) else {
        unreachable!()
    };
    for (i, target) in page.targets.iter_mut().enumerate() {
        target.identity = format!("{i:03}{}", "\\".repeat(509));
        target.incarnation = "\"".repeat(512);
        target.namespace = "\"".repeat(64);
    }
    assert_eq!(
        JournalRequest::new(binding(), JournalCommand::Prepare { page }),
        Err(JournalError::Capacity)
    );
    let JournalCommand::Prepare { mut page } = self::page(1) else {
        unreachable!()
    };
    page.cursor = Some("x".repeat(4097));
    assert_eq!(
        JournalRequest::new(binding(), JournalCommand::Prepare { page }),
        Err(JournalError::Capacity)
    );
}

#[test]
fn unsupported_versions_overflow_and_invalid_leases_refuse() {
    let mut b = binding();
    b.contract.version = 2;
    assert_eq!(JournalRequest::new(b, page(0)), Err(JournalError::Invalid));
    let mut b = binding();
    b.sequence = u64::MAX;
    assert_eq!(JournalRequest::new(b, page(0)), Err(JournalError::Invalid));
    for lease_seconds in [0, 301, u64::MAX] {
        assert_eq!(
            JournalRequest::new(binding(), JournalCommand::Renew { lease_seconds }),
            Err(JournalError::Invalid)
        );
    }
}

#[test]
fn cancellation_is_shared_and_budget_cannot_be_unbounded() {
    assert!(JournalBudget::new(Duration::ZERO).is_err());
    assert!(JournalBudget::new(Duration::from_secs(31)).is_err());
    let budget = JournalBudget::new(Duration::from_secs(5)).unwrap();
    let cloned = budget.clone();
    budget.cancel();
    assert_eq!(cloned.remaining(), Err(JournalError::Cancelled));
}
