use super::{policy, updates};
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{AllocationPlan, IngressClass};

pub(super) fn configuration_request_allocations() {
    use crate::hosted::acp_settings::{ModelChoice, SettingsRequest};
    use serde_json::json;
    let session = "s".repeat(1024);
    let model = "m".repeat(256);
    let prompt = "p".repeat(32 * 1024);
    for effort in [
        crate::ReasoningEffort::Minimal,
        crate::ReasoningEffort::XHigh,
    ] {
        let requests = [
            (
                SettingsRequest::Support { action: "support" },
                json!({"action":"support"}),
            ),
            (
                SettingsRequest::Model {
                    session_id: &session,
                    model_id: ModelChoice(&model, effort),
                },
                json!({"sessionId":session,"modelId":format!("{model}[{}]", effort.as_str())}),
            ),
            (
                SettingsRequest::Effort {
                    session_id: &session,
                    config_id: "reasoning_effort",
                    value: effort.as_str(),
                },
                json!({"sessionId":session,"configId":"reasoning_effort","value":effort.as_str()}),
            ),
            (
                SettingsRequest::Mode {
                    session_id: &session,
                    mode_id: "agent-full-access",
                },
                json!({"sessionId":session,"modeId":"agent-full-access"}),
            ),
            (
                SettingsRequest::Instructions {
                    action: "apply",
                    session_id: &session,
                    system_prompt: &prompt,
                },
                json!({"action":"apply","sessionId":session,"systemPrompt":prompt}),
            ),
        ];
        for (index, (request, expected)) in requests.into_iter().enumerate() {
            let (root, policy) = policy();
            let mut sizing = AllocationProbe::new();
            let storage = sizing
                .track(|| super::super::identity::json_storage(Some(&policy), &request))
                .unwrap()
                .unwrap();
            assert_eq!(sizing.peak(), 0);
            drop(sizing);
            let mut probe = AllocationProbe::new();
            let actual = probe.track(|| serde_json::to_value(&request)).unwrap();
            assert_eq!(actual, expected);
            assert!(probe.peak() <= storage.requested_bytes());
            eprintln!(
                "configuration_request={index} requested_peak={} retained={} admitted={}",
                probe.peak(),
                probe.live(),
                storage.requested_bytes()
            );
            drop(actual);
            assert_eq!(probe.live(), 0);
            drop(storage);
            root.assert_idle();
        }
    }
}

pub(super) fn configuration_pressure_before_request() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = super::super::tests::fixture();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("configuration preparation pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let prepare = || {
        fixture.bridge.configure_settings(
            "conversation",
            "synthetic-model",
            crate::ReasoningEffort::High,
            crate::PermissionLevel::Autonomous,
            None,
        )
    };
    let warm = runtime.block_on(prepare());
    assert!(matches!(warm, Err(crate::AdapterError::AtCapacity(_))));
    drop(warm);
    let pending = prepare();
    tokio::pin!(pending);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| runtime.block_on(pending.as_mut()));
    assert!(matches!(result, Err(crate::AdapterError::AtCapacity(_))));
    eprintln!("configuration_refusal requested_peak={}", probe.peak());
    assert!(probe.peak() <= "ingress allocation capacity exhausted".len());
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(runtime.block_on(async { fixture.bridge.pending.lock().await.is_empty() }));
    drop((result, blocker));
    assert_eq!(probe.live(), 0);
}

pub(super) fn steering_pressure_before_probe() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = super::super::tests::fixture();
    runtime.block_on(fixture.activate());
    let (updates, captured) =
        crate::hosted::update_relay::UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    // Permit the existing session copy, but no request-preparation allocation.
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("steering preparation pressure", &[available - 1024]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 1536]);
    let warm = runtime.block_on(
        fixture
            .bridge
            .drive(crate::ConversationInput::Steer(vec![]), &updates),
    );
    assert!(matches!(warm, Err(crate::AdapterError::AtCapacity(_))));
    drop(warm);
    let pending = fixture
        .bridge
        .drive(crate::ConversationInput::Steer(vec![]), &updates);
    tokio::pin!(pending);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| runtime.block_on(pending.as_mut()));
    assert!(matches!(result, Err(crate::AdapterError::AtCapacity(_))));
    eprintln!("steering_refusal requested_peak={}", probe.peak());
    assert!(
        probe.peak() <= "conversation".len() + "ingress allocation capacity exhausted".len(),
        "steering refused after {} requested bytes",
        probe.peak()
    );
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(captured.take().unwrap().is_none());
    assert!(runtime.block_on(async { fixture.bridge.pending.lock().await.is_empty() }));
    drop((result, blocker));
    assert_eq!(probe.live(), 0);
}

