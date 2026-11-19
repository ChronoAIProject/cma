use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::hosted::acp::ingress::fixture::fixture;
use crate::hosted::subscriptions::HostedSubscriptionSource;
use crate::hosted::update_relay::UpdateRelay;
use crate::{AgentSlot, RuntimeCredential, RuntimeHandle};
use futures_util::FutureExt;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering::SeqCst},
    Arc,
};

struct Source {
    calls: AtomicUsize,
    legacy: AtomicUsize,
    mode: u8,
    foreign: Option<IngressOwnershipPolicy>,
}

#[async_trait::async_trait]
impl HostedSubscriptionSource for Source {
    fn credential_admitted<'a>(
        &'a self,
        sandbox: &'a str,
        runtime: &'a RuntimeHandle,
        agent: &'a AgentSlot,
        _: bool,
        _: Option<&'a str>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, HostedSubscriptionCredential>, AdmittedError> {
        if self.mode == 5 {
            return Err(AdmittedError::Unsupported("synthetic legacy-only source"));
        }
        AdmittedOperation::new(
            self.foreign.as_ref().unwrap_or(policy),
            IngressClass::Data,
            AllocationPlan::checked(
                "synthetic credential",
                &[
                    4096,
                    "synthetic-account".len(),
                    "synthetic-plan".len(),
                    "synthetic upstream failure".len(),
                ],
            )?,
            move |_| async move {
                self.calls.fetch_add(1, SeqCst);
                assert_eq!(
                    (sandbox, runtime.as_str(), agent.as_str()),
                    ("synthetic-sandbox", "synthetic-runtime", "synthetic-agent")
                );
                if self.mode == 3 {
                    std::future::pending::<()>().await;
                }
                if self.mode == 2 {
                    return Err(crate::AdapterError::Unavailable(
                        "synthetic upstream failure".into(),
                    ));
                }
                if self.mode == 1 {
                    return Ok(HostedSubscriptionCredential::Warm);
                }
                Ok(HostedSubscriptionCredential::Connected {
                    access_token: RuntimeCredential::new("x".repeat(4096)),
                    account_id: "synthetic-account".into(),
                    plan_type: Some("synthetic-plan".into()),
                })
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
        self.legacy.fetch_add(1, SeqCst);
        Ok(if self.mode == 0 {
            HostedSubscriptionCredential::Connected {
                access_token: RuntimeCredential::new("x".repeat(4096)),
                account_id: "synthetic-account".into(),
                plan_type: Some("synthetic-plan".into()),
            }
        } else {
            HostedSubscriptionCredential::Warm
        })
    }
}

#[test]
fn credential_reply_refuses_before_source_allocation_while_legacy_still_allocates() {
    if deallocation::isolated("hosted::subscriptions::reply::tests::credential_reply_refuses_before_source_allocation_while_legacy_still_allocates") { return; }
    let fixture = fixture();
    let (binding, source) = binding(0, None);
    let id = json!(7);
    let params = json!({});
    let pressure = fixture
        .policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "credential pressure",
                &[fixture.policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .reply_admitted(&id, &params, &fixture.policy)
            .unwrap_err()
    });
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.calls.load(SeqCst), 0);
    let legacy = probe.track(|| binding.credential(&params).now_or_never().unwrap().unwrap());
    assert_eq!(
        legacy["tokens"]["accessToken"].as_str().unwrap().len(),
        4096
    );
    assert!(probe.peak() >= 4096);
    assert!(probe.live() >= 4096);
    assert_eq!(source.legacy.load(SeqCst), 1);
    drop(legacy);
    assert_eq!(probe.live(), 0);
    drop((probe, pressure));
}

#[test]
fn legacy_binding_preserves_the_original_source_without_an_admitted_port() {
    let (binding, source) = binding(5, None);
    let legacy = binding
        .credential(&json!({}))
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(legacy, json!({"mode":"warm"}));
    assert_eq!(source.legacy.load(SeqCst), 1);
    assert_eq!(source.calls.load(SeqCst), 0);
}

fn binding(
    mode: u8,
    foreign: Option<IngressOwnershipPolicy>,
) -> (HostedSubscriptionBinding, Arc<Source>) {
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        legacy: AtomicUsize::new(0),
        mode,
        foreign,
    });
    (
        HostedSubscriptionBinding {
            source: source.clone(),
            sandbox_id: "synthetic-sandbox".into(),
            runtime: RuntimeHandle("synthetic-runtime".into()),
            agent: AgentSlot("synthetic-agent".into()),
        },
        source,
    )
}

