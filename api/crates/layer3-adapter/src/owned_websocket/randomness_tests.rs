use super::SystemRandomSource;
use crate::{
    allocation_probe::AllocationProbe,
    ingress_budget::{
        AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
    },
    AdapterError,
};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let floor = AllocationPlan::new("fixture_progress", &[64 * 1024]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

#[test]
fn fresh_thread_first_fill_and_repeated_sources_fit_their_owned_metadata() {
    let (peak, plan) = std::thread::spawn(|| {
        let (root, policy) = policy();
        let mut probe = AllocationProbe::new();
        let plan = SystemRandomSource::allocation_plan()
            .unwrap()
            .requested_bytes();
        for _ in 0..3 {
            let mut source = probe.track(|| SystemRandomSource::new(&policy).unwrap());
            assert_eq!(root.capacity_bytes() - root.available_bytes(), plan);
            for length in [16, 4, 0, 32] {
                let mut output = [0; 32];
                probe.track(|| source.fill(&mut output[..length]).unwrap());
            }
            assert!(probe.peak() <= plan);
            drop(source);
            assert_eq!(probe.live(), 0);
            assert_eq!(root.available_bytes(), root.capacity_bytes());
        }
        (probe.peak(), plan)
    })
    .join()
    .unwrap();
    eprintln!("entropy_source_peak={peak} plan={plan}");
}

#[test]
fn the_source_keeps_custody_across_threads_and_retained_results() {
    fn traits<T: Send + Sync + Unpin>() {}
    traits::<SystemRandomSource>();
    let (root, policy) = policy();
    let mut probe = AllocationProbe::new();
    let mut source = probe.track(|| SystemRandomSource::new(&policy).unwrap());
    let charged = source.requested_bytes();
    for _ in 0..2 {
        let result = std::thread::spawn(move || {
            source.fill(&mut [0; 16]).unwrap();
            source.fill(&mut [0; 4]).unwrap();
            source
        });
        source = result.join().unwrap();
        assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
        assert!(probe.live() > 0);
    }
    drop(source);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn entropy_uses_control_capacity_under_full_data_and_transform_pressure() {
    let (root, policy) = policy();
    let held: Vec<_> = [IngressClass::Data, IngressClass::Transformation]
        .into_iter()
        .map(|class| {
            policy
                .reserve(
                    class,
                    AllocationPlan::new(
                        "fixture_pressure",
                        &[policy.budget(class).available_bytes()],
                    )
                    .unwrap(),
                )
                .unwrap()
        })
        .collect();
    let mut source = SystemRandomSource::new(&policy).unwrap();
    source.fill(&mut [0; 16]).unwrap();
    source.fill(&mut [0; 4]).unwrap();
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        0
    );
    drop(source);
    drop(held);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn source_construction_refuses_full_control_capacity_before_owner_allocation() {
    let (root, policy) = policy();
    let held = policy
        .reserve(
            IngressClass::Control,
            AllocationPlan::new(
                "fixture_pressure",
                &[policy.budget(IngressClass::Control).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let before = root.available_bytes();
    assert!(matches!(
        SystemRandomSource::new(&policy),
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(root.available_bytes(), before);
    drop(held);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn boxed_source_keeps_its_allocation_and_custody_until_final_drop() {
    trait Source: Send + Sync {
        fn fill(&mut self, output: &mut [u8]);
    }
    impl Source for SystemRandomSource {
        fn fill(&mut self, output: &mut [u8]) {
            SystemRandomSource::fill(self, output).unwrap();
        }
    }
    let (root, policy) = policy();
    let plan = SystemRandomSource::allocation_plan()
        .unwrap()
        .requested_bytes();
    let mut probe = AllocationProbe::new();
    let mut source: Box<dyn Source> = probe.track(|| {
        let source = SystemRandomSource::new(&policy).unwrap();
        Box::new(source)
    });
    probe.track(|| source.fill(&mut [0; 16]));
    probe.track(|| source.fill(&mut [0; 4]));
    assert!(probe.live() >= std::mem::size_of::<SystemRandomSource>());
    assert!(probe.peak() <= plan);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), plan);
    eprintln!("boxed_entropy_source_peak={} plan={plan}", probe.peak());
    drop(source);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn fills_on_fresh_destination_threads_do_not_leave_requested_heap_storage() {
    let (root, policy) = policy();
    let mut source = SystemRandomSource::new(&policy).unwrap();
    for _ in 0..2 {
        source = std::thread::spawn(move || {
            let mut probe = AllocationProbe::new();
            probe.track(|| source.fill(&mut [0; 16]).unwrap());
            probe.track(|| source.fill(&mut [0; 4]).unwrap());
            assert_eq!(probe.peak(), 0);
            assert_eq!(probe.live(), 0);
            source
        })
        .join()
        .unwrap();
    }
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        source.requested_bytes()
    );
    drop(source);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
