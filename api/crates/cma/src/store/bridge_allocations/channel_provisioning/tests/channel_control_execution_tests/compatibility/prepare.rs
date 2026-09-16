use super::*;
use protocol::{ReaderCase, Scenario};
use serde_json::Value;

pub(super) async fn scenario() {
    let run_id = std::env::var("CHRONO_TEST_COMPAT_RUN").unwrap();
    assert!(run_id.starts_with("cma-control-compat-"));
    let mode = std::env::var("CHRONO_TEST_COMPAT_MODE").unwrap();
    assert!(matches!(
        mode.as_str(),
        "readers"
            | "capacity"
            | "held-interrupt"
            | "held-steer"
            | "released-interrupt"
            | "released-steer"
            | "claimed"
            | "expired-claim"
            | "unknown-write"
            | "completed"
            | "later-turn"
            | "consent-reenabled"
            | "canonical-disagreement"
    ));
    let c = ControlFixture::new().await;
    c.f.emit_assistant_text(
        "compat-partial",
        "retained partial output across old and new workers",
    )
    .await;
    c.f.emit_assistant_text("compat-boundary", "continuing the original source")
        .await;
    c.f.await_assistant_text("retained partial output across old and new workers")
        .await;
    c.f.resources
        .reconcile_command(&c.f.command.receipt.envelope.command_id)
        .await
        .unwrap();
    let steer = mode.contains("steer") || mode == "unknown-write";
    let text = if steer {
        format!(
            "CMA-CONTROL/1 steer {}\nKeep the current source and narrow the scope.",
            c.reference.reference
        )
    } else {
        format!("CMA-CONTROL/1 cancel {}", c.reference.reference)
    };
    let body = signed::body("compat-control", 200, &text, true);
    let write = signed::write(
        &c.f.client,
        &c.f.database,
        &c.f.bridges,
        &c.f.inbox.pin.bridge_id,
        &body,
    )
    .await;
    let request = ChannelControlReservation::prepare(
        c.proof(ChannelControlWrite::from_verified(write).unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    let held = c.store.reserve(request).await.unwrap();
    let inbox = crate::store::MongoCallbackInbox::open(
        &c.f.bridges,
        &c.f.database,
        "fixture-deployment",
        &custody_key(),
    )
    .await
    .unwrap();
    let expected = inbox
        .recover_receipt(
            &signed::write(
                &c.f.client,
                &c.f.database,
                &c.f.bridges,
                &c.f.inbox.pin.bridge_id,
                &body,
            )
            .await,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(expected, CallbackAcceptance::ControlAccepted(_)));
    let mut cases = vec![ReaderCase { body, expected }];
    if mode == "readers" {
        for (callback, provider, text) in [
            ("compat-message", 201, "ordinary retained input"),
            ("compat-refused", 202, "CMA-CONTROL/9 cancel ccr_refused"),
        ] {
            let body = signed::body(callback, provider, text, false);
            let expected = inbox
                .accept(
                    signed::write(
                        &c.f.client,
                        &c.f.database,
                        &c.f.bridges,
                        &c.f.inbox.pin.bridge_id,
                        &body,
                    )
                    .await,
                )
                .await
                .unwrap();
            cases.push(ReaderCase { body, expected });
        }
    }
    let agent =
        c.f.resources
            .get_agent(&held.command.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
    let scenario = Scenario {
        schema_version: 1,
        run_id,
        database: c.f.database.clone(),
        mode: mode.clone(),
        bridge_id: c.f.inbox.pin.bridge_id.clone(),
        source_command_id: c.f.command.receipt.envelope.command_id.clone(),
        control_command_id: held.sidecar.command_id.clone(),
        source_turn: held.command.target_turn.unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
        runtime_conversation: agent.conversation.conversation_id.unwrap(),
        cases,
    };
    let marker = doc! {"_id":"scenario","run_id":&scenario.run_id,"mode":&mode,
    "source_receipt":record_bson(&c.f.command.receipt).unwrap(),"control_receipt":record_bson(&held.command.receipt).unwrap(),
    "intent":record_bson(&held.sidecar.intent).unwrap(),"original_logs":record_bson(&c.f.resources.agent_log(&agent.id,0,100).await.unwrap()).unwrap()};
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("compatibility_scenarios")
        .insert_one(marker)
        .await
        .unwrap();
    let path = std::env::var("CHRONO_TEST_COMPAT_SCENARIO_PATH").unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&scenario).unwrap()).unwrap();
    let h = Harness::open().await;
    match mode.as_str() {
        "released-interrupt" | "released-steer" => worker::service(&h)
            .release_held(&held.command, &worker::execution(&h).operations)
            .await
            .unwrap(),
        "claimed" | "expired-claim" => {
            worker::service(&h)
                .release_held(&held.command, &worker::execution(&h).operations)
                .await
                .unwrap();
            let now = std::time::SystemTime::now();
            let claimed = h
                .resources
                .claim_next_command(CommandClaimRequest {
                    agent_id: agent.id.clone(),
                    holder: "departed-compatible-worker".into(),
                    now: layer2_core::rfc3339_at(now),
                    expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(2)),
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(claimed.receipt, held.command.receipt);
            if mode == "expired-claim" {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        "unknown-write" | "completed" => {
            AgentCommandWorker::new(
                h.resources.clone(),
                worker::execution(&h),
                "compatible-before-restart".into(),
            )
            .unwrap()
            .run_one(&scenario.control_command_id)
            .await
            .unwrap();
            assert_eq!(h.effect_count().await, 1);
            assert_eq!(
                h.command().await.work.observation.unwrap().delivery,
                if mode == "unknown-write" {
                    AgentOperationDelivery::RecoveryRequired
                } else {
                    AgentOperationDelivery::WriteConfirmed
                }
            );
        }
        "later-turn" => {
            c.f.finish().await;
            c.f.resources
                .reconcile_command(&scenario.source_command_id)
                .await
                .unwrap();
            layer2_core::sandbox::AgentOperationService::new(
                c.f.resources.clone(),
                c.f.registry.clone(),
            )
            .execute_command(
                &caller().caller,
                &agent.id,
                AgentCommand::message("a later owner's turn must never receive the old control"),
            )
            .await
            .unwrap();
        }
        "consent-reenabled" => {
            consent(
                &c,
                1,
                SourceControlAction::RevokeSenderControls,
                "compat-revoke",
            )
            .await;
            consent(
                &c,
                2,
                SourceControlAction::AllowSenderCancelAndSteer,
                "compat-reenable",
            )
            .await;
        }
        "canonical-disagreement" => {
            let decisions =
                c.f.client
                    .database(&c.f.database)
                    .collection::<Document>("bridge_callback_dispositions");
            let mut row = decisions
                .find_one(doc! {"kind":"provider","record.version":2_i32})
                .await
                .unwrap()
                .unwrap();
            let mut record: Value =
                mongodb::bson::deserialize_from_bson(row.get("record").unwrap().clone()).unwrap();
            record["receipt"]["receipt"]["intent_id"] = json!(format!("cci_{}", "e".repeat(64)));
            row.insert("record", record_bson(&record).unwrap());
            row.insert(
                "record_sha256",
                layer2_core::profiles::content_digest(&record).unwrap(),
            );
            decisions
                .replace_one(doc! {"_id":row.get_str("_id").unwrap()}, row.clone())
                .await
                .unwrap();
        }
        _ => {}
    }
    h.record_result("prepared",json!({"effects":h.effect_count().await,"status":h.command().await.status,"fresh_admission_enabled":false})).await;
    // The owned runner retains this database across real process boundaries.
    // Dropping the source process must never replace its recorded runtime identity.
}

async fn consent(c: &ControlFixture, revision: u64, action: SourceControlAction, key: &str) {
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
    BridgeSourceControlService::new(c.f.invokes.clone(), c.invokes.clone())
        .submit(
            &caller(),
            None,
            &c.f.grant.id,
            key,
            SourceControlRequest {
                expected_bridge_revision: bridge.revision,
                expected_grant_revision: 1,
                expected_policy_revision: revision,
                action,
            },
        )
        .await
        .unwrap();
}
