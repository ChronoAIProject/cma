use super::*;
use crate::allocation_probe::AllocationProbe;

const POOL_BYTES: usize = 4 * 1024 * 1024;

fn budget() -> Budget {
    // Initialize shared metadata outside the measured operation.
    drop(Budget::process());
    Budget {
        component: Arc::new(Semaphore::new(POOL_BYTES)),
        aggregate: IngressBudget::new(POOL_BYTES),
    }
}

fn exhaust(budget: &Budget, aggregate: bool) -> IngressReservation {
    if aggregate {
        IngressReservation::try_acquire(
            &Arc::new(Semaphore::new(POOL_BYTES)),
            Some(&budget.aggregate),
            budget.aggregate.available_bytes(),
        )
        .unwrap()
    } else {
        IngressReservation::try_acquire(
            &budget.component,
            None,
            budget.component.available_permits(),
        )
        .unwrap()
    }
}

fn input_refusal_allocations(oversize: bool, aggregate: bool) -> usize {
    let budget = budget();
    let input = if oversize {
        ConversationInput::Prompt(vec![ContentBlock::Text(
            agent_client_protocol_schema::v1::TextContent::new("x".repeat(INPUT_BYTES + 1)),
        )])
    } else {
        ConversationInput::RecoverablePrompt {
            content: vec![],
            prompt_id: crate::PromptIdentity::new("conversation", 1, 1),
        }
    };
    drop(ResourceInput::admit_with(input.clone(), budget.clone()));
    let held = exhaust(&budget, aggregate);
    let before = (
        budget.component.available_permits(),
        budget.aggregate.available_bytes(),
    );
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| ResourceInput::admit_with(input, budget.clone()));
    let allocated = probe.allocated();
    assert!(matches!(result, Err(ProtocolError::Capacity)));
    assert_eq!(probe.live(), 0);
    assert_eq!(
        before,
        (
            budget.component.available_permits(),
            budget.aggregate.available_bytes()
        )
    );
    drop(held);
    assert_eq!(budget.component.available_permits(), POOL_BYTES);
    assert_eq!(budget.aggregate.available_bytes(), POOL_BYTES);
    allocated
}

#[test]
fn recovery_input_refuses_without_allocating_when_work_budget_is_exhausted() {
    let allocations = [
        input_refusal_allocations(false, false),
        input_refusal_allocations(false, true),
    ];
    assert_eq!(allocations, [0, 0], "component and aggregate refusal");
}

#[test]
fn oversize_input_refuses_without_allocating_when_work_budget_is_exhausted() {
    let allocations = [
        input_refusal_allocations(true, false),
        input_refusal_allocations(true, true),
    ];
    assert_eq!(allocations, [0, 0], "component and aggregate refusal");
}

fn bind_refusal_allocations(invalid: bool, aggregate: bool) -> usize {
    let budget = budget();
    let input =
        ResourceInput::admit_with(ConversationInput::Prompt(vec![]), budget.clone()).unwrap();
    let mut ticket = super::super::tests::ticket(&input);
    if invalid {
        ticket.execution.receiver.kind =
            crate::resource_execution_protocol::ReceiverKind::ReleaseProducer;
    }
    // Keep caller-owned objects alive separately from the measured bind operation.
    let retained = input.clone();
    drop(input.clone().bind(ticket.clone()));
    let held = exhaust(&budget, aggregate);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| input.bind(ticket));
    let allocated = probe.allocated();
    assert!(result.is_err());
    assert_eq!(probe.live(), 0);
    drop(held);
    drop(retained);
    assert_eq!(budget.component.available_permits(), POOL_BYTES);
    assert_eq!(budget.aggregate.available_bytes(), POOL_BYTES);
    allocated
}

#[test]
fn valid_ticket_refuses_without_allocating_when_work_budget_is_exhausted() {
    let allocations = [
        bind_refusal_allocations(false, false),
        bind_refusal_allocations(false, true),
    ];
    assert_eq!(allocations, [0, 0], "component and aggregate refusal");
}

#[test]
fn invalid_ticket_refuses_without_allocating_when_work_budget_is_exhausted() {
    let allocations = [
        bind_refusal_allocations(true, false),
        bind_refusal_allocations(true, true),
    ];
    assert_eq!(allocations, [0, 0], "component and aggregate refusal");
}
