use super::recovery_tests::{next_request, reply};
use super::tests::fixture;
use crate::hosted::managed_context::writer_tests::binding_with_tools;
use crate::ingress_budget::{AllocationPlan, IngressClass};
use crate::{PermissionLevel, ReasoningEffort};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn refresh_keeps_request_fields_and_same_identity_confirmation() {
    for same_identity in [true, false] {
        let mut fixture = fixture();
        let tools = json!([{"name":"synthetic","url":"https://example.test","headers":[]}]);
        let (binding, files) = binding_with_tools(0, tools.as_array().unwrap().clone());
        let receipt = binding.readiness_receipt("conversation", 1).await.unwrap();
        fixture.bridge.context_refresh = Some(binding);
        fixture.activate().await;
        let baseline = fixture.root.available_bytes();
        let peer = async {
            let request = next_request(&fixture).await;
            assert_eq!(request["method"], "session/close");
            assert_eq!(request["params"], json!({"sessionId":"conversation"}));
            reply(&fixture, &request, json!({})).await;
            let request = next_request(&fixture).await;
            assert_eq!(request["method"], "session/load");
            assert_eq!(
                request["params"],
                json!({"sessionId":"conversation",
                "cwd":"/home/cma/work","mcpServers":tools})
            );
            assert!(fixture
                .bridge
                .retained_history_deadline
                .observe()
                .latest()
                .is_some());
            reply(
                &fixture,
                &request,
                json!({"sessionId":if same_identity {"conversation"} else {"other"}}),
            )
            .await;
            if same_identity {
                for method in [
                    "session/set_model",
                    "session/set_config_option",
                    "session/set_mode",
                ] {
                    let request = next_request(&fixture).await;
                    assert_eq!(request["method"], method);
                    reply(
                        &fixture,
                        &request,
                        json!({"configOptions":[{
                        "id":"reasoning_effort","currentValue":"high"}]}),
                    )
                    .await;
                }
            }
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                fixture.bridge.refresh_context(
                    "conversation",
                    1,
                    &receipt,
                    "guidance",
                    &[],
                    "synthetic-model",
                    ReasoningEffort::High,
                    PermissionLevel::Guarded,
                    None,
                    None
                ),
                peer
            )
        })
        .await
        .unwrap();
        assert_eq!(result.is_ok(), same_identity);
        if !same_identity {
            assert!(matches!(result, Err(crate::AdapterError::Unavailable(_))));
        }
        assert_eq!(files.writes.load(Ordering::SeqCst), 1);
        assert!(fixture
            .bridge
            .retained_history_deadline
            .observe()
            .latest()
            .is_none());
        assert!(fixture.writes.lock().unwrap().is_empty());
        assert!(fixture
            .bridge
            .matches_active_session("conversation")
            .await
            .unwrap());
        assert_eq!(fixture.root.available_bytes(), baseline);
    }
}

pub(super) fn request_allocations() {
    use crate::{allocation_probe::AllocationProbe, hosted::acp_context_refresh::ContextRequest};
    let fixture = fixture();
    let session = "s".repeat(1024);
    let cwd = format!("/{}", "x".repeat(4095));
    let tools = json!([{"name":"synthetic","url":"x".repeat(65_536)}]);
    for (request, expected) in [
        (
            ContextRequest::Close {
                session_id: &session,
            },
            json!({"sessionId":session}),
        ),
        (
            ContextRequest::Load {
                session_id: &session,
                cwd: &cwd,
                mcp_servers: &tools,
            },
            json!({"sessionId":session,"cwd":cwd,"mcpServers":tools}),
        ),
    ] {
        drop(super::identity::json_storage(Some(&fixture.policy), &request).unwrap());
        let baseline = fixture.root.available_bytes();
        let mut probe = AllocationProbe::new();
        let (params, storage) = probe.track(|| {
            let storage = super::identity::json_storage(Some(&fixture.policy), &request).unwrap();
            (serde_json::to_value(&request).unwrap(), storage)
        });
        let charged = baseline - fixture.root.available_bytes();
        assert_eq!(params, expected);
        assert!(probe.peak() <= charged);
        eprintln!(
            "context_request requested_peak={} retained={} admitted={charged}",
            probe.peak(),
            probe.live()
        );
        drop(params);
        assert_eq!(probe.live(), 0);
        assert_eq!(baseline - fixture.root.available_bytes(), charged);
        drop(storage);
        assert_eq!(fixture.root.available_bytes(), baseline);
    }
}

#[tokio::test]
async fn refresh_refuses_tool_copy_before_backend_reads_or_writes() {
    let mut fixture = fixture();
    let (binding, files) = binding_with_tools(
        0,
        vec![json!({"name":"synthetic",
        "url":format!("https://example.test/{}", "x".repeat(65_536))})],
    );
    fixture.bridge.context_refresh = Some(binding);
    let baseline = fixture.root.available_bytes();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("refresh preparation pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let result = fixture
        .bridge
        .refresh_context(
            "conversation",
            1,
            "synthetic-receipt",
            "guidance",
            &[],
            "synthetic-model",
            ReasoningEffort::High,
            PermissionLevel::Guarded,
            None,
            None,
        )
        .await;
    assert!(
        matches!(result, Err(crate::AdapterError::AtCapacity(_))),
        "refresh must admit tools before backend work: {result:?}"
    );
    assert_eq!(files.status_reads.load(Ordering::SeqCst), 0);
    assert_eq!(files.writes.load(Ordering::SeqCst), 0);
    assert!(fixture.writes.lock().unwrap().is_empty());
    drop(blocker);
    assert_eq!(fixture.root.available_bytes(), baseline);
}
