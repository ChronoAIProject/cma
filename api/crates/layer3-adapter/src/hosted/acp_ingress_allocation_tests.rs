use super::typed::TypedAdmission;
use crate::allocation_probe::AllocationProbe;
use crate::hosted::{retained_json::JsonPools, runtime_decode};
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use serde_json::{json, Value};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let plan = |n| AllocationPlan::new("typed allocation fixture", &[n]).unwrap();
    let bounds = ClassBounds::new(
        root.capacity_bytes(),
        plan(8 * 1024 * 1024),
        plan(64 * 1024 * 1024),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

#[test]
fn isolated_typed_allocation_contract() {
    const TEST: &str = "hosted::acp::ingress::allocation_tests::isolated_typed_allocation_contract";
    if std::env::var_os("CMA_TYPED_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_TYPED_ALLOCATION_CHILD", "1")
            .env("RUST_MIN_STACK", "33554432")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        println!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for (name, value) in updates() {
        measured::<crate::SessionUpdate>(name, value);
    }
    measured::<crate::RequestPermissionRequest>(
        "permission",
        json!({
            "sessionId":"s".repeat(1024), "toolCall":{"toolCallId":"t", "title":"approve", "rawInput":{"nested":[null,true,{"x":"y"}]}},
            "options":[{"optionId":"yes".repeat(80),"name":"Allow","kind":"allow_once"}],
        }),
    );
    measured::<crate::PromptResponse>(
        "prompt_result",
        json!({
            "stopReason":"end_turn", "_meta":{"nested":[1,2,3]},
            "usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3},
        }),
    );
    let deep = (0..30).fold(json!("leaf"), |value, _| json!({"nested":[value]}));
    measured::<crate::SessionUpdate>(
        "deep_metadata",
        json!({"sessionUpdate":"session_info_update", "_meta":deep}),
    );
    measured::<crate::SessionUpdate>(
        "dense_containers",
        json!({"sessionUpdate":"tool_call", "toolCallId":"t", "title":"dense",
            "rawInput": vec![json!({"x":[0,1,2,3,4]}); 700]}),
    );
    measured::<crate::SessionUpdate>(
        "grouped_config",
        json!({"sessionUpdate":"config_option_update", "configOptions":[{
            "id":"model", "name":"Model", "type":"select", "currentValue":"a",
            "options":[{"group":"g", "name":"Group", "options":[
                {"value":"a", "name":"A", "_meta":{"nested":[{"key":[1,2,3]}]}},
                {"value":"bad", "name":false},
                {"value":"b", "name":"B", "description":[{"invalid":true}]}
            ]}]
        }]}),
    );
    measured::<crate::SessionUpdate>(
        "defaulted_config_union",
        json!({"sessionUpdate":"config_option_update", "configOptions":[{
            "id":"model", "name":"Model", "type":"select", "currentValue":"a",
            "options":[{"value":"a", "name":"A"}, {"group":"bad", "name":"Group"}]
        }, {"id":"boolean", "name":"Boolean", "type":"boolean", "currentValue":true}]}),
    );
    measured_result::<crate::SessionUpdate>(
        "malformed_content",
        json!({"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":["bad"]}}),
        false,
    );
    measured_result::<crate::RequestPermissionRequest>(
        "malformed_permission",
        json!({"sessionId":"s", "toolCall":{"toolCallId":"t", "title":"title"}, "options":false}),
        false,
    );
    measured_result::<crate::PromptResponse>(
        "malformed_result",
        json!({"stopReason":false}),
        false,
    );
    pressure_before_decode();
    pending_channel_allocations();
    acknowledgement_channel_allocations();
    outgoing_json_allocations();
    http_descriptor_allocations();
    handshake_pressure_before_preparation();
}

