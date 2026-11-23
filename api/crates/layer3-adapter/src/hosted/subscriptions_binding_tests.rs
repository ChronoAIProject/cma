use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::hosted::subscriptions::HostedSubscriptionCredential;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use futures_util::FutureExt;
use std::sync::{
    atomic::{AtomicUsize, Ordering::SeqCst},
    Arc, Barrier,
};

struct Source {
    calls: AtomicUsize,
    _payload: Vec<u8>,
}

#[async_trait::async_trait]
impl HostedSubscriptionSource for Source {
    fn credential_admitted<'a>(
        &'a self,
        _: &'a str,
        _: &'a RuntimeHandle,
        _: &'a AgentSlot,
        _: bool,
        _: Option<&'a str>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, HostedSubscriptionCredential>, AdmittedError> {
        AdmittedOperation::new(
            policy,
            IngressClass::Data,
            AllocationPlan::checked("synthetic warm source", &[])?,
            |_| async {
                self.calls.fetch_add(1, SeqCst);
                Ok(HostedSubscriptionCredential::Warm)
            },
        )
    }

    async fn credential(
        &self,
        _: &str,
        _: &RuntimeHandle,
        _: &AgentSlot,
        _: bool,
        _: Option<&str>,
    ) -> crate::Result<HostedSubscriptionCredential> {
        panic!("admitted binding must never call the legacy source")
    }
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(512 * 1024);
    let floor = AllocationPlan::checked("binding fixture floor", &[64 * 1024]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor)
        .unwrap()
        .with_attachment_limit(2)
        .unwrap();
    let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
    // Isolate binding admission from the already declared root/class release locks.
    drop(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("release locks", &[1]).unwrap(),
            )
            .unwrap(),
    );
    (root, policy)
}

fn source(policy: &IngressOwnershipPolicy) -> SharedOwned<Source> {
    policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "synthetic retained source",
                &[sizing::shared_owner_bytes::<Source>(), 8192],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| Source {
            calls: AtomicUsize::new(0),
            _payload: vec![0; 8192],
        })
}

#[test]
fn a_legacy_binding_cannot_invoke_an_admitted_source_operation() {
    let (root, policy) = policy();
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        _payload: Vec::new(),
    });
    let binding: SubscriptionBinding = HostedSubscriptionBinding {
        source: source.clone(),
        sandbox_id: "synthetic-sandbox".into(),
        runtime: RuntimeHandle("synthetic-runtime".into()),
        agent: AgentSlot("synthetic-agent".into()),
    }
    .into();
    let id = serde_json::json!(1);
    let params = serde_json::json!({});
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| binding.reply_admitted(&id, &params, &policy)),
        Err(AdmittedError::Refused(AdmissionRefusal::InvalidPlan {
            phase: "subscription binding ownership"
        }))
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.calls.load(SeqCst), 0);
    root.assert_idle();
}

#[test]
fn binding_refuses_pressure_and_foreign_ownership_before_copying() {
    let (root, policy) = policy();
    let source = source(&policy);
    let runtime = RuntimeHandle("r".repeat(4096));
    let agent = AgentSlot("a".repeat(4096));
    let sandbox = "s".repeat(4096);
    let bytes = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("materialized binding pressure", &[bytes]).unwrap(),
        )
        .unwrap();
    let payload = vec![0u8; bytes];
    let (_, foreign) = self::policy();
    let foreign_source = self::source(&foreign);
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| AdmittedSubscriptionBinding::new(
            &policy,
            source.clone().into(),
            &sandbox,
            &runtime,
            &agent,
        )),
        Err(AdmissionRefusal::AtCapacity {
            phase: "subscription binding"
        })
    ));
    assert!(matches!(
        probe.track(|| AdmittedSubscriptionBinding::new(
            &policy,
            foreign_source.clone().into(),
            &sandbox,
            &runtime,
            &agent,
        )),
        Err(AdmissionRefusal::InvalidPlan {
            phase: "subscription binding ownership"
        })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.calls.load(SeqCst), 0);
    drop((payload, pressure, source));
    root.assert_idle();
}

#[test]
fn binding_rejects_a_different_attachment_on_the_same_root() {
    let (root, policy) = policy();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let source = source(first.policy());
    let runtime = RuntimeHandle("synthetic-runtime".into());
    let agent = AgentSlot("synthetic-agent".into());
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| AdmittedSubscriptionBinding::new(
            second.policy(),
            source.clone().into(),
            "synthetic-sandbox",
            &runtime,
            &agent,
        )),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    let binding = AdmittedSubscriptionBinding::new(
        first.policy(),
        source.clone().into(),
        "synthetic-sandbox",
        &runtime,
        &agent,
    )
    .unwrap();
    let id = serde_json::json!(1);
    let params = serde_json::json!({});
    let before = probe.allocated();
    assert!(probe
        .track(|| binding.reply_admitted(&id, &params, second.policy()))
        .is_err());
    assert_eq!(probe.allocated(), before);
    assert_eq!(source.calls.load(SeqCst), 0);
    drop((first, source));
    assert!(
        policy.admit_attachment().is_err(),
        "binding retains its source attachment"
    );
    drop(binding);
    drop(policy.admit_attachment().unwrap());
    drop(second);
    root.assert_idle();
}

