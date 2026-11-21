use super::*;
use crate::allocation_probe::{deallocation::arc_allocation_bytes, AllocationProbe};
use std::sync::Barrier;

const POOL_BYTES: usize = 1024 * 1024;

fn budget() -> Budget {
    Budget {
        component: Arc::new(Semaphore::new(POOL_BYTES)),
        aggregate: IngressBudget::new(POOL_BYTES),
    }
}

fn input(budget: &Budget) -> ResourceInput {
    ResourceInput::admit_with(
        ConversationInput::Prompt(vec![ContentBlock::Text(
            agent_client_protocol_schema::v1::TextContent::new("x".repeat(256)),
        )]),
        budget.clone(),
    )
    .unwrap()
}

fn dispatch(budget: &Budget) -> ResourceDispatch {
    let input = input(budget);
    let ticket = super::super::tests::ticket(&input);
    input.bind(ticket).unwrap()
}

fn dispatch_with_retained_input(budget: &Budget) -> (ResourceDispatch, ResourceInput) {
    let input = input(budget);
    let mut ticket = super::super::tests::ticket(&input);
    ticket.nonce = crate::resource_execution_protocol::OpaqueId::new(&"n".repeat(120)).unwrap();
    (input.clone().bind(ticket).unwrap(), input)
}

fn available(budget: &Budget) -> (usize, usize) {
    (
        budget.component.available_permits(),
        budget.aggregate.available_bytes(),
    )
}

fn drop_clones<T: Clone + Send + 'static>(owner: T, race: bool) {
    let clone = owner.clone();
    if race {
        let barrier = Arc::new(Barrier::new(2));
        let peer = barrier.clone();
        let child = std::thread::spawn(move || {
            peer.wait();
            drop(clone);
        });
        barrier.wait();
        drop(owner);
        child.join().unwrap();
    } else {
        drop(owner);
        drop(clone);
    }
}

fn check<T: Clone + Send + 'static>(
    initialize: impl Fn(&Budget) -> T,
    backing: impl Fn(&T) -> (usize, usize),
    race: bool,
) {
    let budget = budget();
    let sample = initialize(&budget);
    let (bytes, _) = backing(&sample);
    drop(sample);
    assert_eq!(available(&budget), (POOL_BYTES, POOL_BYTES));
    let mut probe = AllocationProbe::new();
    for _ in 0..if race { 16 } else { 1 } {
        let (owner, pause) = probe.pause_deallocation_containing(
            bytes,
            || initialize(&budget),
            |value| backing(value).1,
        );
        let clone = owner.clone();
        let held = available(&budget);
        assert!(held.0 < POOL_BYTES);
        assert_eq!(held.0, held.1);
        drop(owner);
        assert_eq!(available(&budget), held, "independent clone lost custody");

        // Consume only the uncharged slack; there are no spare operation owners.
        let pressure = budget.reserve(held.0).unwrap();
        let (during, reused) = pause.inspect_while_dropping(
            move || drop_clones(clone, race),
            || {
                let during = available(&budget);
                let reused = budget.reserve(1).is_ok();
                (during, reused)
            },
        );
        drop(pressure);
        assert_eq!(available(&budget), (POOL_BYTES, POOL_BYTES));
        assert_eq!(probe.live(), 0);
        assert_eq!(during, (0, 0), "permits returned before physical free");
        assert!(!reused, "a competing owner reused live storage's charge");
    }
}

fn isolated(name: &str) -> bool {
    let (_, module) = module_path!().split_once("::").unwrap();
    crate::allocation_probe::deallocation::isolated(&format!("{module}::{name}"))
}

fn input_owner(input: &ResourceInput) -> (usize, usize) {
    (
        arc_allocation_bytes::<Storage>(),
        std::ptr::from_ref(&*input.0) as usize,
    )
}

fn input_bytes(input: &ResourceInput) -> (usize, usize) {
    (input.0.bytes.capacity(), input.bytes().as_ptr() as usize)
}

fn dispatch_owner(dispatch: &ResourceDispatch) -> (usize, usize) {
    (
        arc_allocation_bytes::<DispatchStorage>(),
        std::ptr::from_ref(&*dispatch.0) as usize,
    )
}

#[test]
fn input_owner_stays_charged_until_physical_free() {
    if !isolated("input_owner_stays_charged_until_physical_free") {
        check(input, input_owner, false);
    }
}

#[test]
fn input_payload_stays_charged_until_physical_free() {
    if !isolated("input_payload_stays_charged_until_physical_free") {
        check(input, input_bytes, false);
    }
}

#[test]
fn input_racing_final_clones_stay_charged_until_physical_free() {
    if !isolated("input_racing_final_clones_stay_charged_until_physical_free") {
        check(input, input_owner, true);
    }
}

#[test]
fn dispatch_owner_stays_charged_until_physical_free() {
    if !isolated("dispatch_owner_stays_charged_until_physical_free") {
        check(dispatch, dispatch_owner, false);
    }
}

#[test]
fn dispatch_racing_final_clones_stay_charged_until_physical_free() {
    if !isolated("dispatch_racing_final_clones_stay_charged_until_physical_free") {
        check(dispatch, dispatch_owner, true);
    }
}

#[test]
fn dispatch_retained_input_owner_stays_charged_until_physical_free() {
    if !isolated("dispatch_retained_input_owner_stays_charged_until_physical_free") {
        check(dispatch, |value| input_owner(value.input()), false);
    }
}

#[test]
fn dispatch_retained_input_payload_stays_charged_until_physical_free() {
    if !isolated("dispatch_retained_input_payload_stays_charged_until_physical_free") {
        check(dispatch, |value| input_bytes(value.input()), false);
    }
}

#[test]
fn dispatch_ticket_payload_stays_charged_until_physical_free() {
    if !isolated("dispatch_ticket_payload_stays_charged_until_physical_free") {
        check(
            dispatch_with_retained_input,
            |(dispatch, _)| {
                let nonce = dispatch.ticket().nonce.as_str();
                (nonce.len(), nonce.as_ptr() as usize)
            },
            false,
        );
    }
}
