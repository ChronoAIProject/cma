use super::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use super::store::{
    SandboxEventStore, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore, SandboxTransaction,
    SandboxTransactionStore,
};
use super::*;

const HOLDER: &str = "generation-worker";
const LATER: &str = "2026-09-05T00:31:00Z";
const LEASE_END: &str = "2026-09-05T01:00:00Z";

#[test]
fn durable_reservation_reuses_exact_generation_after_retry_and_serialization() {
    let mut value = provisioning();
    claim(&mut value);
    let revision = value.revision;
    assert_eq!(reserve(&mut value, 101), 101);
    assert_eq!(value.revision, revision + 1);
    assert!(value.work.lease.is_none());
    assert_eq!(value.lifecycle.runtime_generation_high_water, 101);
    let serialized = serde_json::to_value(&value).unwrap();
    let mut resumed: Sandbox = serde_json::from_value(serialized).unwrap();
    claim(&mut resumed);
    let unchanged = resumed.clone();
    assert_eq!(reserve(&mut resumed, 999), 101);
    assert_eq!(resumed, unchanged);
    resumed
        .retry_lifecycle(LATER.into(), HOLDER, fence(&resumed), LATER.into())
        .unwrap();
    claim(&mut resumed);
    assert_eq!(reserve(&mut resumed, 1), 101);
    assert_eq!(resumed.lifecycle.runtime_generation_high_water, 101);
}

