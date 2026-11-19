use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    AdmittedError, AdmittedOperation, AllocationPlan, ClassBounds, IngressBudget, IngressClass,
    IngressOwnershipPolicy,
};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture() -> (
    IngressBudget,
    IngressOwnershipPolicy,
    RuntimeHandle,
    AgentSlot,
) {
    let root = IngressBudget::new(256 * 1024);
    let floor = AllocationPlan::checked("source fixture floor", &[16 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    (
        root,
        policy,
        RuntimeHandle("synthetic-runtime".into()),
        AgentSlot("synthetic-agent".into()),
    )
}

struct LegacySource(AtomicUsize);

#[async_trait::async_trait]
impl HostedSubscriptionSource for LegacySource {
    async fn credential(
        &self,
        _: &str,
        _: &RuntimeHandle,
        _: &AgentSlot,
        _: bool,
        _: Option<&str>,
    ) -> crate::Result<HostedSubscriptionCredential> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(HostedSubscriptionCredential::Warm)
    }
}

#[test]
fn unsupported_source_refuses_without_boxing_or_invoking_legacy_production() {
    let (root, policy, runtime, agent) = fixture();
    let source = LegacySource(AtomicUsize::new(0));
    let port: &dyn HostedSubscriptionSource = &source;
    let mut probe = AllocationProbe::new();
    let refused: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| {
            port.credential_admitted("synthetic-sandbox", &runtime, &agent, false, None, &policy)
                .unwrap_err()
        })
    });
    assert!(refused
        .iter()
        .all(|error| matches!(error, AdmittedError::Unsupported(_))));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.0.load(Ordering::Relaxed), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert!(matches!(
        port.credential("synthetic-sandbox", &runtime, &agent, false, None)
            .now_or_never()
            .unwrap()
            .unwrap(),
        HostedSubscriptionCredential::Warm
    ));
    assert_eq!(source.0.load(Ordering::Relaxed), 1);
}

struct AdmittedSource(AtomicUsize);

#[async_trait::async_trait]
impl HostedSubscriptionSource for AdmittedSource {
    fn credential_admitted<'a>(
        &'a self,
        sandbox: &'a str,
        runtime: &'a RuntimeHandle,
        agent: &'a AgentSlot,
        refresh: bool,
        previous: Option<&'a str>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, HostedSubscriptionCredential>, AdmittedError> {
        AdmittedOperation::new(
            policy,
            IngressClass::Data,
            AllocationPlan::checked("synthetic warm credential", &[0])?,
            move |_| async move {
                assert_eq!(
                    (sandbox, runtime.as_str(), agent.as_str(), refresh, previous),
                    (
                        "synthetic-sandbox",
                        "synthetic-runtime",
                        "synthetic-agent",
                        false,
                        None
                    )
                );
                self.0.fetch_add(1, Ordering::Relaxed);
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
        panic!("admitted source cannot fall back to legacy production")
    }
}

#[test]
fn admitted_source_keeps_arguments_and_output_owner_without_legacy_fallback() {
    let (root, policy, runtime, agent) = fixture();
    let source = AdmittedSource(AtomicUsize::new(0));
    let port: &dyn HostedSubscriptionSource = &source;
    let operation = port
        .credential_admitted("synthetic-sandbox", &runtime, &agent, false, None, &policy)
        .unwrap();
    assert_eq!(source.0.load(Ordering::Relaxed), 0);
    assert!(operation.belongs_to(&policy));
    let output = operation.now_or_never().unwrap().unwrap();
    assert_eq!(source.0.load(Ordering::Relaxed), 1);
    assert!(matches!(&*output, HostedSubscriptionCredential::Warm));
    assert!(output.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(output);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn unsupported_backend_exchange_refuses_without_boxing_or_transport_work() {
    use crate::hosted::{HostedBackend, InMemoryHostedBackend};
    use crate::ingress_budget::{sizing, IngressWorkScope};
    let (root, policy, runtime, _agent) = fixture();
    let backend = InMemoryHostedBackend::default();
    let port: &dyn HostedBackend = &backend;
    let scope = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "owned source request",
                    &[sizing::shared_owner_bytes::<()>()],
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let mut probe = AllocationProbe::new();
    let refused: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| {
            port.exchange_admitted(
                &runtime,
                scope.initialize_owned(|| crate::wire::RequestFrame::Resources),
                &policy,
            )
            .unwrap_err()
        })
    });
    assert!(refused
        .iter()
        .all(|error| matches!(error, AdmittedError::Unsupported(_))));
    assert_eq!(probe.allocated(), 0);
    assert!(backend.requests().await.is_empty());
    drop(scope);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
