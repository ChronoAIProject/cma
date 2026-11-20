use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::hosted::{permission_state::PermissionState, retained_json::JsonPools};
use crate::ingress_budget::{AllocationPlan, ClassBounds, IngressClass, IngressOwnershipPolicy};

#[test]
fn isolated_conversation_manifest_covers_cold_process_component_storage() {
    const TEST: &str = "hosted::acp::output::component_tests::isolated_conversation_manifest_covers_cold_process_component_storage";
    if deallocation::isolated(TEST) {
        return;
    }
    let root = IngressBudget::process();
    let empty = AllocationPlan::checked("no downstream fixture work", &[]).unwrap();
    let floor = AllocationPlan::checked("generic metadata fixture", &[1]).unwrap();
    let base = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 200, empty, empty)
            .unwrap();
    let declared = bounds.metadata_bytes() - base.metadata_bytes();
    let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("resident class lock", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        let raw = memory::process_budget(true).unwrap();
        drop(raw.clone().try_acquire_owned().unwrap());
        let json = JsonPools::process().with_ownership(&policy);
        drop(json.decode(b"null").unwrap());
        drop(PermissionState::new(Some(policy.budget(IngressClass::Data))).unwrap());
        for pool in [
            crate::hosted::sign_in_retention::process_pool(),
            crate::hosted::observation_retention::process_pool(),
        ] {
            drop(pool.try_acquire_owned().unwrap());
        }
    });
    root.assert_idle();
    assert!(probe.live() > 0, "observe cold process storage");
    eprintln!("component resident={} declared={declared}", probe.live());
    assert!(
        probe.live() <= declared,
        "process component storage outlives work without a resident allowance"
    );
    let resident = probe.live();
    probe.track(|| {
        for _ in 0..16 {
            drop(memory::process_budget(true).unwrap());
            drop(JsonPools::process().with_ownership(&policy));
            drop(PermissionState::new(Some(policy.budget(IngressClass::Data))).unwrap());
        }
    });
    assert_eq!(
        probe.live(),
        resident,
        "process constructors share one fixed set"
    );
    root.assert_idle();
    drop((root, policy));
    assert!(probe.live() > 0, "the process pools retain their storage");
}

#[tokio::test]
async fn admitted_open_refuses_missing_process_metadata_before_allocating_work() {
    use crate::ingress_budget::{AdmissionRefusal, AdmittedError};
    use futures_util::FutureExt;

    let root = IngressBudget::new(128 * 1024 * 1024);
    let floor = AllocationPlan::checked("unprovisioned open floor", &[8 * 1024 * 1024]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor)
        .unwrap()
        .with_attachment_limit(1)
        .unwrap();
    let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
    // Cold root/class tests cover their declared lazy release-lock backing.
    // Isolate new open work from that already admitted metadata on cleanup.
    drop(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("initialize release locks", &[1]).unwrap(),
            )
            .unwrap(),
    );
    let attachment = policy.admit_attachment().unwrap();
    let mut settings = super::super::tests::settings(None, false);
    settings.retained_json = true;
    settings.bounded_raw_output = true;
    settings.bounded_permission_state = true;
    settings.shared_runtime_output = true;
    settings.output_json_budget = true;
    let (updates, _) = mpsc::channel(1);
    let socket = Box::new(UnpolledSocket);
    let cwd = "/synthetic/work".to_owned();
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        Bridge::open_admitted(
            socket,
            "agent",
            cwd,
            (settings, None, attachment),
            None,
            None,
            updates,
        )
        .now_or_never()
    });
    assert!(matches!(
        result,
        Some(Err(AdmittedError::Refused(AdmissionRefusal::InvalidPlan {
            phase: "conversation process metadata"
        })))
    ));
    assert_eq!(probe.allocated(), 0);
    root.assert_idle();
}

struct UnpolledSocket;

impl futures_util::Stream for UnpolledSocket {
    type Item =
        Result<crate::ingress_budget::SharedOwned<Message>, crate::ingress_budget::AdmittedError>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("missing metadata must refuse before reading the socket")
    }
}

impl futures_util::Sink<crate::ingress_budget::SharedOwned<Message>> for UnpolledSocket {
    type Error = crate::ingress_budget::AdmittedError;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        panic!("missing metadata must refuse before polling the sink")
    }
    fn start_send(
        self: std::pin::Pin<&mut Self>,
        _: crate::ingress_budget::SharedOwned<Message>,
    ) -> Result<(), Self::Error> {
        panic!("missing metadata must refuse before sending")
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        panic!("missing metadata must refuse before flushing")
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        panic!("missing metadata must refuse before closing")
    }
}
