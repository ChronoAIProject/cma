use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::AdmittedError;
use futures_util::{FutureExt, SinkExt};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, Ordering::SeqCst},
    Arc,
};

#[path = "acp_ingress_split_fixture.rs"]
mod fixture;
use fixture::{frame_scope, policy, Gate, ReadBlock};

#[test]
fn cancelled_send_keeps_frame_owned_until_later_flush_and_final_source_drop() {
    let (root, policy) = policy();
    let source = Gate::<SharedOwned<Message>, AdmittedError>::new();
    let ready = source.ready.clone();
    let sent = source.sent.clone();
    let (mut sink, stream) = super::super::admitted(Box::new(source), policy.clone())
        .split()
        .unwrap();
    let baseline = root.available_bytes();
    let scope = frame_scope(&policy);
    let charge = scope.requested_bytes();
    let mut probe = AllocationProbe::new();
    let message = probe.track(|| Message::Binary(vec![19; 4096]));
    assert!(probe
        .track(|| scope.run(sink.send(message)).now_or_never())
        .is_none());
    drop(scope);
    assert_eq!(baseline - root.available_bytes(), charge);
    assert!(probe.live() >= 4096);
    assert!(probe.live() <= charge);
    assert!(sent.lock().unwrap().is_empty());
    ready.store(true, SeqCst);
    sink.flush().now_or_never().unwrap().unwrap();
    let frame = sent.lock().unwrap().pop().unwrap();
    assert!(frame.belongs_to(&policy));
    assert_eq!(&*frame, &Message::Binary(vec![19; 4096]));
    drop((sink, stream, sent));
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charge);
    let last = frame.clone();
    drop(frame);
    assert!(probe.live() >= 4096);
    drop(last);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn legacy_split_control_keeps_a_live_unowned_frame_after_cancelled_send() {
    let (root, policy) = policy();
    let source = Gate::<Message, Error>::new();
    let (mut sink, stream) = StreamExt::split(source);
    let scope = frame_scope(&policy);
    let mut probe = AllocationProbe::new();
    let message = probe.track(|| Message::Binary(vec![31; 4096]));
    assert!(probe
        .track(|| scope.run(sink.send(message)).now_or_never())
        .is_none());
    drop(scope);
    assert_eq!(probe.live(), 4096);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop((sink, stream));
    assert_eq!(probe.live(), 0);
}

#[test]
fn split_admission_refuses_before_allocation_and_legacy_keeps_its_existing_behavior() {
    let (root, policy) = policy();
    let socket = super::super::admitted(Box::new(Gate::new()), policy.clone());
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "split pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| socket.split().err().unwrap());
    assert!(matches!(error, AdmissionRefusal::AtCapacity { .. }));
    assert_eq!(probe.allocated(), 0);
    drop(probe);
    let source = Gate::<Message, Error>::new();
    let ready = source.ready.clone();
    let sent = source.sent.clone();
    ready.store(true, SeqCst);
    let (mut sink, stream) = super::super::legacy(Box::new(source)).split().unwrap();
    sink.send(Message::Binary(vec![41; 32]))
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(
        sent.lock().unwrap().pop(),
        Some(Message::Binary(vec![41; 32]))
    );
    drop((sink, stream, pressure));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn missing_or_foreign_write_scope_cannot_fill_the_admitted_split_slot() {
    let (root, policy) = policy();
    let source = Gate::<SharedOwned<Message>, AdmittedError>::new();
    let sent = source.sent.clone();
    let (mut sink, stream) = super::super::admitted(Box::new(source), policy.clone())
        .split()
        .unwrap();
    let baseline = root.available_bytes();
    assert!(sink
        .send(Message::Binary(Vec::new()))
        .now_or_never()
        .unwrap()
        .is_err());
    let (foreign_root, foreign) = self::policy();
    let scope = frame_scope(&foreign);
    let result = scope
        .run(sink.send(Message::Binary(Vec::new())))
        .now_or_never()
        .unwrap();
    assert!(result.is_err());
    drop((result, scope));
    assert_eq!(
        foreign_root.available_bytes(),
        foreign_root.capacity_bytes()
    );
    assert_eq!(root.available_bytes(), baseline);
    assert!(sent.lock().unwrap().is_empty());
    drop((sink, stream));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn isolated_split_backing_remains_charged_until_both_halves_are_freed() {
    const TEST: &str = "hosted::acp::ingress::socket::split::tests::isolated_split_backing_remains_charged_until_both_halves_are_freed";
    if deallocation::isolated(TEST) {
        return;
    }
    // Mirror only the pinned BiLock allocation layout for the deallocator hook.
    // Its fields are never accessed; production sizing uses a conservative sum.
    struct Inner {
        _state: AtomicPtr<std::task::Waker>,
        _value: Option<std::cell::UnsafeCell<BridgeSocket>>,
    }
    let bytes = deallocation::arc_allocation_bytes::<Inner>();
    assert_ne!(
        bytes,
        deallocation::arc_allocation_bytes::<crate::ingress_budget::ReservationBundle>()
    );
    for sink_last in [false, true] {
        let (root, policy) = policy();
        let socket = super::super::admitted(Box::new(Gate::new()), policy.clone());
        let mut probe = AllocationProbe::new();
        let ((sink, stream), pause) = probe.pause_deallocation(bytes, || socket.split().unwrap());
        let charged = root.capacity_bytes() - root.available_bytes();
        assert!(charged >= bytes);
        if sink_last {
            drop(stream);
            assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
            assert!(pause.admission_blocked_while_dropping(
                &policy,
                IngressClass::Data,
                move || drop(sink)
            ));
        } else {
            drop(sink);
            assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
            assert!(pause.admission_blocked_while_dropping(
                &policy,
                IngressClass::Data,
                move || drop(stream)
            ));
        }
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[test]
fn contended_split_waker_allocation_fits_the_pre_admitted_layout() {
    let (root, policy) = policy();
    let block = Arc::new(ReadBlock {
        entered: AtomicBool::new(false),
        release: AtomicBool::new(false),
    });
    let mut socket = Gate::new();
    socket.ready.store(true, SeqCst);
    socket.read_block = Some(block.clone());
    let socket = super::super::admitted(Box::new(socket), policy.clone());
    let mut probe = AllocationProbe::new();
    let (mut sink, mut stream) = probe.track(|| socket.split().unwrap());
    let charged = root.capacity_bytes() - root.available_bytes();
    let child = std::thread::spawn(move || {
        assert!(stream.next().now_or_never().is_none());
        stream
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !block.entered.load(SeqCst) && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let entered = block.entered.load(SeqCst);
    let before = probe.allocated();
    let pending = probe.track(|| sink.flush().now_or_never().is_none());
    let parked = probe.allocated() - before;
    block.release.store(true, SeqCst);
    let stream = child.join().unwrap();
    assert!(entered && pending);
    assert!(parked >= size_of::<std::task::Waker>());
    assert!(probe.peak() <= charged);
    sink.flush().now_or_never().unwrap().unwrap();
    drop((sink, stream));
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
