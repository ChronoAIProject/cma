use super::*;

fn held(budget: &Arc<Semaphore>, bytes: usize) -> crate::Result<HeldOutput> {
    let storage = reserve(Some(budget), bytes, None)?;
    Ok(HeldOutput {
        cursor: 1,
        payload: vec![0; bytes],
        prepared: None,
        decoded: None,
        sign_in: None,
        _storage: storage,
    })
}

#[test]
fn attachments_share_capacity_and_popped_values_keep_their_reservations() {
    let weight = NODE_BYTES + 123;
    let budget = Arc::new(Semaphore::new(weight * 3));
    let mut first = OutputQueue::new(true);
    let mut second = OutputQueue::new(true);
    first.push_back(held(&budget, 123).unwrap());
    second.push_back(held(&budget, 123).unwrap());
    second.push_back(held(&budget, 123).unwrap());
    assert_eq!(budget.available_permits(), 0);
    assert!(matches!(
        held(&budget, 123),
        Err(AdapterError::AtCapacity(_))
    ));
    let in_flight = first.pop_front().unwrap();
    drop(first);
    assert_eq!(budget.available_permits(), 0);
    drop(in_flight);
    assert_eq!(budget.available_permits(), weight);
    second.push_back(held(&budget, 123).unwrap());
    drop(second);
    assert_eq!(budget.available_permits(), weight * 3);
}

#[test]
fn empty_payloads_still_charge_queue_nodes_and_drain_to_baseline() {
    let budget = Arc::new(Semaphore::new(NODE_BYTES * 2));
    let mut queue = OutputQueue::new(true);
    for _ in 0..2 {
        queue.push_back(held(&budget, 0).unwrap());
    }
    assert!(matches!(held(&budget, 0), Err(AdapterError::AtCapacity(_))));
    drop(queue.pop_front());
    assert_eq!(budget.available_permits(), NODE_BYTES);
    drop(queue.pop_front());
    assert_eq!(budget.available_permits(), NODE_BYTES * 2);
    assert!(queue.pop_front().is_none());
}

#[test]
fn disabled_policy_uses_no_reservation_and_arithmetic_overflow_refuses() {
    assert!(process_budget(false).is_none());
    assert!(reserve(None, usize::MAX, None).unwrap().is_none());
    let budget = Arc::new(Semaphore::new(1024));
    assert!(matches!(
        reserve(Some(&budget), usize::MAX, None),
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(budget.available_permits(), 1024);
    assert!(Arc::ptr_eq(
        &process_budget(true).unwrap(),
        &process_budget(true).unwrap()
    ));
}

#[test]
fn one_maximal_retained_payload_keeps_its_full_storage_until_drop() {
    let bytes = crate::hosted::acp::MAX_FRAME_BYTES;
    let budget = Arc::new(Semaphore::new(bytes + NODE_BYTES));
    let value = held(&budget, bytes).unwrap();
    assert_eq!(value.payload.capacity(), bytes);
    assert_eq!(budget.available_permits(), 0);
    drop(value);
    assert_eq!(budget.available_permits(), bytes + NODE_BYTES);
}
