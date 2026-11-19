use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{
    sizing, AdmittedError, AllocationPlan, ClassBounds, IngressBudget, IngressClass,
    IngressOwnershipPolicy,
};
use futures_util::{FutureExt, StreamExt};

struct ErrorSocket(IngressOwnershipPolicy);

impl ErrorSocket {
    fn failure(&self) -> AdmittedError {
        let plan = AllocationPlan::checked(
            "synthetic socket error",
            &[2048, sizing::shared_owner_bytes::<crate::AdapterError>()],
        )
        .unwrap();
        match self.0.admit(IngressClass::Control, plan) {
            Ok(storage) => storage
                .initialize(|| crate::AdapterError::Unavailable("x".repeat(2048)))
                .into(),
            Err(refusal) => refusal.into(),
        }
    }
}

impl Stream for ErrorSocket {
    type Item = Result<SharedOwned<Message>, AdmittedError>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(Err(self.failure())))
    }
}

impl Sink<SharedOwned<Message>> for ErrorSocket {
    type Error = AdmittedError;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Err(self.failure()))
    }
    fn start_send(self: Pin<&mut Self>, _: SharedOwned<Message>) -> Result<(), Self::Error> {
        Err(self.failure())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Err(self.failure()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Err(self.failure()))
    }
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(65536);
    let plan = |bytes| AllocationPlan::checked("socket test floor", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(32768), plan(8192)).unwrap(),
    )
    .unwrap();
    for class in [IngressClass::Data, IngressClass::Control] {
        drop(policy.admit(class, plan(1)).unwrap());
    }
    (root, policy)
}

fn assert_fixed_transport_error(error: &Error) {
    let Error::Io(error) = error else {
        panic!("existing IO failure classification expected")
    };
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert!(
        error.get_ref().is_none(),
        "the Bridge must not box a second diagnostic"
    );
}

#[test]
fn isolated_socket_errors_retain_storage_through_read_and_every_sink_failure() {
    if deallocation::isolated("hosted::acp::ingress::socket_tests::isolated_socket_errors_retain_storage_through_read_and_every_sink_failure") { return; }
    let (root, policy) = policy();
    // A producer satisfying the former raw-error contract could return this
    // allocation after every reservation had ended. Keep it as a live control.
    let mut legacy = futures_util::stream::poll_fn(|_| {
        Poll::Ready(Some(Err::<SharedOwned<Message>, _>(
            crate::AdapterError::Unavailable("x".repeat(2048)),
        )))
    });
    let mut probe = AllocationProbe::new();
    let raw_error = probe.track(|| legacy.next().now_or_never().unwrap().unwrap().unwrap_err());
    assert_eq!(probe.live(), 2048);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop(raw_error);
    assert_eq!(probe.live(), 0);
    drop(probe);
    let mut socket: Box<dyn AdmittedSocket<Message>> = Box::new(ErrorSocket(policy.clone()));
    let frame = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "synthetic outgoing frame",
                &[sizing::shared_owner_bytes::<Message>()],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| Message::Binary(Vec::new()));
    let waker = futures_util::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut probe = AllocationProbe::new();
    let errors = [
        probe.track(|| socket.next().now_or_never().unwrap().unwrap().unwrap_err()),
        probe.track(|| match Pin::new(&mut socket).poll_ready(&mut cx) {
            Poll::Ready(Err(error)) => error,
            _ => panic!("failure expected"),
        }),
        probe.track(|| Pin::new(&mut socket).start_send(frame).unwrap_err()),
        probe.track(|| match Pin::new(&mut socket).poll_flush(&mut cx) {
            Poll::Ready(Err(error)) => error,
            _ => panic!("failure expected"),
        }),
        probe.track(|| match Pin::new(&mut socket).poll_close(&mut cx) {
            Poll::Ready(Err(error)) => error,
            _ => panic!("failure expected"),
        }),
    ];
    assert!(errors.iter().all(|error| error.belongs_to(&policy)));
    assert!(errors.iter().all(|error| matches!(error.as_adapter(), Some(crate::AdapterError::Unavailable(text)) if text.len() == 2048)));
    assert!(probe.live() >= 5 * 2048);
    assert!(probe.live() <= root.capacity_bytes() - root.available_bytes());
    let clone = probe.track(|| errors[0].clone());
    drop((socket, errors, policy));
    assert!(probe.live() >= 2048);
    assert!(probe.live() <= root.capacity_bytes() - root.available_bytes());
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn socket_producer_refuses_before_error_allocation_when_control_is_exhausted() {
    let (root, policy) = policy();
    let pressure = policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("socket error pressure", &[policy.bounds().control_bytes()])
                .unwrap(),
        )
        .unwrap();
    let mut socket: Box<dyn AdmittedSocket<Message>> = Box::new(ErrorSocket(policy.clone()));
    let mut probe = AllocationProbe::new();
    let errors: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| socket.next().now_or_never().unwrap().unwrap().unwrap_err())
    });
    assert!(errors.iter().all(|error| error.refusal().is_some()));
    assert_eq!(probe.allocated(), 0);
    drop((probe, socket, pressure));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn bridge_consumes_owned_errors_without_second_allocation_or_changed_classification() {
    let (root, policy) = policy();
    let mut socket = admitted(Box::new(ErrorSocket(policy.clone())), policy.clone());
    let scope = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "synthetic write scope",
                    &[
                        sizing::shared_owner_bytes::<Message>(),
                        sizing::shared_owner_bytes::<()>(),
                    ],
                )
                .unwrap(),
            )
            .unwrap(),
    );
    // Initialize process-lived tracing before attributing operation storage.
    drop(socket.next().now_or_never().unwrap());
    let baseline = root.available_bytes();
    let waker = futures_util::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut probe = AllocationProbe::new();
    let mut errors = Vec::with_capacity(4);
    let producer_bytes = 2048
        + deallocation::arc_allocation_bytes::<crate::AdapterError>()
        + deallocation::arc_allocation_bytes::<crate::ingress_budget::ReservationBundle>();
    for phase in 0..4 {
        let before = probe.allocated();
        let error = probe.track(|| {
            if phase == 0 {
                socket
                    .next()
                    .now_or_never()
                    .unwrap()
                    .unwrap()
                    .err()
                    .unwrap()
            } else {
                let result = match phase {
                    1 => Pin::new(&mut socket).poll_ready(&mut cx),
                    2 => Pin::new(&mut socket).poll_flush(&mut cx),
                    _ => Pin::new(&mut socket).poll_close(&mut cx),
                };
                match result {
                    Poll::Ready(Err(error)) => error,
                    _ => panic!("failure expected"),
                }
            }
        });
        assert_fixed_transport_error(&error);
        assert_eq!(probe.allocated() - before, producer_bytes);
        errors.push(error);
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), baseline);
    }
    let before = probe.allocated();
    let send = probe.track(|| {
        scope
            .run(async { Pin::new(&mut socket).start_send(Message::Binary(Vec::new())) })
            .now_or_never()
            .unwrap()
    });
    assert_fixed_transport_error(send.as_ref().unwrap_err());
    assert_eq!(
        probe.allocated() - before,
        producer_bytes + deallocation::arc_allocation_bytes::<Message>()
    );
    drop((send, errors));
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), baseline);
    drop((probe, socket, scope));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