#[tokio::test]
async fn admitted_steering_preserves_support_probe_and_result() {
    use serde_json::json;
    for supported in [false, true] {
        let fixture = super::super::tests::fixture();
        fixture.activate().await;
        let (updates, captured) =
            crate::hosted::update_relay::UpdateRelay::capture_admitted(&fixture.policy).unwrap();
        let baseline = fixture.root.available_bytes();
        let peer = async {
            for step in 0..if supported { 2 } else { 1 } {
                let frame = loop {
                    if let Some(frame) = fixture.writes.lock().unwrap().pop() {
                        break frame;
                    }
                    tokio::task::yield_now().await;
                };
                let super::super::Message::Binary(bytes) = &*frame else {
                    panic!("expected conversation frame");
                };
                let request: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
                assert_eq!(request["method"], "cma/steer");
                let result = if step == 0 {
                    assert_eq!(request["params"], json!({"action":"support"}));
                    json!({"supported":supported,"version":1})
                } else {
                    assert_eq!(request["params"]["sessionId"], "conversation");
                    assert_eq!(request["params"]["prompt"][0]["text"], "steering fixture");
                    json!({"injected":true,"sessionId":"conversation"})
                };
                let payload =
                    serde_json::to_vec(&json!({"id":request["id"],"result":result})).unwrap();
                let response = fixture
                    .bridge
                    .json_pools
                    .as_ref()
                    .unwrap()
                    .decode(&payload)
                    .unwrap();
                fixture
                    .bridge
                    .receive_frame(response, &updates)
                    .await
                    .unwrap();
            }
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                fixture.bridge.drive(
                    crate::ConversationInput::Steer(vec![crate::ContentBlock::from(
                        "steering fixture"
                    )]),
                    &updates,
                ),
                peer
            )
        })
        .await
        .unwrap();
        if supported {
            result.unwrap();
        } else {
            assert!(matches!(result, Err(crate::AdapterError::Unsupported(_))));
        }
        assert!(captured.take().unwrap().is_none());
        assert!(fixture.bridge.pending.lock().await.is_empty());
        assert_eq!(fixture.root.available_bytes(), baseline);
    }
}

pub(super) fn bridge_identity_allocations() {
    use sha2::{Digest, Sha256};
    let (root, policy) = policy();
    let storage = super::super::identity::bridge_identity_storage(Some(&policy))
        .unwrap()
        .unwrap();
    let mut probe = AllocationProbe::new();
    let identities = probe.track(|| {
        (
            format!("{:x}", Sha256::digest(b"synthetic-agent")),
            uuid::Uuid::new_v4().to_string(),
        )
    });
    assert!(probe.peak() <= storage.requested_bytes());
    assert!(probe.live() >= 64 + 36);
    eprintln!(
        "bridge_identity requested_peak={} requested_retained={} admitted={}",
        probe.peak(),
        probe.live(),
        storage.requested_bytes()
    );
    drop(identities);
    assert_eq!(probe.live(), 0);
    drop(storage);
    root.assert_idle();
}