fn http_descriptor_allocations() {
    use agent_client_protocol_schema::v1::{HttpHeader, McpServer, McpServerHttp};
    for name in ["cma_channel", "cma_access", "cmaeg"] {
        for size in [16, 65_536] {
            for credential in [None, Some("synthetic".repeat(size))] {
                let endpoint = format!("https://example.test/{}", "x".repeat(size));
                let mut sizing = AllocationProbe::new();
                let plan = sizing
                    .track(|| {
                        super::identity::http_descriptor_plan(
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
    assert!(super::identity::http_descriptor_plan(usize::MAX, None).is_err());
    assert!(super::identity::http_descriptor_plan(1, Some(usize::MAX)).is_err());
}

fn handshake_pressure_before_preparation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = super::tests::fixture();
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
        super::super::tests::settings(None, false),
        &updates,
    ));
    assert!(matches!(warm, Err(crate::AdapterError::AtCapacity(_))));
    drop(warm);
    let settings = super::super::tests::settings(None, false);
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

fn outgoing_json_allocations() {
    for (name, value) in updates() {
        let update: crate::SessionUpdate = serde_json::from_value(value).unwrap();
        let (root, policy) = policy();
        let mut preflight = AllocationProbe::new();
        let storage = preflight
            .track(|| super::identity::json_storage(Some(&policy), &update))
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

fn pending_channel_allocations() {
    use super::identity::PendingCalls;
    let (root, policy) = policy();
    let mut warm = PendingCalls::new(Some(&policy)).unwrap();
    drop(warm.register("warm", false).unwrap());
    drop(warm);
    let mut replies = Vec::with_capacity(128);
    let mut probe = AllocationProbe::new();
    let mut calls = probe.track(|| PendingCalls::new(Some(&policy))).unwrap();
    for index in 0..128 {
        let id = format!("synthetic-{index}");
        replies.push(probe.track(|| calls.register(&id, false)).unwrap());
    }
    let admitted = root.capacity_bytes() - root.available_bytes();
    assert!(
        probe.peak() <= admitted,
        "RPC allocation peak {} > {admitted}",
        probe.peak()
    );
    eprintln!(
        "rpc requested_peak={} requested_retained={} admitted={admitted}",
        probe.peak(),
        probe.live()
    );
    drop(calls);
    assert!(probe.live() <= root.capacity_bytes() - root.available_bytes());
    assert!(
        probe.live() > 0,
        "receivers must still own their oneshot storage"
    );
    drop(replies);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

fn acknowledgement_channel_allocations() {
    let (root, policy) = policy();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source = crate::RuntimeOutputSource {
        stream_id: uuid::Uuid::from_u128(1).to_string(),
        cursor: 1,
        native_conversation_id: "conversation".repeat(85),
        payload_sha256: "a".repeat(64),
    };
    let (warm_tx, mut warm_rx) =
        crate::RuntimeOutputReceipt::admitted_channel(policy.clone(), 128).unwrap();
    let warm = crate::RuntimeOutputReceipt::new_admitted(source.clone(), warm_tx);
    runtime.block_on(warm.acknowledge()).unwrap();
    drop(runtime.block_on(warm_rx.recv()).unwrap());
    drop((warm, warm_rx));
    let mut values = Vec::with_capacity(128);
    let mut probe = AllocationProbe::new();
    let (sender, mut receiver) = probe
        .track(|| crate::RuntimeOutputReceipt::admitted_channel(policy, 128))
        .unwrap();
    let receipt = crate::RuntimeOutputReceipt::new_admitted(source, sender);
    for _ in 0..128 {
        probe
            .track(|| runtime.block_on(receipt.acknowledge()))
            .unwrap();
    }
    let admitted = root.capacity_bytes() - root.available_bytes();
    assert!(
        probe.peak() <= admitted,
        "ACK allocation peak {} > {admitted}",
        probe.peak()
    );
    eprintln!(
        "ack_queue requested_peak={} requested_retained={} admitted={admitted}",
        probe.peak(),
        probe.live()
    );
    for _ in 0..128 {
        values.push(probe.track(|| runtime.block_on(receiver.recv())).unwrap());
    }
    drop((receipt, receiver));
    assert!(probe.live() <= root.capacity_bytes() - root.available_bytes());
    drop(values);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

fn measured<T: serde::de::DeserializeOwned>(name: &'static str, value: Value) {
    measured_result::<T>(name, value, true);
}

fn measured_result<T: serde::de::DeserializeOwned>(name: &'static str, value: Value, valid: bool) {
    let (root, policy) = policy();
    // Semaphore infrastructure is process-lived and initialized outside the
    // allocation group for this decode, like the existing decoder fixtures.
    drop(TypedAdmission::new::<T>(&value, &policy).unwrap());
    let admission = TypedAdmission::new::<T>(&value, &policy).unwrap();
    let reserved_peak = root.capacity_bytes() - root.available_bytes();
    let mut probe = AllocationProbe::new();
    let decoded = probe.track(|| admission.decode::<T>(&value));
    assert_eq!(decoded.is_ok(), valid, "{name}: unexpected decode result");
    let retained = root.capacity_bytes() - root.available_bytes();
    assert!(
        probe.peak() <= reserved_peak,
        "{name}: peak {} > allowance {reserved_peak}",
        probe.peak()
    );
    assert!(
        probe.live() <= retained,
        "{name}: retained {} > allowance {retained}",
        probe.live()
    );
    eprintln!("typed={name} requested_peak={} requested_retained={} allocation_churn={} admitted_peak={reserved_peak} admitted_retained={retained}", probe.peak(), probe.live(), probe.allocated());
    drop(decoded);
    assert_eq!(probe.live(), 0, "{name}: storage leaked");
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

fn pressure_before_decode() {
    let (root, policy) = policy();
    let payload = serde_json::to_vec(&json!({"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"x".repeat(4 * 1024 * 1024 - 1024)}})).unwrap();
    let raw = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("raw fixture", &[payload.len() + 512]).unwrap(),
        )
        .unwrap()
        .initialize(|| payload.clone());
    let pools = JsonPools::new(32 * 1024 * 1024, 13 * 1024 * 1024).with_ownership(&policy);
    let frame = pools.decode(&raw).unwrap();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let blocked = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("held data", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    assert!(TypedAdmission::new::<crate::SessionUpdate>(&frame, &policy).is_err());
    let mut probe = AllocationProbe::new();
    let failure = probe.track(|| TypedAdmission::new::<crate::SessionUpdate>(&frame, &policy));
    assert!(failure.is_err());
    assert!(
        probe.peak() < 1024,
        "typed refusal allocated payload bytes: {}",
        probe.peak()
    );
    drop(failure);
    assert_eq!(probe.live(), 0);
    drop(probe);
    let mut baseline = AllocationProbe::new();
    let old = baseline
        .track(|| runtime_decode::decode::<crate::SessionUpdate>(&frame, true))
        .unwrap();
    assert!(
        baseline.peak() > 4 * 1024 * 1024 - 1024,
        "negative control did not perform the unadmitted decode"
    );
    eprintln!("pressure baseline_requested_peak={} new_refusal_before_typed_allocation=true wire_bytes={}", baseline.peak(), payload.len());
    drop(old);
    assert_eq!(baseline.live(), 0);
    drop((blocked, frame, raw));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

fn updates() -> Vec<(&'static str, Value)> {
    vec![
        (
            "message_text",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x".repeat(4 * 1024 * 1024 - 1024)}}),
        ),
        (
            "thought",
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}),
        ),
        (
            "user_image",
            json!({"sessionUpdate":"user_message_chunk","content":{"type":"image","data":"data","mimeType":"image/png"}}),
        ),
        (
            "audio",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"audio","data":"data","mimeType":"audio/wav"}}),
        ),
        (
            "resource_text",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"resource","resource":{"uri":"synthetic:resource","text":"contents"}}}),
        ),
        (
            "resource_blob",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"resource","resource":{"uri":"synthetic:resource","blob":"eA=="}}}),
        ),
        (
            "resource_link",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"resource_link","uri":"synthetic:resource","name":"resource"}}),
        ),
        (
            "tool_call",
            json!({"sessionUpdate":"tool_call","toolCallId":"call","title":"read","content":[{"type":"content","content":{"type":"text","text":"output"}},{"type":"diff","path":"/synthetic/file","oldText":"old","newText":"new"},{"type":"terminal","terminalId":"t"}],"rawInput":{"a":[1,2]},"rawOutput":[null,"value"],"locations":[{"path":"/synthetic/file","line":1}]}),
        ),
        (
            "tool_update",
            json!({"sessionUpdate":"tool_call_update","toolCallId":"call","title":null,"kind":"unknown-kind","content":[false,{"type":"content","content":{"type":"text","text":"output"}}],"_meta":{"valid":true}}),
        ),
        (
            "plan",
            json!({"sessionUpdate":"plan","entries":[{"content":"work","priority":"medium","status":"pending"}]}),
        ),
        (
            "commands",
            json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"inspect","description":"inspect state","input":{"hint":"path"}}]}),
        ),
        (
            "mode",
            json!({"sessionUpdate":"current_mode_update","currentModeId":"m".repeat(65536)}),
        ),
        (
            "config",
            json!({"sessionUpdate":"config_option_update","configOptions":[{"id":"model","name":"Model","type":"select","currentValue":"a","options":[{"value":"a","name":"A"}]}]}),
        ),
        (
            "metadata",
            json!({"sessionUpdate":"session_info_update","title":"title","_meta":{"test":[1,2,{"x":true}]}}),
        ),
        (
            "usage",
            json!({"sessionUpdate":"usage_update","used":4,"size":100}),
        ),
        (
            "notice",
            json!({"sessionUpdate":"notice","severity":"_custom","title":"notice","description":"detail"}),
        ),
        (
            "compaction",
            json!({"sessionUpdate":"compaction_update","compactionId":"c","status":"completed","summary":[{"type":"text","text":"summary"}],"error":null}),
        ),
        (
            "compaction_chunk",
            json!({"sessionUpdate":"compaction_summary_chunk","compactionId":"c","content":{"type":"text","text":"summary"}}),
        ),
    ]
}
