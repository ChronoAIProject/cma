use super::*;

fn option() -> PermissionOption {
    PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce)
}

fn record(state: &mut PermissionState, id: u64) -> Result<Arc<Entry>> {
    state.remember(
        &format!("{id:064x}"),
        &serde_json::json!(id),
        &[option()],
        false,
    )
}

#[test]
fn full_table_keeps_existing_answers_and_refuses_new_records() {
    let budget = Arc::new(Semaphore::new(PROCESS_BYTES));
    let mut state = PermissionState::with_budget(budget.clone(), 2).unwrap();
    let first = record(&mut state, 1).unwrap();
    first.select(PermissionDecision::Allow);
    record(&mut state, 2).unwrap();
    let remaining = budget.available_permits();
    assert!(matches!(
        record(&mut state, 3),
        Err(AdapterError::Unavailable(_))
    ));
    assert_eq!(budget.available_permits(), remaining);
    let replay = record(&mut state, 1).unwrap();
    replay.select(PermissionDecision::Deny);
    assert!(Arc::ptr_eq(&first, &replay));
    assert_eq!(
        replay.answer().unwrap()["result"]["outcome"]["optionId"],
        "allow"
    );
}

#[test]
fn final_inflight_owner_retains_storage_after_table_drop() {
    let budget = Arc::new(Semaphore::new(PROCESS_BYTES));
    let mut state = PermissionState::with_budget(budget.clone(), 2).unwrap();
    let held = record(&mut state, 1).unwrap();
    let replay = held.clone();
    drop(state);
    assert!(budget.available_permits() < PROCESS_BYTES);
    let remaining = budget.available_permits();
    drop(held);
    assert_eq!(budget.available_permits(), remaining);
    drop(replay);
    assert_eq!(budget.available_permits(), PROCESS_BYTES);
}

#[test]
fn simultaneous_attachments_share_capacity_and_recover_after_drop() {
    let budget = Arc::new(Semaphore::new(4096));
    let mut first = PermissionState::with_budget(budget.clone(), 4).unwrap();
    let mut second = PermissionState::with_budget(budget.clone(), 4).unwrap();
    let held = record(&mut first, 1).unwrap();
    let spare = budget
        .clone()
        .try_acquire_many_owned(budget.available_permits() as u32)
        .unwrap();
    assert!(matches!(
        record(&mut second, 2),
        Err(AdapterError::Unavailable(_))
    ));
    assert!(Arc::ptr_eq(&held, &record(&mut first, 1).unwrap()));
    drop(first);
    let freed_table = budget
        .clone()
        .try_acquire_many_owned(budget.available_permits() as u32)
        .unwrap();
    assert!(
        record(&mut second, 2).is_err(),
        "in-flight owner still holds entry capacity"
    );
    drop(held);
    record(&mut second, 2).unwrap();
    drop(second);
    drop(spare);
    drop(freed_table);
    assert_eq!(budget.available_permits(), 4096);
}

#[test]
fn oversized_shapes_fail_without_spending_retention_capacity() {
    let budget = Arc::new(Semaphore::new(PROCESS_BYTES));
    let mut state = PermissionState::with_budget(budget.clone(), 4).unwrap();
    let initial = budget.available_permits();
    let id = "a".repeat(64);
    for choices in [
        vec![option(); MAX_OPTIONS + 1],
        vec![PermissionOption::new(
            "x".repeat(MAX_OPTION_BYTES + 1),
            "name",
            PermissionOptionKind::AllowOnce,
        )],
    ] {
        assert!(matches!(
            state.remember(&id, &serde_json::json!(1), &choices, false),
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(budget.available_permits(), initial);
    }
    let choices = vec![
        PermissionOption::new(
            "x".repeat(MAX_OPTION_BYTES),
            "name",
            PermissionOptionKind::AllowOnce
        );
        MAX_OPTIONS
    ];
    state
        .remember(&id, &serde_json::json!(1), &choices, false)
        .unwrap();
    assert!(budget.available_permits() < initial);
}

#[test]
fn names_and_metadata_are_not_retained_by_compact_permissions() {
    let plain = option();
    let mut rich = serde_json::to_value(&plain).unwrap();
    rich["name"] = serde_json::json!("description".repeat(10000));
    rich["_meta"] = serde_json::json!({"large":"metadata".repeat(10000)});
    let rich: PermissionOption = serde_json::from_value(rich).unwrap();
    let budget = Arc::new(Semaphore::new(PROCESS_BYTES));
    let mut state = PermissionState::with_budget(budget.clone(), 2).unwrap();
    let initial = budget.available_permits();
    let first = state
        .remember(
            &format!("{:064x}", 1),
            &serde_json::json!(1),
            &[plain],
            false,
        )
        .unwrap();
    let first_bytes = initial - budget.available_permits();
    let next = budget.available_permits();
    let second = state
        .remember(
            &format!("{:064x}", 2),
            &serde_json::json!(2),
            &[rich],
            false,
        )
        .unwrap();
    assert_eq!(first_bytes, next - budget.available_permits());
    assert!(first.answer().is_none());
    second.select(PermissionDecision::Abort);
    second.select(PermissionDecision::Allow);
    assert_eq!(
        second.answer().unwrap()["result"]["outcome"]["outcome"],
        "cancelled"
    );
}

#[test]
fn turn_scope_preserves_uncertain_retries_without_reusing_prior_permission() {
    let budget = Arc::new(Semaphore::new(PROCESS_BYTES));
    let mut state = PermissionState::with_budget(budget.clone(), 4).unwrap();
    state.begin_prompt("prompt-a");
    let first = record(&mut state, 1).unwrap();
    first.select(PermissionDecision::Allow);
    state.begin_prompt("prompt-a");
    assert!(Arc::ptr_eq(&first, &record(&mut state, 1).unwrap()));
    state.complete_prompt("foreign-prompt");
    assert!(Arc::ptr_eq(&first, &record(&mut state, 1).unwrap()));
    let held = budget.available_permits();
    state.complete_prompt("prompt-a");
    state.begin_prompt("prompt-b");
    assert_eq!(
        budget.available_permits(),
        held,
        "scope change never evicts uncertain storage"
    );
    let second = record(&mut state, 1).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.answer().is_none());
    second.select(PermissionDecision::Deny);
    assert_eq!(
        second.answer().unwrap()["result"]["outcome"]["outcome"],
        "cancelled"
    );
    assert_eq!(
        first.answer().unwrap()["result"]["outcome"]["optionId"],
        "allow"
    );
}
