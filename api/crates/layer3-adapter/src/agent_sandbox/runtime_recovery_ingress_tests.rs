use super::{ingress::*, *};
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, ClassBounds, IngressBudget,
    IngressClass, IngressOwnershipPolicy,
};
use futures_util::FutureExt;
use std::{
    future::{pending, poll_fn, ready, Future},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

fn command() -> RuntimeRecoveryCommand {
    RuntimeRecoveryCommand {
        program: "/fixture/program".into(),
        args: vec!["fixed-argument".into()],
        input: b"synthetic private input".to_vec(),
    }
}

fn binding() -> crate::agent_sandbox::PodBinding {
    crate::agent_sandbox::PodBinding {
        sandbox_name: "fixture-sandbox".into(),
        sandbox_uid: "fixture-sandbox-uid".into(),
        pod_name: "fixture-pod".into(),
        pod_uid: "fixture-pod-uid".into(),
        pod_ip: std::net::Ipv4Addr::LOCALHOST.into(),
        runtime_container_id: Some("fixture-container".into()),
        runtime_restart_count: Some(0),
    }
}

#[test]
fn command_validation_is_allocation_free_and_preserves_exact_limits() {
    let mut exact = command();
    exact.args = vec![String::new(); 64];
    exact.args[0] = "x".repeat(ARGUMENT_LIMIT - exact.program.len());
    exact.input = vec![0; IO_LIMIT];
    let mut probe = AllocationProbe::new();
    assert_eq!(probe.track(|| validate_command(&exact)), Ok(()));
    let invalid = [
        RuntimeRecoveryCommand {
            program: "relative".into(),
            ..command()
        },
        RuntimeRecoveryCommand {
            program: "/nul\0program".into(),
            ..command()
        },
        RuntimeRecoveryCommand {
            program: format!("/{}", "x".repeat(ARGUMENT_LIMIT)),
            ..command()
        },
        RuntimeRecoveryCommand {
            args: vec![String::new(); 65],
            ..command()
        },
        RuntimeRecoveryCommand {
            args: vec!["nul\0argument".into()],
            ..command()
        },
        RuntimeRecoveryCommand {
            args: vec!["x".repeat(ARGUMENT_LIMIT)],
            ..command()
        },
        RuntimeRecoveryCommand {
            input: vec![0; IO_LIMIT + 1],
            ..command()
        },
    ];
    for command in invalid {
        assert_eq!(
            probe.track(|| validate_command(&command)),
            Err(Failure::Command)
        );
    }
    assert_eq!(probe.allocated(), 0);
}

#[test]
fn binding_validation_is_allocation_free_and_checks_every_captured_identity() {
    let before = binding();
    let expected = (before.sandbox_name.as_str(), before.sandbox_uid.as_str());
    let mut probe = AllocationProbe::new();
    assert_eq!(probe.track(|| validate_binding(expected, &before)), Ok(()));
    for changed in [
        crate::agent_sandbox::PodBinding {
            sandbox_name: "different".into(),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            sandbox_uid: "different".into(),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            pod_name: "different".into(),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            pod_uid: "different".into(),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            pod_ip: std::net::Ipv6Addr::LOCALHOST.into(),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            runtime_container_id: Some("different".into()),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            runtime_restart_count: Some(1),
            ..before.clone()
        },
    ] {
        assert_eq!(
            probe.track(|| validate_final_binding(&before, &changed)),
            Err(Failure::ChangedIdentity)
        );
    }
    for missing in [
        crate::agent_sandbox::PodBinding {
            runtime_container_id: None,
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            runtime_container_id: Some(String::new()),
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            runtime_restart_count: None,
            ..before.clone()
        },
        crate::agent_sandbox::PodBinding {
            runtime_restart_count: Some(-1),
            ..before.clone()
        },
    ] {
        assert_eq!(
            probe.track(|| validate_binding(expected, &missing)),
            Err(Failure::ContainerIdentity)
        );
    }
    assert_eq!(
        probe.track(|| validate_binding(("other", expected.1), &before)),
        Err(Failure::SandboxIdentity)
    );
    assert_eq!(probe.allocated(), 0);
}

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic recovery output", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(256 * 1024), plan(64 * 1024)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

// Only the shared I/O phase is qualified here. The native client, HTTP stack,
// codec and executor still require their own admitted constructors and owners.
fn admitted_output<'a>(
    policy: &IngressOwnershipPolicy,
    bytes: &'a [u8],
    started: &'a AtomicBool,
    status: impl Future<Output = bool> + Send + 'a,
) -> std::result::Result<AdmittedOperation<'a, Vec<u8>>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Control,
        plan(IO_LIMIT + 1),
        move |scope| async move {
            started.store(true, SeqCst);
            let output = exchange_io(
                tokio::io::sink(),
                bytes,
                b"",
                status,
                || ready(true),
                Vec::with_capacity(IO_LIMIT + 1),
            )
            .await
            .map_err(|_| AdmittedError::Unavailable("synthetic recovery I/O failed"))?;
            Ok(scope.initialize_owned(|| output))
        },
    )
}

