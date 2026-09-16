use super::*;

pub(super) async fn exercise(h: &Harness) {
    assert!(!matches!(h.scenario.mode.as_str(), "capacity" | "readers"));
    let before = h.command().await;
    let sidecar: ChannelControlSidecar = mongodb::bson::deserialize_from_bson(
        h.control_row()
            .await
            .get("channel_control_admission")
            .unwrap()
            .clone(),
    )
    .unwrap();
    let marker = h
        .client
        .database(&h.scenario.database)
        .collection::<Document>("compatibility_scenarios")
        .find_one(doc! {"_id":"scenario"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record_bson(&before.receipt).unwrap(),
        marker["control_receipt"]
    );
    assert_eq!(record_bson(&sidecar.intent).unwrap(), marker["intent"]);
    assert_eq!(before.target_turn, Some(h.scenario.source_turn));
    let original_logs: Vec<layer2_core::sandbox::AgentLogEntry> =
        mongodb::bson::deserialize_from_bson(marker["original_logs"].clone()).unwrap();
    if let Some(lease) = &before.work.lease {
        if lease.expires_at > layer2_core::now_rfc3339() {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    let common = AgentCommandWorker::new(
        h.resources.clone(),
        worker::execution(h),
        "compatible-rollback-worker".into(),
    )
    .unwrap();
    for _ in 0..3 {
        common
            .run_one(&h.scenario.control_command_id)
            .await
            .unwrap();
    }
    let after = h.command().await;
    assert_eq!(after.receipt, before.receipt);
    assert_eq!(after.target_turn, before.target_turn);
    let row = h.control_row().await;
    let sidecar: ChannelControlSidecar =
        mongodb::bson::deserialize_from_bson(row.get("channel_control_admission").unwrap().clone())
            .unwrap();
    assert_eq!(record_bson(&sidecar.intent).unwrap(), marker["intent"]);
    let source = h
        .resources
        .get_command(&h.scenario.source_command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record_bson(&source.receipt).unwrap(),
        marker["source_receipt"]
    );
    assert!(h.logs().await.starts_with(&original_logs));
    let refused = matches!(
        h.scenario.mode.as_str(),
        "later-turn" | "consent-reenabled" | "canonical-disagreement"
    );
    let expected_effects = u64::from(!refused);
    assert_eq!(h.effect_count().await, expected_effects);
    if expected_effects == 1 {
        let effect = h
            .client
            .database(&h.scenario.database)
            .collection::<Document>("compatibility_runtime_inputs")
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap();
        let expected_kind = if h.scenario.cases[0].body["content"]["text"]
            .as_str()
            .unwrap()
            .starts_with("CMA-CONTROL/1 steer ")
        {
            "steer"
        } else {
            "interrupt"
        };
        assert_eq!(effect.get_str("kind").unwrap(), expected_kind);
        assert_eq!(
            effect.get_str("command_id").unwrap(),
            h.scenario.control_command_id.as_str()
        );
    }
    if refused {
        assert!(after.work.dispatch.is_none());
    } else {
        assert_eq!(
            after.work.dispatch.as_ref().unwrap().turn_sequence,
            h.scenario.source_turn
        );
        assert_eq!(
            after.work.observation.as_ref().unwrap().delivery,
            if h.scenario.mode == "unknown-write" {
                AgentOperationDelivery::RecoveryRequired
            } else {
                AgentOperationDelivery::WriteConfirmed
            }
        );
        assert_eq!(
            after.status.is_terminal(),
            h.scenario.mode != "unknown-write"
        );
    }
    for case in &h.scenario.cases {
        let write = signed::write(
            &h.client,
            &h.scenario.database,
            &h.bridges,
            &h.scenario.bridge_id,
            &case.body,
        )
        .await;
        assert_eq!(
            h.inbox.recover_receipt(&write).await.unwrap(),
            Some(case.expected.clone())
        );
        assert_eq!(h.inbox.accept(write).await.unwrap(), case.expected);
    }
    // The normal callback store still permanently refuses fresh controls while
    // accepted control execution/reconciliation remains part of this composition.
    let mut counts = Vec::new();
    for collection in ["agent_commands", "bridge_channel_control_intents"] {
        counts.push((
            collection,
            h.client
                .database(&h.scenario.database)
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
        ));
    }
    let fresh = signed::body(
        "compatible-fresh-disabled",
        900,
        h.scenario.cases[0].body["content"]["text"]
            .as_str()
            .unwrap(),
        true,
    );
    let receipt = h
        .inbox
        .accept(
            signed::write(
                &h.client,
                &h.scenario.database,
                &h.bridges,
                &h.scenario.bridge_id,
                &fresh,
            )
            .await,
        )
        .await
        .unwrap();
    let CallbackAcceptance::ControlRefused(receipt) = receipt else {
        panic!("fresh controls must remain disabled on the compatible rollback baseline");
    };
    assert_eq!(
        receipt.code,
        layer2_core::bridge::ingress::ChannelControlRefusal::ChannelControlsDisabled
    );
    for (collection, before) in counts {
        assert_eq!(
            h.client
                .database(&h.scenario.database)
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            before,
            "disabled fresh ingress created {collection} work"
        );
    }
    assert_eq!(h.effect_count().await, expected_effects);
    h.record_result("compatible_rollback",json!({"effects":expected_effects,"status":after.status,"source_turn":h.scenario.source_turn,"fresh_admission_enabled":false,"original_receipt_intent_output_preserved":true})).await;
}
