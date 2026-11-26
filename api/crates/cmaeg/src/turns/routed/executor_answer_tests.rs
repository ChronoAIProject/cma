use super::{
    adapter_test_server::HttpServer,
    delivery_tests::stored,
    executor_tests::executor,
    scheduler::{Disposition, TargetExecutor},
    test_support::{fixture, supported, Setup},
    tool_reply_store::{Admission, ReplyStore},
    *,
};
use crate::{
    auth::AgentCaller,
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
};
use serde_json::json;

#[tokio::test]
async fn stream_only_refusals_publish_a_receipt_and_recover_the_frame_without_a_send() {
    for uncertain in [false, true] {
        let mut setup = Setup::with_request(
            supported(),
            fixture::request(
                |value| value["routing"]["default_targets"] = json!(["default"]),
                fixture::message("fixture"),
            ),
        )
        .await;
        let mut journal = setup.journal().clone();
        journal.accepted_answer_receipts = true;
        setup.dispatcher = Dispatcher::new(
            setup.server.store(),
            journal.clone(),
            setup.current.clone(),
            setup.port.clone(),
        );
        let target = setup.target();
        let replies = ReplyStore(journal, None);
        let outcome = if uncertain {
            DeliveryOutcome::Unknown {
                reason: "fixture_uncertain".into(),
            }
        } else {
            DeliveryOutcome::Rejected {
                reason: "fixture_refused".into(),
            }
        };
        let keys = if uncertain { 1 } else { 64 };
        for index in 0..keys {
            let key = storage::hash(&[index]);
            let Admission::Claimed(claim) = replies
                .admit_keyed(&setup.plan, target, key.clone(), key)
                .await
                .unwrap()
            else {
                panic!("reserve an unrelated ordinary reply");
            };
            replies
                .finish(&claim, &receipt_for(outcome.clone()))
                .await
                .unwrap();
        }
        let peer = replies.operator_record(&setup.plan, target).await.unwrap();
        let http = HttpServer::start().await;
        let mut frames = super::executor_tests::frames();
        frames[2] = json!({
            "type": "response.output_item.done",
            "sequence_number": 2,
            "output_index": 0,
            "item": {
                "id": "m", "type": "message", "role": "assistant",
                "cmaeg_delivery": "primary_v1",
                "content": [{"type": "output_text", "text": "hello world"}]
            }
        });
        http.memory.lock().unwrap().config.replay_frames = Some(frames);
        let (mut runner, mut sends, work) = executor(&setup, &http);
        runner.frame_limit = 3;
        assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
        assert_eq!(stored(&setup).await.delivery.unwrap().cursor, 2);
        assert_eq!(
            peer,
            replies.operator_record(&setup.plan, target).await.unwrap()
        );

        let (delivery, _) = super::delivery_tests::lane(&setup);
        let tools = delivery.tool_replies();
        let channel = setup.current.channel.lock().unwrap().clone();
        let read = || {
            tools.accepted_answer_receipt(
                AgentCaller {
                    key_hash: channel.agent_key_hash,
                },
                &channel,
                AnswerReceiptLookup {
                    channel_id: channel.id.clone(),
                    binding_id: target.launch.provider.binding_id.clone(),
                    event_id: setup.plan.identity.event_id.clone(),
                    response_id: "resp_accepted".into(),
                    item_id: "m".into(),
                },
            )
        };
        let before = serde_json::to_value(read().await.unwrap().unwrap().result).unwrap();
        assert_eq!(before["dispatch"], "not_dispatched");
        assert_eq!(before["outcome"], "rejected");
        assert_eq!(before["reason"], "refused_before_dispatch");

        // Recreate a crash after the refusal and frame intent were written
        // but before the target checkpoint learned the terminal receipt.
        setup.server.change(&target.id, |state| {
            state["phase"]["durable_cursor"] = json!(1);
            state["delivery"]["status"] = json!({"state": "intent"});
        });
        runner.frame_limit = 1;
        assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
        assert!(matches!(
            stored(&setup).await.phase,
            Phase::Accepted {
                durable_cursor: Some(2),
                ..
            }
        ));
        assert_eq!(
            before,
            serde_json::to_value(read().await.unwrap().unwrap().result).unwrap()
        );
        assert_eq!(
            peer,
            replies.operator_record(&setup.plan, target).await.unwrap()
        );
        assert!(
            sends.try_recv().is_err(),
            "neither stream pass may enqueue a platform command"
        );
    }
}