#[test]
fn output_admission_refuses_before_buffer_allocation_or_io() {
    let (root, policy) = policy();
    let held = policy
        .admit(
            IngressClass::Control,
            plan(policy.budget(IngressClass::Control).available_bytes()),
        )
        .unwrap();
    let started = AtomicBool::new(false);
    let mut probe = AllocationProbe::new();
    let failure =
        probe.track(|| admitted_output(&policy, b"fixture", &started, ready(true)).unwrap_err());
    assert!(matches!(
        failure.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert!(!started.load(SeqCst));
    drop(held);
    root.assert_idle();
}

#[test]
fn admitted_output_retains_exact_buffer_without_growth_under_data_pressure() {
    let (root, policy) = policy();
    let pressure = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let started = AtomicBool::new(false);
    for length in [0, 1, IO_LIMIT] {
        let input = vec![b'x'; length];
        let mut probe = AllocationProbe::new();
        let mut operation =
            probe.track(|| admitted_output(&policy, &input, &started, ready(true)).unwrap());
        let output = probe.track(|| (&mut operation).now_or_never().unwrap().unwrap());
        assert_eq!(output.as_slice(), input);
        assert_eq!(output.capacity(), IO_LIMIT + 1);
        assert!(probe.peak() <= operation.requested_bytes());
        drop(operation);
        assert_eq!(
            probe.live(),
            IO_LIMIT
                + 1
                + crate::ingress_budget::sizing::shared_allocation_bytes::<
                    crate::ingress_budget::ReservationBundle,
                >()
        );
        assert!(output.belongs_to(&policy));
        drop(output);
        assert_eq!(probe.live(), 0);
    }
    drop(pressure);
    root.assert_idle();
}

#[test]
fn output_limit_and_failed_status_prevent_join_and_success() {
    for (length, status, expected) in [
        (IO_LIMIT + 1, true, Failure::OutputLimit),
        (0, false, Failure::Exit),
    ] {
        let bytes = vec![b'x'; length];
        let joined = AtomicBool::new(false);
        let result = exchange_io(
            tokio::io::sink(),
            bytes.as_slice(),
            b"",
            ready(status),
            || {
                joined.store(true, SeqCst);
                ready(true)
            },
            Vec::with_capacity(IO_LIMIT + 1),
        )
        .now_or_never()
        .unwrap();
        assert_eq!(result, Err(expected));
        assert!(!joined.load(SeqCst));
    }
    assert_eq!(
        exchange_io(
            tokio::io::sink(),
            &b""[..],
            b"",
            ready(true),
            || ready(false),
            Vec::new()
        )
        .now_or_never()
        .unwrap(),
        Err(Failure::Join)
    );
}

#[derive(Default)]
struct Gates {
    input: AtomicBool,
    eof: AtomicBool,
    status: AtomicBool,
    join: AtomicBool,
    joining: AtomicBool,
    shutdowns: AtomicUsize,
}

struct Input<'a>(&'a Gates);
impl AsyncWrite for Input<'_> {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.0.input.load(SeqCst) {
            Poll::Ready(Ok(bytes.len()))
        } else {
            Poll::Pending
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.0.shutdowns.fetch_add(1, SeqCst);
        Poll::Ready(Ok(()))
    }
}
struct Output<'a>(&'a Gates);
impl AsyncRead for Output<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.0.eof.load(SeqCst) {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

#[test]
fn no_output_is_published_before_stdin_eof_status_and_join_complete() {
    let gates = Gates::default();
    let mut operation = Box::pin(exchange_io(
        Input(&gates),
        Output(&gates),
        b"fixture input",
        poll_fn(|_| {
            if gates.status.load(SeqCst) {
                Poll::Ready(true)
            } else {
                Poll::Pending
            }
        }),
        || {
            gates.joining.store(true, SeqCst);
            poll_fn(|_| {
                if gates.join.load(SeqCst) {
                    Poll::Ready(true)
                } else {
                    Poll::Pending
                }
            })
        },
        Vec::new(),
    ));
    assert!((&mut operation).now_or_never().is_none());
    assert_eq!(gates.shutdowns.load(SeqCst), 0);
    for gate in [&gates.input, &gates.eof] {
        gate.store(true, SeqCst);
        assert!((&mut operation).now_or_never().is_none());
        assert!(!gates.joining.load(SeqCst));
    }
    gates.status.store(true, SeqCst);
    assert!((&mut operation).now_or_never().is_none());
    assert!(gates.joining.load(SeqCst));
    assert_eq!(gates.shutdowns.load(SeqCst), 1);
    gates.join.store(true, SeqCst);
    assert_eq!((&mut operation).now_or_never().unwrap(), Ok(Vec::new()));
}

#[test]
fn cancellation_drops_the_buffer_and_future_before_returning_capacity() {
    let (root, policy) = policy();
    let started = AtomicBool::new(false);
    let mut probe = AllocationProbe::new();
    let mut operation =
        probe.track(|| admitted_output(&policy, b"fixture", &started, pending()).unwrap());
    assert!(probe.track(|| (&mut operation).now_or_never()).is_none());
    assert!(started.load(SeqCst));
    assert!(probe.live() > IO_LIMIT);
    assert!(root.reserved_work_bytes() >= probe.live());
    drop(operation);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

#[test]
fn output_custody_survives_physical_buffer_deallocation() {
    if deallocation::isolated("agent_sandbox::runtime_recovery::ingress_tests::output_custody_survives_physical_buffer_deallocation") { return; }
    let (root, policy) = policy();
    let started = AtomicBool::new(false);
    let mut probe = AllocationProbe::new();
    let (output, pause) = probe.pause_deallocation(IO_LIMIT + 1, || {
        admitted_output(&policy, b"fixture", &started, ready(true))
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    assert!(pause
        .admission_blocked_while_dropping(&policy, IngressClass::Control, move || drop(output)));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}
