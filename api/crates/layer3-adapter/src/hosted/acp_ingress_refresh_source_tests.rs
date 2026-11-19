use crate::hosted::acp::{
    channel::{Admitted, Mode},
    ingress::fixture::fixture,
};
use crate::hosted::managed_context::{guidance_admission::fixture as files, writer_tests};
use crate::hosted::update_relay::UpdateRelay;
use crate::ingress_budget::{
    sizing, AdmittedConversationSendOutcome as Outcome, AdmittedError, AllocationPlan,
    IngressClass, IngressWorkScope,
};
use crate::{AdapterError, ConversationInput, PermissionLevel, ReasoningEffort};
use serde_json::json;
use std::sync::atomic::Ordering::SeqCst;

#[tokio::test]
async fn admitted_refresh_channel_selects_sources_and_preserves_delivery_classification() {
    for fault in [0, 2, 11, 20, 21, 22, 23, 24, 25, 26, 255] {
        let mut fixture = fixture();
        let (binding, source) = files::binding(
            if (20..=26).contains(&fault) { 0 } else { fault },
            "Old managed\n\nOwner exact text  \n",
            None,
        );
        let (binding, tools) = if (24..=26).contains(&fault) {
            let (binding, source) =
                crate::hosted::managed_context::transport_admission::tests::attach(
                    binding,
                    fault == 25,
                    if fault == 26 { 5 } else { 0 },
                    None,
                );
            (binding, Some(source))
        } else {
            (binding, None)
        };
        let receipt = files::receipt(&binding, &fixture.policy);
        let (legacy, legacy_source) = writer_tests::binding(0);
        fixture.bridge.context_refresh = Some(if fault == 255 { legacy } else { binding });
        fixture.activate().await;
        fixture.bridge.recovering_context.store(1, SeqCst);
        if tools.is_some() {
            let payload = serde_json::to_vec(&json!({"params":{"sessionId":"conversation","update":{"sessionUpdate":"session_info_update","_meta":{"goal":null}}}})).unwrap();
            let frame =
                crate::hosted::retained_json::JsonFrame::decode_admitted(&payload, &fixture.policy)
                    .unwrap();
            fixture.bridge.observe_context_frame_goal(&frame).await;
        }
        let root = fixture.root;
        let policy = fixture.policy;
        let writes = fixture.writes;
        let bridge = Admitted::own(fixture.bridge, Some(&policy)).unwrap();
        let (updates, captured) = UpdateRelay::capture_admitted(&policy).unwrap();
        let input = Admitted::input(&bridge, &updates).await.unwrap();
        let request = IngressWorkScope::new(
            policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "synthetic refresh input",
                        &[4096, sizing::shared_owner_bytes::<ConversationInput>()],
                    )
                    .unwrap(),
                )
                .unwrap(),
        )
        .initialize_owned(|| ConversationInput::RefreshContext {
            service_origin: tools.as_ref().map(|_| "https://new.example.test".into()),
            operation_sequence: if fault == 22 { 2 } else { 1 },
            readiness_receipt: if fault == 21 {
                "wrong".into()
            } else {
                receipt.clone()
            },
            previous_guidance: vec!["Old managed".into()],
            working_directory: (fault == 23).then(|| "relative".into()),
            guidance: "New managed".into(),
            model: "synthetic-model".into(),
            reasoning_effort: ReasoningEffort::High,
            permission: PermissionLevel::Guarded,
        });
        let peer = async {
            if !matches!(fault, 0 | 20 | 24..=26) {
                return;
            }
            for method in [
                "session/close",
                "session/load",
                "session/set_model",
                "session/set_config_option",
                "session/set_mode",
            ] {
                let request = loop {
                    if let Some(frame) = writes.lock().unwrap().pop() {
                        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = &*frame else {
                            panic!("binary request")
                        };
                        break serde_json::from_slice::<serde_json::Value>(&bytes[1..]).unwrap();
                    }
                    tokio::task::yield_now().await;
                };
                assert_eq!(request["method"], method);
                assert_eq!(request["params"]["sessionId"], "conversation");
                let result = match method {
                    "session/load" => {
                        assert_eq!(request["params"]["cwd"], "/home/cma/work");
                        if tools.is_some() {
                            assert_eq!(
                                request["params"]["mcpServers"][0]["name"],
                                if fault == 25 { "cmaeg" } else { "cma_access" }
                            );
                        } else {
                            assert_eq!(request["params"]["mcpServers"], json!([]));
                        }
                        json!({"sessionId": if fault == 20 { "other" } else { "conversation" }})
                    }
                    "session/set_config_option" => {
                        json!({"configOptions":[{"id":"reasoning_effort","currentValue":"high"}]})
                    }
                    _ => json!({}),
                };
                let payload =
                    serde_json::to_vec(&json!({"id":request["id"],"result":result})).unwrap();
                let frame = bridge
                    .json_pools
                    .as_ref()
                    .unwrap()
                    .decode(&payload)
                    .unwrap();
                bridge
                    .pending
                    .lock()
                    .await
                    .remove(request["id"].as_str().unwrap())
                    .unwrap()
                    .answer
                    .send(frame)
                    .unwrap();
                if fault == 20 && method == "session/load" {
                    break;
                }
            }
        };
        let (outcome, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(input.send_classified(request), peer)
        })
        .await
        .unwrap();
        assert_eq!(
            source.writes.load(SeqCst),
            usize::from(matches!(fault, 0 | 2 | 20 | 24..=26))
        );
        assert_eq!(legacy_source.status_reads.load(SeqCst), 0);
        assert_eq!(
            bridge.recovering_context.load(SeqCst),
            u64::from(!matches!(fault, 0 | 24 | 25))
        );
        assert!(bridge.matches_active_session("conversation").await.unwrap());
        assert!(writes.lock().unwrap().is_empty());
        match (&outcome, fault) {
            (Outcome::WriteConfirmed, 0 | 24 | 25)
            | (Outcome::PreWriteFailure(AdmittedError::Unsupported(_)), 255) => {}
            (Outcome::PreWriteFailure(error), 21 | 22) => assert!(matches!(
                error.as_adapter(),
                Some(AdapterError::OperationIdentityConflict(_))
            )),
            (Outcome::PreWriteFailure(error), 23) => assert!(matches!(
                error.as_adapter(),
                Some(AdapterError::Unsupported(_))
            )),
            (Outcome::Ambiguous(error), 2) => assert!(
                matches!(error.as_adapter(), Some(AdapterError::Unavailable(detail)) if detail == "write ACK lost")
            ),
            (Outcome::Ambiguous(error), 11) => assert!(matches!(
                error.as_adapter(),
                Some(AdapterError::RuntimeLedgerHold(_))
            )),
            (Outcome::Ambiguous(error), 20 | 26) => assert!(
                matches!(error.as_adapter(), Some(AdapterError::Unavailable(detail)) if detail == "managed context refresh is not confirmed")
            ),
            _ => panic!("fault={fault}: {outcome:?}"),
        }
        if let Some(tools) = &tools {
            assert_eq!(tools.confirmations.load(SeqCst), 1);
        }
        let retained = outcome.clone();
        drop((outcome, input, updates, captured));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while bridge.strong_count() > 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop((
            bridge,
            tools,
            source,
            legacy_source,
            fixture.acknowledgements,
            writes,
        ));
        match &retained {
            Outcome::Ambiguous(error) | Outcome::PreWriteFailure(error) => assert_eq!(
                root.capacity_bytes() - root.available_bytes(),
                error.requested_bytes()
            ),
            _ => assert_eq!(root.available_bytes(), root.capacity_bytes()),
        }
        drop(retained);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}