#[test]
fn isolated_credential_reply_retains_independent_json_and_never_invokes_legacy() {
    if deallocation::isolated("hosted::subscriptions::reply::tests::isolated_credential_reply_retains_independent_json_and_never_invokes_legacy") { return; }
    let fixture = fixture();
    let (binding, source) = binding(0, None);
    let id = json!("correlation");
    let params = json!({});
    // The shared policy owns its semaphore backing before this operation begins.
    drop(
        fixture
            .policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("policy backing", &[1]).unwrap(),
            )
            .unwrap(),
    );
    let baseline = fixture.root.available_bytes();
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        binding
            .reply_admitted(&id, &params, &fixture.policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    assert_eq!(source.calls.load(SeqCst), 1);
    assert_eq!(source.legacy.load(SeqCst), 0);
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], id);
    assert_eq!(result["result"]["mode"], "subscription");
    assert_eq!(
        result["result"]["tokens"]["accessToken"]
            .as_str()
            .unwrap()
            .len(),
        4096
    );
    assert_eq!(
        result["result"]["tokens"]["chatgptAccountId"],
        "synthetic-account"
    );
    assert_eq!(
        result["result"]["tokens"]["chatgptPlanType"],
        "synthetic-plan"
    );
    assert!(probe.live() >= 4096);
    assert!(probe.live() <= baseline - fixture.root.available_bytes());
    drop((binding, source));
    assert!(probe.live() >= 4096);
    drop(result);
    assert_eq!(probe.live(), 0);
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[test]
fn credential_reply_preserves_warm_rules_and_rejects_bad_identity_before_source() {
    let fixture = fixture();
    let (binding, source) = binding(1, None);
    let id = json!(7);
    let params = json!({});
    let warm = binding
        .reply_admitted(&id, &params, &fixture.policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(
        **warm,
        json!({"jsonrpc":"2.0", "id":7, "result":{"mode":"warm"}})
    );
    drop(warm);
    for params in [
        json!({"forceRefresh":true}),
        json!({"previousAccountId":"synthetic-account"}),
    ] {
        let error = binding
            .reply_admitted(&id, &params, &fixture.policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::InvalidPlan { .. })
        ));
    }
    assert_eq!(source.calls.load(SeqCst), 3);
    for params in [
        json!([]),
        json!({"previousAccountId":""}),
        json!({"previousAccountId":"x".repeat(257)}),
    ] {
        assert!(binding
            .reply_admitted(&id, &params, &fixture.policy)
            .is_err());
    }
    assert!(binding
        .reply_admitted(&json!("x".repeat(257)), &json!({}), &fixture.policy)
        .is_err());
    assert_eq!(source.calls.load(SeqCst), 3);
    assert_eq!(source.legacy.load(SeqCst), 0);
}

#[test]
fn credential_source_errors_and_cancelled_work_keep_their_owner_until_drop() {
    let fixture = fixture();
    let id = json!(7);
    let params = json!({});
    let baseline = fixture.root.available_bytes();
    let (binding, source) = binding(2, None);
    let error = binding
        .reply_admitted(&id, &params, &fixture.policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error.as_adapter(), Some(crate::AdapterError::Unavailable(text)) if text == "synthetic upstream failure")
    );
    assert!(error.belongs_to(&fixture.policy));
    drop((binding, source));
    assert!(fixture.root.available_bytes() < baseline);
    drop(error);
    assert_eq!(fixture.root.available_bytes(), baseline);

    let (binding, source) = self::binding(3, None);
    let mut operation = Box::pin(
        binding
            .reply_admitted(&id, &params, &fixture.policy)
            .unwrap(),
    );
    assert!(operation.as_mut().now_or_never().is_none());
    assert_eq!(source.calls.load(SeqCst), 1);
    drop(operation);
    assert_eq!(fixture.root.available_bytes(), baseline);
    let foreign = self::fixture();
    let (binding, source) = self::binding(0, Some(foreign.policy.clone()));
    let error = binding
        .reply_admitted(&id, &params, &fixture.policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(source.calls.load(SeqCst), 0);
    assert_eq!(source.legacy.load(SeqCst), 0);
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[tokio::test]
async fn actual_bridge_uses_admitted_credentials_and_retains_the_final_socket_frame() {
    let mut fixture = fixture();
    let (binding, source) = binding(0, None);
    fixture.bridge.subscription = Some(binding);
    let request = json!({"method":"cma/codex/credentials", "id":"call", "params":{}});
    let frame = JsonFrame::prepare(&request, Some(&fixture.policy)).unwrap();
    let (updates, capture) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    fixture.bridge.receive_frame(frame, &updates).await.unwrap();
    assert_eq!(source.calls.load(SeqCst), 1);
    assert_eq!(source.legacy.load(SeqCst), 0);
    let sent = fixture.writes.lock().unwrap().pop().unwrap();
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = &*sent else {
        panic!("binary reply expected")
    };
    let reply: Value = serde_json::from_slice(&bytes[1..]).unwrap();
    assert_eq!(reply["id"], "call");
    assert_eq!(
        reply["result"]["tokens"]["accessToken"]
            .as_str()
            .unwrap()
            .len(),
        4096
    );
    assert!(capture.take().unwrap().is_none());
    let root = fixture.root.clone();
    drop((fixture, updates, capture, reply, source));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(sent);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn actual_bridge_keeps_redacted_refusal_under_data_pressure() {
    for (mode, pressure) in [(0, true), (2, false), (5, false)] {
        let mut fixture = fixture();
        let (binding, source) = binding(mode, None);
        fixture.bridge.subscription = Some(binding);
        let request = json!({"method":"cma/codex/credentials", "id":"call", "params":{}});
        let frame = JsonFrame::prepare(&request, Some(&fixture.policy)).unwrap();
        let (updates, capture) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
        let pressure = pressure.then(|| {
            fixture
                .policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "credential pressure",
                        &[fixture.policy.budget(IngressClass::Data).available_bytes()],
                    )
                    .unwrap(),
                )
                .unwrap()
        });
        fixture.bridge.receive_frame(frame, &updates).await.unwrap();
        let sent = fixture.writes.lock().unwrap().pop().unwrap();
        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = &*sent else {
            panic!("binary reply expected")
        };
        let reply: Value = serde_json::from_slice(&bytes[1..]).unwrap();
        assert_eq!(
            reply,
            json!({"jsonrpc":"2.0", "id":"call", "error":{"code":-32001, "message":"Re-login with Codex subscription to continue"}})
        );
        assert_eq!(source.legacy.load(SeqCst), 0);
        assert_eq!(source.calls.load(SeqCst), usize::from(mode == 2));
        drop((pressure, sent, updates, capture));
    }
}
