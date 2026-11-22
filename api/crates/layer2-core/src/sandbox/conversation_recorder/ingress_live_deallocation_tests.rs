use super::*;
use crate::sandbox::conversation_recorder::live::{AgentLiveFragmentKind, AgentLiveHub};
use layer3_adapter::ingress_budget::AllocationPlan;

#[path = "ingress_live_deallocation_probe.rs"]
mod probe;

fn fragment() -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: crate::AgentId("synthetic-free-agent".into()),
        runtime_generation: 1,
        turn_sequence: 2,
        after_seq: 3,
        message_id: "synthetic-free-message".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: "x".repeat(4093),
    }
}

fn arc_bytes<T>() -> usize {
    std::alloc::Layout::new::<[usize; 2]>()
        .extend(std::alloc::Layout::new::<T>())
        .unwrap()
        .0
        .pad_to_align()
        .size()
}

#[derive(Clone, Copy, Debug)]
enum FinalOwner {
    Direct,
    Racing,
    Deferred,
    Cancelled,
    Evicted,
}

fn check(scenario: FinalOwner, target_bytes: usize) {
    let root = IngressBudget::new(256 * 1024 * 1024);
    let zero = AllocationPlan::checked("live free fixture", &[]).unwrap();
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 1, zero, zero).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let idle_root = root.capacity_bytes() - root.metadata_bytes();
    assert_eq!(root.available_bytes(), idle_root);
    let attachment = policy.admit_attachment().unwrap();
    let attached_root = root.available_bytes();
    let attached_data = policy.budget(IngressClass::Data).available_bytes();
    let source = fragment();
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    let local = Arc::new(Semaphore::new(bytes));
    let budget = Arc::new(LiveBudget {
        local: local.clone(),
        aggregate: Some(attachment.policy().budget(IngressClass::Data)),
        pre_admit: true,
    });
    let hub = AgentLiveHub::with_budget(budget.clone());
    let mut first = hub.subscribe(&source.agent_id);
    let mut second = hub.subscribe(&source.agent_id);
    // Warm every broadcast slot before isolating the delivery's counted layouts.
    // Slot/queue storage qualification belongs to its separate owner.
    for _ in 0..256 {
        hub.publish(Some(source.clone()));
        drop(first.try_recv().unwrap());
        drop(second.try_recv().unwrap());
    }
    let delivery = if matches!(scenario, FinalOwner::Direct | FinalOwner::Racing) {
        probe::capture(|| budget.copy_chunk(&source, &source.delta).unwrap())
    } else {
        let input = source.clone();
        probe::capture(|| hub.publish(Some(input)));
        first.try_recv().unwrap()
    };
    let target = probe::address_of_size(target_bytes);
    let live = probe::live_bytes();
    assert_eq!(
        live,
        bytes,
        "{scenario:?}: charge must equal actual delivery allocations; {:?}",
        probe::live_layouts()
    );
    assert_eq!(local.available_permits(), 0);
    let held_root = root.available_bytes();
    let held_data = policy.budget(IngressClass::Data).available_bytes();
    assert_eq!(attached_root - held_root, bytes);
    assert_eq!(attached_data - held_data, bytes);
    let release: Box<dyn FnOnce() + Send> = match scenario {
        FinalOwner::Direct => {
            drop((first, second, hub, budget));
            Box::new(move || drop(delivery))
        }
        FinalOwner::Racing => {
            drop((first, second, hub, budget));
            let other = delivery.clone();
            Box::new(move || {
                let barrier = Arc::new(std::sync::Barrier::new(2));
                let other_barrier = barrier.clone();
                let worker = std::thread::spawn(move || {
                    other_barrier.wait();
                    drop(other);
                });
                barrier.wait();
                drop(delivery);
                worker.join().unwrap();
            })
        }
        FinalOwner::Deferred => {
            let subscriber = second.try_recv().unwrap();
            let deferred = delivery.clone();
            drop((subscriber, delivery, first, second, hub, budget));
            Box::new(move || drop(deferred))
        }
        FinalOwner::Cancelled => {
            drop((delivery, first, hub, budget));
            Box::new(move || drop(second))
        }
        FinalOwner::Evicted => {
            drop((delivery, first, budget));
            Box::new(move || {
                // The sole payload exhausts the component; new publications
                // enqueue gaps until the original subscriber frame is evicted.
                for _ in 0..256 {
                    hub.publish(Some(source.clone()));
                }
                drop((second, hub));
            })
        }
    };
    drop(attachment);
    let observed = probe::pause(target, release, || {
        let root_available = root.available_bytes();
        let data_available = policy.budget(IngressClass::Data).available_bytes();
        let component_available = local.available_permits();
        let live = probe::live_bytes();
        let competitor = Arc::new(Semaphore::new(root.capacity_bytes()));
        let root_blocked =
            IngressReservation::try_acquire(&competitor, Some(&root), held_root + 1).is_none();
        let data_blocked = policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("competing retained live data", &[held_data + 1]).unwrap(),
            )
            .is_err();
        let component_blocked = local.clone().try_acquire_owned().is_err();
        let attachment_blocked = policy.admit_attachment().is_err();
        (
            root_available,
            data_available,
            component_available,
            live,
            root_blocked,
            data_blocked,
            component_blocked,
            attachment_blocked,
        )
    });
    assert_eq!(
        observed.0, held_root,
        "root returned before {scenario:?} physical free: {observed:?}"
    );
    assert_eq!(
        observed.1, held_data,
        "Data returned before {scenario:?} physical free"
    );
    assert_eq!(
        observed.2, 0,
        "component returned before {scenario:?} physical free"
    );
    assert!(
        observed.3 > 0,
        "target must remain live until System.dealloc"
    );
    assert!(
        observed.4 && observed.5 && observed.6,
        "competing admission reused retiring storage"
    );
    assert!(
        observed.7,
        "last attachment custody returned before physical free"
    );
    assert_eq!(probe::live_bytes(), 0);
    assert_eq!(local.available_permits(), bytes);
    assert_eq!(root.available_bytes(), idle_root);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        bounds.data_bytes()
    );
    drop(policy.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), idle_root);
}