#[test]
fn source_erasure_and_binding_clones_keep_actual_storage_without_new_allocations() {
    let (root, policy) = policy();
    let runtime = RuntimeHandle("r".repeat(32 * 1024));
    let agent = AgentSlot("a".repeat(32 * 1024));
    let sandbox = "s".repeat(32 * 1024);
    let baseline = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let source = probe.track(|| source(&policy));
    let before = probe.allocated();
    let erased: SharedOwned<dyn HostedSubscriptionSource> = probe.track(|| source.clone().into());
    assert_eq!(probe.allocated(), before);
    assert_eq!(
        &*source as *const Source as *const (),
        &*erased as *const dyn HostedSubscriptionSource as *const ()
    );
    let binding = probe.track(|| {
        AdmittedSubscriptionBinding::new(&policy, erased, &sandbox, &runtime, &agent).unwrap()
    });
    assert!(probe.peak() >= 3 * 32 * 1024 + 8192);
    assert!(probe.peak() <= baseline - root.available_bytes());
    assert_eq!(binding.value().sandbox_id, sandbox);
    assert_eq!(binding.value().runtime, runtime);
    assert_eq!(binding.value().agent, agent);
    let allocated = probe.allocated();
    let retained = probe.track(|| binding.clone());
    assert_eq!(probe.allocated(), allocated);
    assert_eq!(
        binding.value().sandbox_id.as_ptr(),
        retained.value().sandbox_id.as_ptr()
    );
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(binding);
        panic!("synthetic failed startup");
    }));
    assert!(unwind.is_err());
    drop(source);
    assert!(probe.live() > 0);
    assert!(probe.live() <= baseline - root.available_bytes());
    drop(retained);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

#[test]
fn isolated_binding_storage_stays_charged_through_final_physical_deallocation() {
    const TEST: &str = "hosted::subscriptions::binding::tests::isolated_binding_storage_stays_charged_through_final_physical_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    for concurrent in [false, true] {
        let (root, policy) = policy();
        let source = source(&policy);
        let runtime = RuntimeHandle("synthetic-runtime".into());
        let agent = AgentSlot("synthetic-agent".into());
        let baseline = root.available_bytes();
        let mut probe = AllocationProbe::new();
        let (binding, pause) = probe.pause_deallocation_containing(
            deallocation::arc_allocation_bytes::<Binding>(),
            || {
                AdmittedSubscriptionBinding::new(
                    &policy,
                    source.clone().into(),
                    "synthetic-sandbox",
                    &runtime,
                    &agent,
                )
                .unwrap()
            },
            |binding| binding.value() as *const Binding as usize,
        );
        let charged = baseline - root.available_bytes();
        let clone = binding.clone();
        let barrier = Arc::new(Barrier::new(2));
        pause.inspect_while_dropping(
            move || {
                if concurrent {
                    let peer = barrier.clone();
                    let worker = std::thread::spawn(move || {
                        peer.wait();
                        drop(binding);
                    });
                    barrier.wait();
                    drop(clone);
                    worker.join().unwrap();
                } else {
                    drop((binding, clone));
                }
            },
            || {
                assert_eq!(root.available_bytes(), baseline - charged);
                assert!(policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "pre-free binding reuse",
                            &[policy.budget(IngressClass::Data).available_bytes() + 1],
                        )
                        .unwrap()
                    )
                    .is_err());
            },
        );
        assert_eq!(root.available_bytes(), baseline);
        assert_eq!(probe.live(), 0);
        drop(source);
        root.assert_idle();
    }
}

#[tokio::test]
async fn retained_binding_outlives_bridge_and_keeps_the_source_callable() {
    let mut fixture = crate::hosted::acp::ingress::fixture::fixture();
    let root = fixture.root.clone();
    let policy = fixture.policy.clone();
    let source = source(&policy);
    let binding = AdmittedSubscriptionBinding::new(
        &policy,
        source.clone().into(),
        "synthetic-sandbox",
        &RuntimeHandle("synthetic-runtime".into()),
        &AgentSlot("synthetic-agent".into()),
    )
    .unwrap();
    fixture.bridge.subscription = Some(binding.clone().into());
    drop(fixture);
    assert!(root.reserved_work_bytes() > 0);
    let reply = binding
        .reply_admitted(&serde_json::json!(1), &serde_json::json!({}), &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(source.calls.load(SeqCst), 1);
    assert_eq!(reply["result"]["mode"], "warm");
    drop((reply, binding, source));
    root.assert_idle();
}