pub(super) fn http_descriptor_allocations() {
    use agent_client_protocol_schema::v1::{HttpHeader, McpServer, McpServerHttp};
    for name in ["cma_channel", "cma_access", "cmaeg"] {
        for size in [16, 65_536] {
            for credential in [None, Some("synthetic".repeat(size))] {
                let endpoint = format!("https://example.test/{}", "x".repeat(size));
                let mut sizing = AllocationProbe::new();
                let plan = sizing
                    .track(|| {
                        super::super::identity::http_descriptor_plan(
                            endpoint.len(),
                            credential.as_ref().map(String::len),
                        )
                    })
                    .unwrap();
                assert_eq!(sizing.peak(), 0);
                drop(sizing);
                let (root, policy) = policy();
                let storage = policy.reserve(IngressClass::Data, plan).unwrap();
                let mut probe = AllocationProbe::new();
                let (descriptor, copied) = probe.track(|| {
                    let mut server = McpServerHttp::new(name, &endpoint);
                    if let Some(credential) = &credential {
                        server = server.headers(vec![HttpHeader::new(
                            "Authorization",
                            format!("Bearer {credential}"),
                        )]);
                    }
                    let descriptor = serde_json::to_value(McpServer::Http(server)).unwrap();
                    let copied = descriptor.clone();
                    (descriptor, copied)
                });
                assert!(
                    probe.peak() <= storage.requested_bytes(),
                    "{name}: descriptor peak {} > {}",
                    probe.peak(),
                    storage.requested_bytes()
                );
                assert!(probe.live() > 0);
                eprintln!(
                    "descriptor={name} input_size={size} requested_peak={} retained={} admitted={}",
                    probe.peak(),
                    probe.live(),
                    storage.requested_bytes()
                );
                drop((descriptor, copied));
                assert_eq!(probe.live(), 0);
                drop(storage);
                root.assert_idle();
            }
        }
    }
    assert!(super::super::identity::http_descriptor_plan(usize::MAX, None).is_err());
    assert!(super::super::identity::http_descriptor_plan(1, Some(usize::MAX)).is_err());
}

pub(super) fn handshake_pressure_before_preparation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = super::super::tests::fixture();
    let (updates, captured) =
        crate::hosted::update_relay::UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("handshake pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    // Exclude Tokio/tracing's first-use bookkeeping from this payload fixture.
    let warm = runtime.block_on(fixture.bridge.handshake(
        None,
        "/fixture".into(),
        crate::hosted::acp::tests::settings(None, false),
        &updates,
        None,
    ));
    assert!(matches!(warm, Err(crate::AdapterError::AtCapacity(_))));
    drop(warm);
    let settings = crate::hosted::acp::tests::settings(None, false);
    let cwd = format!("/{}", "x".repeat(4095));
    let pending = fixture
        .bridge
        .handshake(None, cwd, settings, &updates, None);
    eprintln!("handshake_future_stack_bytes={}", size_of_val(&pending));
    // block_on boxes large futures; measure JSON preparation separately from
    // that fixed executor allocation by passing a stack-pinned reference.
    tokio::pin!(pending);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| runtime.block_on(pending.as_mut()));
    assert!(matches!(result, Err(crate::AdapterError::AtCapacity(_))));
    // The small refusal is allowed; initialize's map/tree must not exist.
    assert!(
        probe.peak() < 512,
        "handshake refused after {} bytes",
        probe.peak()
    );
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(captured.take().unwrap().is_none());
    drop((result, blocker));
    assert_eq!(probe.live(), 0);
}

pub(super) fn outgoing_json_allocations() {
    for (name, value) in updates() {
        let update: crate::SessionUpdate = serde_json::from_value(value).unwrap();
        let (root, policy) = policy();
        let mut preflight = AllocationProbe::new();
        let storage = preflight
            .track(|| super::super::identity::json_storage(Some(&policy), &update))
            .unwrap();
        assert_eq!(preflight.peak(), 0, "{name}: preflight must not allocate");
        drop(preflight);
        let mut actual = AllocationProbe::new();
        let frame =
            actual.track(|| serde_json::json!({"sessionId":"s".repeat(1024), "prompt":update}));
        let admitted = root.reserved_work_bytes();
        assert!(
            actual.peak() <= admitted,
            "{name}: JSON peak {} > {admitted}",
            actual.peak()
        );
        drop(frame);
        assert_eq!(actual.live(), 0);
        drop(storage);
        root.assert_idle();
    }
}
