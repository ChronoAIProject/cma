//! Isolated, reproducible recorder load; never sends messages to a real runtime.
use super::*;
use layer2_core::sandbox::{
    AgentOutputCoalescePolicy, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, MessageId, RuntimeOutputReceipt,
    RuntimeOutputSource, SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const SAMPLES: u64 = 120;

fn frame(
    cursor: u64,
    stream: &str,
    conversation: &str,
    ack: tokio::sync::mpsc::Sender<RuntimeOutputSource>,
) -> ConversationUpdate {
    let effect = match cursor % 10 {
        0..=5 => SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from("thought"))),
        6..=8 => SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from("text ")).message_id(MessageId::new("reply")),
        ),
        _ => SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            format!("tool-{cursor}"),
            ToolCallUpdateFields::new().title("Synthetic tool"),
        )),
    };
    ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: stream.into(),
                cursor,
                native_conversation_id: conversation.into(),
                payload_sha256: format!("{cursor:064x}"),
            },
            ack,
        ),
        update: Some(Box::new(ConversationUpdate::Said(effect)).into()),
    }
}

#[tokio::test]
#[ignore = "isolated real Mongo replica set measurement; CHRONO_TEST_MONGODB_URI required"]
async fn output_frame_replay_measurement() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let commands = Arc::new(AtomicU64::new(0));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Started(_)) {
            observed.fetch_add(1, Ordering::Relaxed);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_1807_measurement_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let version = client
        .database("admin")
        .run_command(doc! {"buildInfo":1})
        .await
        .unwrap();
    println!(
        "FRAME_MONGO_VERSION {}",
        version.get_str("version").unwrap()
    );
    for enabled in [false, true] {
        for rate in [10_u64, 30, 60] {
            let agent = working(&store, &format!("sbx_measurement_{enabled}_{rate}")).await;
            let recorder = DurableAgentUpdateRecorder::with_output_policy(
                Arc::new(store.clone()),
                Arc::default(),
                None,
                false,
                false,
                enabled.then(|| {
                    AgentOutputCoalescePolicy::new(Duration::from_millis(250), 64).unwrap()
                }),
            );
            let mut assembly = AgentMessageAssembly::default();
            let stream = uuid::Uuid::new_v4().to_string();
            let conversation = agent.conversation.conversation_id.clone().unwrap();
            let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
            let (ack, mut acknowledgements) = tokio::sync::mpsc::channel(1);
            let received = Arc::new(AtomicU64::new(0));
            let produced = received.clone();
            let start = Instant::now();
            let producer = tokio::spawn(async move {
                let start = tokio::time::Instant::now();
                for cursor in 1..=SAMPLES {
                    tokio::time::sleep_until(
                        start + Duration::from_secs_f64((cursor - 1) as f64 / rate as f64),
                    )
                    .await;
                    produced.store(cursor, Ordering::Relaxed);
                    send.send((cursor, frame(cursor, &stream, &conversation, ack.clone())))
                        .unwrap();
                }
            });
            let mut durations = Vec::new();
            let mut counts = [0_u64; 3];
            let mut command_totals = [0_u64; 3];
            let mut pending: Option<RuntimeOutputReceipt> = None;
            let mut acknowledged = 0;
            loop {
                let next = if let Some(deadline) = assembly.output_flush_deadline() {
                    tokio::select! {
                        biased;
                        _ = tokio::time::sleep_until(deadline) => None,
                        next = receive.recv() => next,
                    }
                } else {
                    receive.recv().await
                };
                let Some((cursor, update)) = next else {
                    if let Some(receipt) = pending.take() {
                        if let Some(deadline) = assembly.output_flush_deadline() {
                            tokio::time::sleep_until(deadline).await;
                        }
                        let command_start = commands.load(Ordering::Relaxed);
                        let committed = recorder
                            .flush_output(&agent.id, 1, &mut assembly, NOW.into())
                            .await
                            .unwrap()
                            .unwrap();
                        assert_eq!(committed, receipt.source);
                        receipt.acknowledge().await.unwrap();
                        acknowledged = acknowledgements.recv().await.unwrap().cursor;
                        println!("FRAME_FLUSH enabled={enabled} rate={rate} cursor={acknowledged} commands={}", commands.load(Ordering::Relaxed) - command_start);
                    }
                    if receive.is_closed() && receive.is_empty() {
                        break;
                    }
                    continue;
                };
                let receipt = match &update {
                    ConversationUpdate::RuntimeOutput { receipt, .. } => receipt.clone(),
                    _ => unreachable!(),
                };
                let before = serde_json::to_value(&assembly).unwrap();
                let command_start = commands.load(Ordering::Relaxed);
                let started = Instant::now();
                let result = recorder
                    .record_output(&agent.id, 1, &mut assembly, update, NOW.into())
                    .await
                    .unwrap();
                let elapsed = started.elapsed().as_secs_f64();
                let count = commands.load(Ordering::Relaxed) - command_start;
                let after = serde_json::to_value(&assembly).unwrap();
                let category = if !result.outcome.appended.is_empty() {
                    0
                } else if ["message_id", "content", "prompt_evidence"]
                    .iter()
                    .any(|key| before[key] != after[key])
                {
                    1
                } else {
                    2
                };
                if category != 0 && (!enabled || category == 1) {
                    assert_eq!(count, 4, "quiet/text frames require one snapshot read plus transaction read/write/commit");
                }
                counts[category] += 1;
                command_totals[category] += count;
                durations.push(elapsed);
                if let Some(committed) = result.committed {
                    assert_eq!(committed, receipt.source);
                    receipt.acknowledge().await.unwrap();
                    acknowledged = acknowledgements.recv().await.unwrap().cursor;
                    pending = None;
                } else {
                    pending = Some(receipt);
                }
                let lag = received
                    .load(Ordering::Relaxed)
                    .saturating_sub(acknowledged);
                println!("FRAME_SAMPLE {{\"enabled\":{enabled},\"rate\":{rate},\"cursor\":{cursor},\"seconds\":{},\"duration\":{elapsed},\"commands\":{count},\"category\":{category},\"lag\":{lag},\"queue\":{}}}", start.elapsed().as_secs_f64(), receive.len());
            }
            producer.await.unwrap();
            let elapsed = start.elapsed().as_secs_f64();
            let checkpoint = store
                .agent_message_checkpoint(&agent.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(checkpoint.output.unwrap().cursor, SAMPLES);
            durations.sort_by(f64::total_cmp);
            println!("FRAME_SUMMARY {{\"enabled\":{enabled},\"rate\":{rate},\"samples\":{SAMPLES},\"elapsed\":{elapsed},\"throughput\":{},\"p50\":{},\"p95\":{},\"counts\":{counts:?},\"command_totals\":{command_totals:?}}}", SAMPLES as f64 / elapsed, durations[durations.len()/2], durations[durations.len()*95/100]);
        }
    }
    client.database(&name).drop().await.unwrap();
}
