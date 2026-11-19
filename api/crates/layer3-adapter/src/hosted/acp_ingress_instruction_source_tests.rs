use super::super::tests::{fixture, Fixture};
use super::{next_request, reply};
use crate::hosted::managed_context::instructions_admission::tests::binding;
use crate::ingress_budget::{sizing, AllocationPlan, IngressClass, IngressWorkScope};
use crate::{AdapterError, PermissionLevel, ReasoningEffort};
use serde_json::json;
use std::sync::atomic::Ordering::SeqCst;

async fn choices(fixture: &Fixture) {
    for method in [
        "session/set_model",
        "session/set_config_option",
        "session/set_mode",
    ] {
        let request = next_request(fixture).await;
        assert_eq!(request["method"], method);
        reply(
            fixture,
            &request,
            json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]}),
        )
        .await;
    }
}

async fn failed_apply(fixture: &Fixture, request: &serde_json::Value) {
    let frame = json!({"id": request["id"], "error": {
        "code": -32603, "message": "Internal error",
        "data": {"details": "Cannot read properties of undefined (reading 'replace')"}
    }});
    let payload = serde_json::to_vec(&frame).unwrap();
    let owned = fixture
        .bridge
        .json_pools
        .as_ref()
        .unwrap()
        .decode(&payload)
        .unwrap();
    let pending = fixture
        .bridge
        .pending
        .lock()
        .await
        .remove(request["id"].as_str().unwrap())
        .unwrap();
    pending.answer.send(owned).unwrap();
}

#[tokio::test]
async fn instruction_recovery_uses_admitted_sources_and_preserves_uncertain_outcomes() {
    for fault in [0, 1, 7, 255] {
        let mut fixture = fixture();
        let root = fixture.root.clone();
        let (context, source) = binding(fault, "exact instructions", None);
        let (legacy, legacy_source) = crate::hosted::managed_context::writer_tests::binding(0);
        fixture.bridge.context_refresh = Some(if fault == 255 { legacy } else { context });
        fixture.bridge.instructions_support.store(1, SeqCst);
        fixture.activate().await;
        let scope = IngressWorkScope::new(
            fixture
                .policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "instruction recovery caller",
                        &[sizing::shared_owner_bytes::<()>()],
                    )
                    .unwrap(),
                )
                .unwrap(),
        );
        let peer = async {
            choices(&fixture).await;
            let apply = next_request(&fixture).await;
            assert_eq!(apply["method"], "cma/agent-instructions");
            assert_eq!(apply["params"]["systemPrompt"], "exact instructions");
            failed_apply(&fixture, &apply).await;
            if fault != 0 {
                return;
            }
            let list = next_request(&fixture).await;
            assert_eq!(list["method"], "session/list");
            reply(
                &fixture,
                &list,
                json!({"sessions":[{"sessionId":"conversation","cwd":"/synthetic"}]}),
            )
            .await;
            let load = next_request(&fixture).await;
            assert_eq!(load["method"], "session/load");
            assert_eq!(
                load["params"],
                json!({"sessionId":"conversation","cwd":"/synthetic","mcpServers":[]})
            );
            reply(&fixture, &load, json!({"sessionId":"conversation"})).await;
            choices(&fixture).await;
            let replay = next_request(&fixture).await;
            assert_eq!(replay["method"], apply["method"]);
            assert_eq!(replay["params"], apply["params"]);
            use sha2::{Digest, Sha256};
            reply(&fixture, &replay, json!({"applied":true,"sessionId":"conversation","sha256":format!("{:x}",Sha256::digest(b"exact instructions"))})).await;
        };
        let operation = scope
            .initialize_owned(|| async {
                fixture
                    .bridge
                    .configure_settings(
                        "conversation",
                        "synthetic-model",
                        ReasoningEffort::High,
                        PermissionLevel::Guarded,
                        Some("exact instructions"),
                    )
                    .await
            })
            .resolve_admitted_result(&fixture.policy, IngressClass::Data);
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(operation, peer)
        })
        .await
        .unwrap();
        assert_eq!(result.is_ok(), fault == 0, "fault={fault}: {result:?}");
        assert_eq!(
            legacy_source.status_reads.load(SeqCst),
            0,
            "enabled recovery fell back to legacy source"
        );
        assert_eq!(source.reads.load(SeqCst), usize::from(fault != 255));
        assert!(fixture.writes.lock().unwrap().is_empty());
        assert!(fixture.bridge.pending.lock().await.is_empty());
        assert!(fixture
            .bridge
            .matches_active_session("conversation")
            .await
            .unwrap());
        if let Err(error) = &result {
            let Some(AdapterError::Unavailable(detail)) = error.as_adapter() else {
                panic!("confirmation failure lost uncertainty: {error:?}")
            };
            assert!(detail.starts_with("pending Agent instructions are not confirmed: "));
            if fault == 7 {
                assert_eq!(
                    detail,
                    &format!(
                        "pending Agent instructions are not confirmed: runtime unavailable: {}",
                        "x".repeat(2048)
                    )
                );
            }
            let clone = error.clone();
            drop((fixture, scope, result, source, legacy_source));
            assert_eq!(
                root.capacity_bytes() - root.available_bytes(),
                clone.requested_bytes()
            );
            drop(clone);
        } else {
            drop((fixture, scope, result, source, legacy_source));
        }
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}
