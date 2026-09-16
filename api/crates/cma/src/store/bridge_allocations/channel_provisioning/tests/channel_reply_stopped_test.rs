use super::*;
use layer2_core::sandbox::{
    store::{AgentProvenanceStore, AgentStore},
    AgentStatus, AgentStopReason,
};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn queued_original_reply_after_reset_survives_stopped_new_sandbox_agent_without_execution_authority(
) {
    let f = ReplyFixture::new().await;
    let context = f.context().await;
    assert_eq!(
        f.reply(&context, "before-agent-stop", "Accepted before stop.")
            .await["isError"],
        false
    );
    reset(&f).await;
    // Establish the typed retained terminal record. This regression exercises
    // delivery predicates; the asynchronous Agent stop worker has separate tests.
    let mut agent = f
        .resources
        .get_agent(&f.command.receipt.envelope.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent
        .transition(
            AgentStatus::Stopped,
            layer2_core::now_rfc3339(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    f.resources.save_agent(revision, agent).await.unwrap();
    let mut retained = f
        .resources
        .agent_provenance_snapshot(&f.command.receipt.envelope.agent_id)
        .await
        .unwrap();
    f.grant
        .target
        .require_retained_reply_target(&f.command.receipt.envelope.owner, &retained.resource)
        .unwrap();
    let deleted = retained.resource.lineage.agent.as_mut().unwrap();
    deleted.status = AgentStatus::Deleted;
    deleted.deleted_at = Some(layer2_core::now_rfc3339());
    assert!(f
        .grant
        .target
        .require_retained_reply_target(&f.command.receipt.envelope.owner, &retained.resource)
        .is_err());
    assert!(f
        .invokes
        .authorize(layer2_core::bridge::invoke::BridgeInvokeRequest {
            grant_id: f.grant.id.clone(),
            pin: f.grant.pin.clone(),
            target: f.grant.target.ids.clone(),
            effect: layer2_core::bridge::invoke::BridgeInvokeEffect::ExecuteCommand {
                command_id: f.command.receipt.envelope.command_id.clone()
            },
        })
        .await
        .is_err());
    assert_eq!(
        f.raw_call(
            "reply_to_channel",
            serde_json::json!({
                "reply_context": context,
                "request_key": "new-after-stop",
                "text": "Cannot execute a new tool."
            }),
        )
        .await
        .status(),
        axum::http::StatusCode::UNAUTHORIZED
    );
    let relay = Relay {
        original: f.allocation.clone(),
        command: f.command.clone(),
        message: f.inbox.message_id.clone(),
        calls: Mutex::new(Vec::new()),
    };
    assert!(f
        .replies
        .deliver_one("stopped-original-reply", &relay)
        .await
        .unwrap());
    assert_eq!(relay.calls.lock().unwrap().len(), 1);
    assert_eq!(
        f.resources
            .get_command(&f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        f.command.receipt
    );
    f.close().await;
}
