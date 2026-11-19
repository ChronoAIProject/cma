use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use futures_util::FutureExt;
use std::sync::atomic::AtomicUsize;

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let floor = AllocationPlan::checked("settings query floor", &[64 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    (root, policy)
}

struct Source {
    owner: IngressWorkScope,
    result_policy: IngressOwnershipPolicy,
    invoked: AtomicUsize,
}

impl AdmittedConversationSettingsSupport for Source {
    fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.owner.belongs_to(policy)
    }

    fn managed_context_receipt(
        &self,
        sequence: u64,
        _: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        AdmittedOperation::new(
            &self.result_policy,
            IngressClass::Data,
            AllocationPlan::checked("synthetic receipt", &[64])?,
            move |_| async move {
                self.invoked.fetch_add(1, Ordering::Relaxed);
                // Fixed capacity avoids any old/new growth overlap.
                let mut receipt = String::with_capacity(64);
                use std::fmt::Write;
                write!(&mut receipt, "{sequence:064x}").unwrap();
                Ok(Some(receipt))
            },
        )
    }
}

fn source(policy: &IngressOwnershipPolicy, result: &IngressOwnershipPolicy) -> Arc<Source> {
    Arc::new(Source {
        owner: IngressWorkScope::new(
            policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("source fixture owner", &[4096]).unwrap(),
                )
                .unwrap(),
        ),
        result_policy: result.clone(),
        invoked: AtomicUsize::new(0),
    })
}

#[test]
fn missing_source_returns_owned_negative_observations() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let support = sender
        .system_prompt_supported()
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    let ready = sender
        .managed_context_may_be_ready()
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    let receipt = sender
        .managed_context_receipt(1)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    let transport = sender
        .managed_transport_receipt(1)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert!(!*support && !*ready);
    assert!(receipt.is_none() && transport.is_none());
    assert!(support.belongs_to(&policy) && ready.belongs_to(&policy));
    assert!(receipt.belongs_to(&policy) && transport.belongs_to(&policy));
    drop((sender, receiver));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop((support, ready, receipt, transport));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn unsupported_and_pressured_sources_refuse_without_future_or_result_allocation() {
    let (root, policy) = fixture();
    let source = source(&policy, &policy);
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let sender = sender.with_settings_support(source.clone()).unwrap();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "query pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    for _ in 0..128 {
        let unsupported = probe.track(|| sender.system_prompt_supported().unwrap_err());
        assert!(matches!(unsupported, AdmittedError::Unsupported(_)));
        let unsupported = probe.track(|| sender.managed_context_may_be_ready().unwrap_err());
        assert!(matches!(unsupported, AdmittedError::Unsupported(_)));
        let unsupported = probe.track(|| sender.managed_transport_receipt(1).unwrap_err());
        assert!(matches!(unsupported, AdmittedError::Unsupported(_)));
        let refused = probe.track(|| sender.managed_context_receipt(1).unwrap_err());
        assert!(matches!(
            refused.refusal(),
            Some(AdmissionRefusal::AtCapacity { .. })
        ));
    }
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.invoked.load(Ordering::Relaxed), 0);
    drop((probe, pressure, sender, receiver, source));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn receipt_keeps_sequence_and_storage_after_channel_and_source_drop() {
    let (root, policy) = fixture();
    let source = source(&policy, &policy);
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let sender = sender.with_settings_support(source.clone()).unwrap();
    let operation = sender.managed_context_receipt(23).unwrap();
    assert_eq!(source.invoked.load(Ordering::Relaxed), 0);
    let receipt = operation.now_or_never().unwrap().unwrap();
    assert_eq!(source.invoked.load(Ordering::Relaxed), 1);
    assert_eq!(receipt.as_deref(), Some(format!("{:064x}", 23).as_str()));
    drop((sender, receiver, source));
    assert!(receipt.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(receipt);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn foreign_source_and_foreign_operation_refuse_before_source_polling() {
    let (root, policy) = fixture();
    let (foreign_root, foreign) = fixture();
    for source_owner in [&policy, &foreign] {
        let source = source(source_owner, &foreign);
        let (sender, receiver) =
            AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
        let sender = sender.with_settings_support(source.clone());
        if std::ptr::eq(source_owner, &policy) {
            let sender = sender.unwrap();
            assert!(matches!(
                sender.managed_context_receipt(1).unwrap_err().refusal(),
                Some(AdmissionRefusal::InvalidPlan { .. })
            ));
        } else {
            assert!(matches!(sender, Err(AdmissionRefusal::InvalidPlan { .. })));
        }
        assert_eq!(source.invoked.load(Ordering::Relaxed), 0);
        drop((receiver, source));
        assert_eq!(root.available_bytes(), root.capacity_bytes());
        assert_eq!(
            foreign_root.available_bytes(),
            foreign_root.capacity_bytes()
        );
    }
}
