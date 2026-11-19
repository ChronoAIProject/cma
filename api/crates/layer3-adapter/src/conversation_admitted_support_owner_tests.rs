use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{sizing, ClassBounds, IngressBudget};
use futures_util::FutureExt;
use std::sync::atomic::AtomicUsize;

struct InlineSource {
    policy: IngressOwnershipPolicy,
    payload: [u8; 512],
    invoked: AtomicUsize,
}

impl AdmittedConversationSettingsSupport for InlineSource {
    fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.policy.shares_root(policy)
    }

    fn system_prompt_supported(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        AdmittedOperation::new(
            policy,
            IngressClass::Data,
            AllocationPlan::checked("inline support query", &[0])?,
            |_| async {
                self.invoked.fetch_add(1, Ordering::Relaxed);
                Ok(self.payload[511] == 31)
            },
        )
    }
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let floor = AllocationPlan::checked("source ownership floor", &[64 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

fn source(
    storage: &IngressOwnershipPolicy,
    declared: &IngressOwnershipPolicy,
) -> SharedOwned<InlineSource> {
    storage
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "inline support source",
                &[sizing::shared_owner_bytes::<InlineSource>()],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| InlineSource {
            policy: declared.clone(),
            payload: [31; 512],
            invoked: AtomicUsize::new(0),
        })
}

#[test]
fn sender_clones_retain_source_without_allocating_an_erasure_wrapper() {
    let (root, policy) = policy();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let mut probe = AllocationProbe::new();
    let source = probe.track(|| source(&policy, &policy));
    let bytes = source.requested_bytes();
    assert!(probe.live() >= 512 && probe.live() <= bytes);
    let before = probe.allocated();
    let sender = probe.track(|| sender.with_owned_settings_support(source).unwrap());
    let last = probe.track(|| sender.clone());
    assert_eq!(probe.allocated(), before);
    drop((sender, receiver));
    let result = last
        .system_prompt_supported()
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert!(*result);
    drop(result);
    assert!(probe.live() >= 512);
    assert!(root.capacity_bytes() - root.available_bytes() >= bytes);
    drop(last);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn outer_and_declared_source_ownership_must_both_match_before_query() {
    let (root, policy) = policy();
    let (foreign_root, foreign) = self::policy();
    for (storage, declared) in [
        (&foreign, &policy),
        (&policy, &foreign),
        (&foreign, &foreign),
    ] {
        let source = source(storage, declared);
        let (sender, receiver) =
            AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
        let mut probe = AllocationProbe::new();
        let result = probe.track(|| sender.with_owned_settings_support(source.clone()));
        assert!(matches!(result, Err(AdmissionRefusal::InvalidPlan { .. })));
        assert_eq!(probe.allocated(), 0);
        assert_eq!(source.invoked.load(Ordering::Relaxed), 0);
        drop((result, receiver, source));
        assert_eq!(root.available_bytes(), root.capacity_bytes());
        assert_eq!(
            foreign_root.available_bytes(),
            foreign_root.capacity_bytes()
        );
    }
}

#[test]
fn isolated_type_erasure_preserves_payload_identity_and_physical_deallocation_custody() {
    const TEST: &str = "conversation_channel::admitted::support::owner_tests::isolated_type_erasure_preserves_payload_identity_and_physical_deallocation_custody";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = policy();
    let mut probe = AllocationProbe::new();
    let (source, pause) = probe.pause_deallocation_containing(
        deallocation::arc_allocation_bytes::<InlineSource>(),
        || source(&policy, &policy),
        |source| (&**source as *const InlineSource) as usize,
    );
    let address = (&*source as *const InlineSource) as usize;
    let bytes = source.requested_bytes();
    let erased: SharedOwned<dyn AdmittedConversationSettingsSupport> = source.into();
    assert_eq!(
        (&*erased as *const dyn AdmittedConversationSettingsSupport) as *const () as usize,
        address
    );
    let last = erased.clone();
    assert!(erased.ptr_eq(&last));
    drop(erased);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), bytes);
    assert!(
        pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || {
            drop(last);
        })
    );
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
