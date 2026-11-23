use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Commands {
    finds: Vec<String>,
    point_reads: usize,
    source_key_probes: usize,
    index_inventories: usize,
    writes: usize,
}

impl Commands {
    fn assert_capture_and_point_checks(&self) {
        for collection in [segments::HEADS, segments::AUDITS] {
            assert_eq!(
                self.finds.iter().filter(|name| *name == collection).count(),
                1 + self.point_reads,
                "{collection}: one capture check plus each retained point-read check"
            );
        }
        assert_eq!(self.source_key_probes, 2 * (1 + self.point_reads));
        assert_eq!(self.index_inventories, 1 + self.point_reads);
        assert_eq!(self.writes, 0);
    }
}

async fn observed_store(name: &str) -> (MongoSandboxStore, Arc<Mutex<Commands>>) {
    let commands = Arc::new(Mutex::new(Commands::default()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut observed = observed.lock().unwrap();
            if event.command.get_str("listIndexes").ok() == Some("agent_log") {
                observed.index_inventories += 1;
            }
            if let Ok(collection) = event.command.get_str("find") {
                if collection == "agent_log"
                    && event
                        .command
                        .get_document("projection")
                        .is_ok_and(|p| p.contains_key("found"))
                {
                    observed.source_key_probes += 1;
                }
                observed.finds.push(collection.to_owned());
                if collection == "agent_log"
                    && event
                        .command
                        .get_document("filter")
                        .ok()
                        .and_then(|filter| filter.get_document("seq").ok())
                        .is_some_and(|seq| seq.contains_key("$in"))
                {
                    observed.point_reads += 1;
                }
            }
            if matches!(
                event.command_name.as_str(),
                "insert" | "update" | "delete" | "findAndModify"
            ) {
                observed.writes += 1;
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, name).await.unwrap();
    *commands.lock().unwrap() = Commands::default();
    (store, commands)
}

// The previous dispatch checked the head before capture checked it again.
// Keep that baseline only here to count actual Mongo commands on equal data.
async fn previous_usage(store: &MongoSandboxStore, agent: &Agent) -> AgentUsageReading {
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(crate::store::agents::read_agent(&row).unwrap(), *agent);
    assert!(segments::read_format(&row).unwrap().is_some());
    let head = segments::read_head(store, &row, &mut session, true)
        .await
        .unwrap();
    assert_eq!(head.last_seq as u64, agent.conversation.last_seq);
    let source = capture::capture(store, agent, &mut session, capture::CaptureMode::Snapshot)
        .await
        .unwrap();
    AgentUsageReading {
        agent_id: agent.id.clone(),
        through_seq: agent.conversation.last_seq,
        usage: source.projection().usage(),
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn projections_capture_head_once_with_equal_results_and_fewer_find_commands() {
    for closed in [0, 2] {
        let (_, db, _, agent, entries) = fixture(closed, false).await;
        let (store, commands) = observed_store(db.name()).await;
        let baseline = previous_usage(&store, &agent).await;
        let before = std::mem::take(&mut *commands.lock().unwrap());
        assert_eq!(before.writes, 0);

        let usage = store
            .agent_log_usage_projection(&agent)
            .await
            .unwrap()
            .unwrap();
        let after = std::mem::take(&mut *commands.lock().unwrap());
        assert_eq!(usage, baseline);
        assert_eq!(usage.usage.output_tokens, 8);
        after.assert_capture_and_point_checks();
        // The redundant dispatch head also performs both source-key probes.
        let saved = if closed == 0 { 6 } else { 5 };
        assert_eq!(before.finds.len(), after.finds.len() + saved);
        eprintln!(
            "projection_capture closed={closed} usage_find_before={} after={} saved={saved}",
            before.finds.len(),
            after.finds.len()
        );

        let transcript = expected(&agent, &entries);
        assert_eq!(
            store.agent_log_transcript_projection(&agent).await.unwrap(),
            Some(transcript.clone())
        );
        std::mem::take(&mut *commands.lock().unwrap()).assert_capture_and_point_checks();

        let page = store
            .agent_log_transcript_page(&agent, &first(32))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.transcript.agent_id, transcript.agent_id);
        assert_eq!(page.transcript.through_seq, transcript.through_seq);
        assert_eq!(
            page.transcript.turns[0].owner_message,
            transcript.turns[0].owner_message
        );
        std::mem::take(&mut *commands.lock().unwrap()).assert_capture_and_point_checks();

        let expected_answer = transcript.turns[0].final_answer().unwrap();
        let answer = store
            .agent_log_final_answer(&agent, expected_answer.sequence)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&answer.message, expected_answer);
        assert_eq!(answer.turn_sequence, transcript.turns[0].turn_sequence);
        std::mem::take(&mut *commands.lock().unwrap()).assert_capture_and_point_checks();
        assert_eq!(collect(&store, &agent, 4).await, transcript);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn privacy_export_refuses_over_budget_segment_before_fetching_log_originals() {
    let (_, db, store, agent, _) = fixture(0, false).await;
    let (observed, commands) = observed_store(db.name()).await;
    let request = first(32);
    let page = observed
        .agent_log_export_page(&agent, &request)
        .await
        .unwrap();
    assert!(page.is_some());
    assert!(!std::mem::take(&mut *commands.lock().unwrap())
        .finds
        .is_empty());

    let over_budget = 8_i64 * 1024 * 1024 + 1;
    assert_eq!(
        store
            .segment_logs
            .heads
            .update_one(
                doc! {"_id": agent.id.as_str()},
                doc! {"$set": {"active_bytes": over_budget, "lifetime_bytes": over_budget}},
            )
            .await
            .unwrap()
            .modified_count,
        1,
    );
    assert_eq!(
        observed.agent_log_export_page(&agent, &request).await,
        Err(SandboxStoreError::AgentLogCapacityExceeded),
    );
    let refused = std::mem::take(&mut *commands.lock().unwrap());
    assert!(refused.finds.iter().any(|name| name == segments::HEADS));
    assert!(refused.finds.iter().all(|name| name != "agent_log"));
    assert_eq!(refused.writes, 0);
    db.drop().await.unwrap();
}
