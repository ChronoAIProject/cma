use super::*;
use crate::ingress_budget::{sizing, ClassBounds, IngressBudget, IngressWorkScope};

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(2 * 1024 * 1024);
    let floor = plan(64 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("extended work fixture", &[bytes]).unwrap()
}

fn new_scope(policy: &IngressOwnershipPolicy) -> IngressWorkScope {
    IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Data,
                plan(sizing::shared_owner_bytes::<String>()),
            )
            .unwrap(),
    )
}

#[tokio::test]
async fn extension_survives_completed_future_and_every_result_clone() {
    let (root, policy) = fixture();
    let scope = new_scope(&policy);
    let owned = scope
        .run(async {
            IngressWorkScope::current()
                .unwrap()
                .admit_extra(&policy, IngressClass::Data, plan(4096))
                .unwrap();
            "x".repeat(4096)
        })
        .await
        .into_shared_admitted()
        .unwrap();
    let charged = root.reserved_work_bytes();
    assert_eq!(charged, scope.requested_bytes());
    assert!(charged >= 4096 + size_of::<Node>());
    let clone = owned.clone();
    drop((scope, owned));
    assert_eq!(root.reserved_work_bytes(), charged);
    assert_eq!(clone.len(), 4096);
    drop(clone);
    root.assert_idle();
}

#[tokio::test]
async fn cancelled_parent_cannot_release_an_inherited_extended_result() {
    let (root, policy) = fixture();
    let scope = new_scope(&policy);
    let (sent, received) = tokio::sync::oneshot::channel();
    let parent = tokio::spawn(async move {
        scope
            .run(async move {
                let child = tokio::spawn(IngressWorkScope::inherit_owned(async move {
                    IngressWorkScope::current()
                        .unwrap()
                        .admit_extra(&policy, IngressClass::Data, plan(8192))
                        .unwrap();
                    "y".repeat(8192)
                }));
                sent.send(child).unwrap();
                std::future::pending::<()>().await;
            })
            .await
    });
    let child = received.await.unwrap();
    parent.abort();
    assert!(parent.await.unwrap_err().is_cancelled());
    while !child.is_finished() {
        tokio::task::yield_now().await;
    }
    assert!(root.reserved_work_bytes() >= 8192);
    let result = child.await.unwrap();
    assert_eq!(result.len(), 8192);
    drop(result);
    root.assert_idle();
}

#[test]
fn foreign_root_overflow_and_contention_leave_no_partial_allowance() {
    let (root, policy) = fixture();
    let (foreign_root, foreign) = fixture();
    let scope = new_scope(&policy);
    let before = root.available_bytes();
    assert!(matches!(
        scope.admit_extra(&foreign, IngressClass::Data, plan(4096)),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    foreign_root.assert_idle();
    assert!(matches!(
        scope.admit_extra(&policy, IngressClass::Data, plan(u32::MAX as usize)),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    let extensions = Extensions::default();
    let lock = extensions.nodes.try_lock().unwrap();
    assert!(matches!(
        extensions.admit(&policy, IngressClass::Control, plan(4096)),
        Err(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(extensions.requested_bytes(), 0);
    assert_eq!(root.available_bytes(), before);
    drop((lock, scope));
    root.assert_idle();
}

#[test]
fn split_keeps_extension_storage_with_its_original_owner() {
    let (root, policy) = fixture();
    let mut bundle = policy.admit(IngressClass::Data, plan(1024)).unwrap();
    bundle
        .admit_extra(&policy, IngressClass::Data, plan(4096))
        .unwrap();
    let child = bundle.split_admitted(plan(512)).unwrap();
    assert_eq!(child.requested_bytes(), 512);
    let retained = bundle.requested_bytes();
    assert!(retained >= 512 + 4096 + size_of::<Node>());
    drop(child);
    assert_eq!(root.reserved_work_bytes(), retained);
    drop(bundle);
    root.assert_idle();
}

#[test]
fn long_extension_list_drops_without_recursive_stack_growth() {
    let (root, policy) = fixture();
    let scope = new_scope(&policy);
    for _ in 0..4096 {
        scope
            .admit_extra(&policy, IngressClass::Data, plan(1))
            .unwrap();
    }
    drop(scope);
    root.assert_idle();
}

#[test]
fn isolated_extension_allocation_contract() {
    const TEST: &str =
        "ingress_budget::ownership::extension::tests::isolated_extension_allocation_contract";
    if std::env::var_os("CMA_SCOPE_EXTENSION_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_SCOPE_EXTENSION_ALLOCATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (root, policy) = fixture();
    drop(new_scope(&policy));
    let mut extensions = Extensions::default();
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    probe
        .track(|| extensions.admit(&policy, IngressClass::Data, plan(128)))
        .unwrap();
    let node = extensions.nodes.get_mut().take().unwrap();
    let live = probe.live();
    let charged = root.reserved_work_bytes();
    let (storage, tail) = probe.track(|| pop(node));
    assert!(tail.is_none());
    assert_eq!(
        probe.live(),
        live - size_of::<Node>(),
        "free node before guard release"
    );
    assert_eq!(root.reserved_work_bytes(), charged);
    drop((extensions, storage));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
    drop(probe);
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let scope = probe.track(|| new_scope(&policy));
    probe
        .track(|| scope.admit_extra(&policy, IngressClass::Data, plan(8192)))
        .unwrap();
    let result = probe.track(|| scope.initialize(|| "z".repeat(8192)));
    let charged = scope.requested_bytes();
    assert!(
        probe.peak() <= charged,
        "peak={} admitted={charged}",
        probe.peak()
    );
    let clone = probe.track(|| result.clone());
    probe.track(|| drop((scope, result)));
    assert_eq!(root.reserved_work_bytes(), charged);
    assert!(probe.live() >= 8192);
    eprintln!(
        "extended_scope peak={} retained={} admitted={charged}",
        probe.peak(),
        probe.live()
    );
    probe.track(|| drop(clone));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
    drop(probe);

    let scope = new_scope(&policy);
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let unextended = probe.track(|| scope.initialize(|| "z".repeat(8192)));
    assert!(
        probe.peak() > scope.requested_bytes(),
        "negative control must detect missing admission"
    );
    eprintln!(
        "unextended_scope peak={} admitted={}",
        probe.peak(),
        scope.requested_bytes()
    );
    drop((unextended, probe));

    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let refused: [_; 128] = probe.track(|| {
        std::array::from_fn::<_, 128, _>(|_| {
            scope.admit_extra(&policy, IngressClass::Data, plan(8192))
        })
    });
    assert!(refused
        .iter()
        .all(|result| matches!(result, Err(AdmissionRefusal::AtCapacity { .. }))));
    assert_eq!(probe.allocated(), 0);
    probe
        .track(|| scope.admit_extra(&policy, IngressClass::Control, plan(128)))
        .unwrap();
    let control = probe.track(|| scope.initialize_owned(|| vec![0u8; 128]));
    assert_eq!(control.len(), 128);
    drop((control, probe, scope, pressure));
    root.assert_idle();
}
