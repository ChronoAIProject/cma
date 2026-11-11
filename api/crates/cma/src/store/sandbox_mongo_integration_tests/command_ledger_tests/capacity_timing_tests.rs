use super::*;
use crate::store::agent_log_telemetry::{isolated_test, snapshot, ScanCaller};
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_normal_capacity;
use layer3_adapter::{
    ConversationUpdate, PromptIdentity, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource,
    TurnStopReason,
};

#[derive(Default)]
struct Trace {
    enabled: bool,
    start: Option<std::time::Instant>,
    transactions: Vec<(String, u128)>,
    commands: usize,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sixty_four_mib_terminal_settlement_measures_trusted_and_full_fallback_transactions() {
    for enabled in [false, true] {
        let trace = Arc::new(Mutex::new(Trace::default()));
        let observed = trace.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let mut trace = observed.lock().unwrap();
            if !trace.enabled {
                return;
            }
            match event {
                CommandEvent::Started(event) => {
                    trace.commands += 1;
                    if event.command.get_bool("startTransaction").unwrap_or(false) {
                        assert!(trace.start.is_none());
                        trace.start = Some(std::time::Instant::now());
                    }
                }
                CommandEvent::Succeeded(event)
                    if matches!(
                        event.command_name.as_str(),
                        "commitTransaction" | "abortTransaction"
                    ) =>
                {
                    if let Some(start) = trace.start.take() {
                        trace
                            .transactions
                            .push((event.command_name, start.elapsed().as_micros()));
                    }
                }
                _ => {}
            }
        }));
        let client = Client::with_options(options).unwrap();
        let name = format!("cma_capacity_timing_{}", uuid::Uuid::new_v4().simple());
        let (store, agent, parent, execution, _, _) = composed_with_history(
            &client,
            &name,
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            active_sandbox("sbx_commands"),
            Some((100, true)),
        )
        .await;
        let command = record(
            store
                .reserve_command(live_request(&agent, &parent, "timed-terminal"))
                .await
                .unwrap(),
        );
        AgentCommandWorker::new(store.clone(), execution, "timing-worker".into())
            .unwrap()
            .run_one(&command.receipt.envelope.command_id)
            .await
            .unwrap();
        let working = store.get_agent(&agent.id).await.unwrap().unwrap();
        let full = fill_normal_capacity(&store, working, true).await;
        let mut measured = MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap();
        if !enabled {
            measured.bounded_admission_agents.clear();
        }
        let measured = Arc::new(measured);
        let recorder = DurableAgentUpdateRecorder::new(measured);
        let (acks, _) = tokio::sync::mpsc::channel(1);
        let update = ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "b7111111-1111-4111-8111-111111111111".into(),
                    cursor: 1,
                    native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
                    payload_sha256: "a".repeat(64),
                },
                acks,
            ),
            update: Some(Box::new(ConversationUpdate::TurnEndedForPrompt {
                prompt_id: PromptIdentity::new(
                    full.conversation.conversation_id.as_deref().unwrap(),
                    1,
                    full.conversation.active_turn.unwrap(),
                ),
                response: PromptResponse::new(TurnStopReason::EndTurn),
            })),
        };
        trace.lock().unwrap().enabled = true;
        isolated_test(async {
            let start=std::time::Instant::now();
            let outcome=recorder.record_update(&agent.id,1,&mut AgentMessageAssembly::default(),update,layer2_core::now_rfc3339()).await.unwrap();
            let elapsed=start.elapsed();
            assert!(outcome.turn_closed);
            let scans=snapshot(ScanCaller::Checkpoint,true);
            assert_eq!(scans[0],if enabled {0} else {2});
            let mut t=trace.lock().unwrap();t.enabled=false;
            assert_eq!(t.transactions.len(),2);
            assert_eq!(t.transactions[0].0,"abortTransaction");
            assert_eq!(t.transactions[1].0,"commitTransaction");
            assert!(t.transactions.iter().all(|(_,micros)| *micros<60_000_000));
            println!("capacity_timing {}",serde_json::json!({"prepared_enabled":enabled,"N":1,"bson_bytes":67108864,"recorder_us":elapsed.as_micros(),"transactions_us":t.transactions,"commands":t.commands,"scans":scans[0],"scan_documents":scans[1],"scan_bytes":scans[2]}));
        }).await;
        client.database(&name).drop().await.unwrap();
    }
}