macro_rules! physical_free_case {
    ($name:ident, $scenario:ident, $bytes:expr) => {
        #[test]
        fn $name() {
            let name = concat!(module_path!(), "::", stringify!($name))
                .strip_prefix("layer2_core::")
                .unwrap();
            if !probe::isolated(name) {
                check(FinalOwner::$scenario, $bytes);
            }
        }
    };
}

physical_free_case!(
    direct_owner_backing_stays_charged_until_physical_free,
    Direct,
    arc_bytes::<IngressReservation>()
);

physical_free_case!(
    racing_owner_backing_stays_charged_until_physical_free,
    Racing,
    arc_bytes::<IngressReservation>()
);

physical_free_case!(
    deferred_owner_backing_stays_charged_until_physical_free,
    Deferred,
    arc_bytes::<IngressReservation>()
);

physical_free_case!(
    cancelled_owner_backing_stays_charged_until_physical_free,
    Cancelled,
    arc_bytes::<IngressReservation>()
);

physical_free_case!(
    evicted_owner_backing_stays_charged_until_physical_free,
    Evicted,
    arc_bytes::<IngressReservation>()
);

physical_free_case!(
    direct_payload_stays_charged_until_physical_free,
    Direct,
    4093
);

physical_free_case!(
    racing_payload_stays_charged_until_physical_free,
    Racing,
    4093
);

physical_free_case!(
    deferred_payload_stays_charged_until_physical_free,
    Deferred,
    4093
);

physical_free_case!(
    cancelled_payload_stays_charged_until_physical_free,
    Cancelled,
    4093
);

physical_free_case!(
    evicted_payload_stays_charged_until_physical_free,
    Evicted,
    4093
);

physical_free_case!(
    direct_fragment_backing_stays_charged_until_physical_free,
    Direct,
    arc_bytes::<AgentLiveFragment>()
);

physical_free_case!(
    racing_fragment_backing_stays_charged_until_physical_free,
    Racing,
    arc_bytes::<AgentLiveFragment>()
);

physical_free_case!(
    deferred_fragment_backing_stays_charged_until_physical_free,
    Deferred,
    arc_bytes::<AgentLiveFragment>()
);

physical_free_case!(
    cancelled_fragment_backing_stays_charged_until_physical_free,
    Cancelled,
    arc_bytes::<AgentLiveFragment>()
);

physical_free_case!(
    evicted_fragment_backing_stays_charged_until_physical_free,
    Evicted,
    arc_bytes::<AgentLiveFragment>()
);