#[test]
fn replacement_binding_requires_exact_durable_reservation_and_consumes_it() {
    let mut value = provisioning();
    claim(&mut value);
    let unreserved = value.clone();
    assert!(value
        .bind_lifecycle_runtime(binding(Some(101)), HOLDER, fence(&value), LATER.into())
        .is_err());
    assert_eq!(value, unreserved);
    assert_eq!(reserve(&mut value, 101), 101);
    claim(&mut value);
    let reserved = value.clone();
    for generation in [None, Some(0), Some(100), Some(102)] {
        assert!(value
            .bind_lifecycle_runtime(binding(generation), HOLDER, fence(&value), LATER.into())
            .is_err());
        assert_eq!(value, reserved);
    }
    value
        .bind_lifecycle_runtime(binding(Some(101)), HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_eq!(value.runtime.unwrap().provision_generation, Some(101));
    assert!(value.lifecycle.runtime_provision.is_none());
    assert_eq!(value.lifecycle.runtime_generation_high_water, 101);
}

#[test]
fn high_water_outlives_lost_runtime_and_absent_high_generation_agents() {
    let mut value = sandbox();
    let runtime = value.runtime.clone().unwrap();
    value
        .record_adopted_runtime_generation(&runtime, 101, LATER.into())
        .unwrap();
    value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
    value
        .recover_runtime_loss(&[], false, LATER.into())
        .unwrap();
    assert!(value.runtime.is_none());
    assert_eq!(value.lifecycle.runtime_generation_high_water, 101);
    claim(&mut value);
    let checkpoint = value.lifecycle.checkpoints.latest.clone().unwrap();
    value
        .complete_lifecycle_checkpoint_verification(
            &checkpoint,
            HOLDER,
            fence(&value),
            LATER.into(),
        )
        .unwrap();
    claim(&mut value);
    assert_eq!(reserve(&mut value, 2), 102);
}

#[test]
fn cancelled_reservations_burn_generation_without_leaking_authority_to_new_operation() {
    let mut value = provisioning();
    claim(&mut value);
    assert_eq!(reserve(&mut value, 101), 101);
    let reservation = value.lifecycle.runtime_provision.clone().unwrap();
    let mut failed = value.clone();
    claim(&mut failed);
    failed
        .fail_lifecycle(
            "provider failed".into(),
            HOLDER,
            fence(&failed),
            LATER.into(),
        )
        .unwrap();
    assert!(failed.lifecycle.runtime_provision.is_none());
    assert_eq!(failed.lifecycle.runtime_generation_high_water, 101);
    value.begin_retirement(LATER.into()).unwrap();
    assert!(value.lifecycle.runtime_provision.is_none());
    assert_eq!(value.lifecycle.runtime_generation_high_water, 101);
    let mut next_operation = provisioning();
    next_operation.lifecycle.runtime_generation_high_water =
        value.lifecycle.runtime_generation_high_water;
    claim(&mut next_operation);
    assert_eq!(reserve(&mut next_operation, 1), 102);
    let mut wrong_authority = next_operation.clone();
    wrong_authority
        .lifecycle
        .runtime_provision
        .as_mut()
        .unwrap()
        .operation_id = "another-operation".into();
    assert!(wrong_authority.validate().is_err());
    next_operation.lifecycle.runtime_provision = Some(reservation);
    assert!(next_operation.validate().is_err());
}

#[test]
fn runtime_generation_adoption_is_exact_and_legacy_unknown_stays_unknown_until_observed() {
    let mut value = sandbox();
    let mut serialized = serde_json::to_value(&value).unwrap();
    assert!(serialized["runtime"].get("provision_generation").is_none());
    serialized["lifecycle"]
        .as_object_mut()
        .unwrap()
        .remove("runtime_generation_high_water");
    let legacy = serde_json::from_value::<Sandbox>(serialized).unwrap();
    assert_eq!(legacy.runtime.unwrap().provision_generation, None);
    assert_eq!(legacy.lifecycle.runtime_generation_high_water, 0);
    assert!(legacy.lifecycle.runtime_provision.is_none());
    let runtime = value.runtime.clone().unwrap();
    let original = value.clone();
    let mut wrong_runtime = runtime.clone();
    wrong_runtime.runtime_id = "another-runtime".into();
    assert!(value
        .record_adopted_runtime_generation(&wrong_runtime, 41, LATER.into())
        .is_err());
    assert!(value
        .record_adopted_runtime_generation(&runtime, 0, LATER.into())
        .is_err());
    assert_eq!(value, original);
    value
        .record_adopted_runtime_generation(&runtime, 41, LATER.into())
        .unwrap();
    assert_eq!(value.revision, original.revision + 1);
    assert_eq!(value.lifecycle.runtime_generation_high_water, 41);
    let known = value.runtime.clone().unwrap();
    let adopted = value.clone();
    value
        .record_adopted_runtime_generation(&known, 41, LATER.into())
        .unwrap();
    assert_eq!(value, adopted);
    assert!(value
        .record_adopted_runtime_generation(&known, 42, LATER.into())
        .is_err());
    assert_eq!(value, adopted);
}

#[test]
fn invalid_authority_and_exhausted_generation_are_atomic() {
    let mut value = provisioning();
    value.lifecycle.runtime_generation_high_water = u64::MAX;
    claim(&mut value);
    let original = value.clone();
    assert!(value
        .reserve_lifecycle_runtime_generation(1, HOLDER, fence(&value), LATER.into())
        .is_err());
    assert_eq!(value, original);
    value.lifecycle.runtime_generation_high_water = 4;
    let original = value.clone();
    assert!(value
        .reserve_lifecycle_runtime_generation(5, HOLDER, fence(&value) + 1, LATER.into())
        .is_err());
    assert_eq!(value, original);
}

#[tokio::test]
async fn store_reservation_is_guarded_exact_and_atomic_with_its_event() {
    let store = InMemorySandboxStore::default();
    let mut current = provisioning();
    current.revision = 1;
    claim(&mut current);
    store.create_sandbox(current.clone()).await.unwrap();
    let mut next = current.clone();
    assert_eq!(reserve(&mut next, 101), 101);
    let mutation = reservation_mutation(&current, next.clone());
    let mut forged = mutation.clone();
    forged.sandbox.credential.delivered = false;
    assert!(store.commit_sandbox_lifecycle(forged).await.is_err());
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert!(store
        .sandbox_events(&current.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    store
        .commit_sandbox_lifecycle(mutation.clone())
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(next));
    assert_eq!(
        store
            .sandbox_events(&current.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store.commit_sandbox_lifecycle(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
}

#[tokio::test]
async fn reservation_must_fence_above_current_live_agent_attachment_authority() {
    let store = InMemorySandboxStore::default();
    let mut current = provisioning();
    current.revision = 1;
    claim(&mut current);
    store.create_sandbox(current.clone()).await.unwrap();
    let mut parked = agent("agt_high_generation", "conversation-high", 0);
    parked.status = AgentStatus::Paused;
    parked.revision = 1;
    parked.conversation.runtime_generation = Some(100);
    super::store::AgentStore::create_agent(&store, parked)
        .await
        .unwrap();
    let mut stale = current.clone();
    reserve(&mut stale, 2);
    assert_eq!(
        store
            .commit_sandbox_lifecycle(reservation_mutation(&current, stale))
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    let mut fresh = current.clone();
    reserve(&mut fresh, 101);
    store
        .commit_sandbox_lifecycle(reservation_mutation(&current, fresh))
        .await
        .unwrap();
}

#[tokio::test]
async fn generic_store_writes_cannot_lower_high_water_or_forget_a_bound_generation() {
    let store = InMemorySandboxStore::default();
    let mut current = sandbox();
    let runtime = current.runtime.clone().unwrap();
    current
        .record_adopted_runtime_generation(&runtime, 41, LATER.into())
        .unwrap();
    current.revision = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    for change in 0..4 {
        let mut next = current.clone();
        next.revision += 1;
        match change {
            0 => next.lifecycle.runtime_generation_high_water = 1,
            1 => next.runtime.as_mut().unwrap().provision_generation = None,
            _ => {
                next.runtime = Some(binding(if change == 2 { Some(1) } else { None }));
            }
        }
        assert_eq!(
            store.save_sandbox(1, next.clone()).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store
                .commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: None,
                    expected_revision: 1,
                    sandbox: Box::new(next),
                    event_at: LATER.into(),
                    event: SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::ProvisionRuntime
                    },
                })
                .await,
            Err(SandboxStoreError::Conflict)
        );
    }
    assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(current));
}

fn provisioning() -> Sandbox {
    let mut value = sandbox();
    value.status = SandboxStatus::Paused;
    value.runtime = None;
    value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
    value
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .unwrap();
    claim(&mut value);
    let checkpoint = value.lifecycle.checkpoints.latest.clone().unwrap();
    value
        .complete_lifecycle_checkpoint_verification(
            &checkpoint,
            HOLDER,
            fence(&value),
            LATER.into(),
        )
        .unwrap();
    value
}

fn reserve(value: &mut Sandbox, minimum: u64) -> u64 {
    value
        .reserve_lifecycle_runtime_generation(minimum, HOLDER, fence(value), LATER.into())
        .unwrap()
}

fn claim(value: &mut Sandbox) {
    value
        .work
        .claim(HOLDER.into(), LATER, LEASE_END.into())
        .unwrap();
}

fn fence(value: &Sandbox) -> u64 {
    value.work.lease.as_ref().unwrap().fence
}

fn binding(generation: Option<u64>) -> RuntimeBinding {
    RuntimeBinding {
        boot_id: None,
        runtime_id: "replacement-runtime".into(),
        runtime_version: "image-v1".into(),
        provision_generation: generation,
        bound_at: LATER.into(),
    }
}

fn reservation_mutation(current: &Sandbox, sandbox: Sandbox) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: LATER.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}
