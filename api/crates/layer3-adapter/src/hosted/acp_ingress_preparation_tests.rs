use super::{policy, updates};
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{AllocationPlan, IngressClass};

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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
                assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    ));
    assert!(matches!(warm, Err(crate::AdapterError::AtCapacity(_))));
    drop(warm);
    let settings = crate::hosted::acp::tests::settings(None, false);
    let cwd = format!("/{}", "x".repeat(4095));
    let pending = fixture.bridge.handshake(None, cwd, settings, &updates);
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
        let admitted = root.capacity_bytes() - root.available_bytes();
        assert!(
            actual.peak() <= admitted,
            "{name}: JSON peak {} > {admitted}",
            actual.peak()
        );
        drop(frame);
        assert_eq!(actual.live(), 0);
        drop(storage);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}
