use super::super::resource_revocations_tests::Service;
use super::Scenario;
use axum::Extension;
use layer2_core::resource_access::{execution::ExecutionError, revocations::ResourceRevocations};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("revocation_") {
        return None;
    }
    let (router, _, _) = super::super::resource_access_tests::app().await;
    let subject = if name == "revocation_outsider" {
        "outsider"
    } else {
        "owner-a"
    };
    let context = super::super::resource_access_tests::context(subject);
    let mut service = if name.starts_with("revocation_effective") {
        Service::completed()
    } else {
        Service::new()
    };
    if name.starts_with("revocation_effective_sandbox") {
        service.value.selector = layer2_core::resource_access::ResourceSelector::Sandbox {
            sandbox_id: layer2_core::SandboxId("sandbox".into()),
        };
        let receipt = service.value.receipt.as_mut().unwrap();
        receipt.share.selector = service.value.selector.clone();
        if name.ends_with("set_mode") {
            receipt.share.revoked_at = None;
        }
    }
    if name == "revocation_evidence" {
        service.value.stage =
            layer2_core::resource_access::revocations::RevocationStage::EvidenceReceived;
        service.value.stopped = service.value.participants;
    }
    service.error = match name {
        "revocation_conflict" => Some(ExecutionError::IdempotencyConflict),
        "revocation_closing" => Some(ExecutionError::Closing {
            operation: serde_json::from_value(serde_json::json!("b".repeat(64))).unwrap(),
        }),
        "revocation_capacity" => Some(ExecutionError::Capacity),
        "revocation_unknown" => Some(ExecutionError::Unknown),
        _ => None,
    };
    let router = if name == "revocation_disabled" {
        router
    } else {
        router.layer(Extension(Arc::new(service) as Arc<dyn ResourceRevocations>))
    };
    Some(Scenario {
        router: router.layer(Extension(context.clone())).layer(Extension(
            crate::DelegatedCredential::new("request-credential".into()),
        )),
        subject: (name != "revocation_anonymous").then_some(subject),
        authorization: context.caller.authorization,
        symbols: Default::default(),
    })
}
