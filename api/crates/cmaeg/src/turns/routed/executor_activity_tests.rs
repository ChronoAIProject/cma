use super::{
    adapter_test_server::HttpServer,
    delivery_tests::{command, stored},
    executor_tests::executor,
    scheduler::{Disposition, TargetExecutor},
    test_support::*,
    *,
};
use crate::{
    activity::{ActivityStore, Change, ConsumerState},
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
    ingress::routing_store::test_server::{Row, Server},
};
use cmaeg_core::seal::seal_blob;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn activity_terminal_outcome_survives_checkpoint_recovery_and_queued_replay() {
    for status in ["completed", "failed", "incomplete"] {
        for observe_live in [true, false] {
            let record = uuid::Uuid::new_v4().to_string();
            let request_for_record = || {
                fixture::request(
                    |value| {
                        fixture::targets(value);
                        value["record_id"] = record.clone().into();
                    },
                    fixture::message("hello"),
                )
            };
            let mut setup = Setup::with_request(supported(), request_for_record()).await;
            let request = request_for_record();
            let record = request.channel.definition.record_id.to_string();
            let owner = request.channel.definition.owner.subject.clone();
            let activity_records = Server::start_realtime().await;
            activity_records.memory.channels.lock().unwrap().insert(
                record.clone(),
                Row {
                    owner: owner.clone(),
                    blob: seal_blob(
                        &fixture::keys(),
                        &serde_json::to_vec(&request.channel.definition.to_blob_json()).unwrap(),
                    )
                    .unwrap(),
                    expiry: time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
                    version: 1,
                },
            );
            let activity = Arc::new(ActivityStore::new(
                activity_records.records.clone(),
                Arc::new(fixture::keys()),
            ));
            activity.received(&request).await.unwrap();
            if observe_live {
                let journal = setup.journal().clone().with_activity(activity.clone());
                setup.dispatcher = Dispatcher::new(
                    setup.server.store(),
                    journal,
                    setup.current.clone(),
                    setup.port.clone(),
                );
            }
            let http = HttpServer::start().await;
            http.memory.lock().unwrap().config.replay_frames = Some(vec![
                json!({"type":"response.created","sequence_number":0,"response":{
                    "id":"resp_accepted","object":"response","status":"queued",
                    "conversation":{"id":"agt_native"},"metadata":{},"output":[]}}),
                json!({"type":format!("response.{status}"),"sequence_number":1,"response":{
                    "id":"resp_accepted","object":"response","status":status,
                    "conversation":{"id":"agt_native"},"metadata":{},"output":[],
                    "error":{"code":"provider_failed","message":"fixture failure"},
                    "incomplete_details":{"reason":"max_output_tokens"}}}),
            ]);
            let (runner, mut rx, work) = executor(&setup, &http);
            let (result, ()) = tokio::join!(runner.execute(&work), async {
                if status != "completed" {
                    command(&mut rx)
                        .await
                        .receipt
                        .send(receipt_for(DeliveryOutcome::Confirmed {
                            platform_message_id: Some("reply".into()),
                        }))
                        .unwrap();
                }
            });
            assert!(matches!(result, Disposition::Settled));
            assert!(matches!(
                stored(&setup).await.phase,
                Phase::Completed { .. }
            ));
            setup.server.change(&setup.target().id, |state| {
                state["phase"]["state"] = json!("accepted");
            });
            // Simulate a fresh process: no activity context cache, provider POST
            // or delivery is repeated after the acknowledged terminal cursor.
            let restored = Arc::new(ActivityStore::new(
                activity_records.records.clone(),
                Arc::new(fixture::keys()),
            ));
            let journal = setup.journal().clone().with_activity(restored.clone());
            setup.dispatcher = Dispatcher::new(
                setup.server.store(),
                journal,
                setup.current.clone(),
                setup.port.clone(),
            );
            let count = http.seen().len();
            let (resumed, mut rx, work) = executor(&setup, &http);
            assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
            assert_eq!(http.seen().len(), count);
            assert!(rx.try_recv().is_err());
            restored
                .change(
                    &record,
                    request.activity.event_id.as_str(),
                    &Change::state("default", ConsumerState::Queued, None),
                )
                .await
                .unwrap();
            let page = restored.latest(&owner, &record).await.unwrap();
            let consumer = &page.items[0].consumers[0];
            let expected = if !observe_live {
                ConsumerState::Unknown
            } else if status == "completed" {
                ConsumerState::Completed
            } else {
                ConsumerState::Failed
            };
            assert_eq!(
                consumer.state, expected,
                "{status}, observed={observe_live}"
            );
            assert!(consumer.completed_at.is_some());
        }
    }
}
