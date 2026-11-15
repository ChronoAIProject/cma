use super::*;
use layer2_core::sandbox::{
    agent_output_index::AgentOutputChange, store::indexed::OUTPUT_PAGE_CHANGES,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::{ClientOptions, Hint, ReadConcern},
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default, PartialEq, Eq)]
struct Commands {
    finds: usize,
    get_mores: usize,
    writes: usize,
}

async fn observed_client() -> (Client, Arc<Mutex<Commands>>) {
    let commands = Arc::new(Mutex::new(Commands::default()));
    let seen = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut seen = seen.lock().unwrap();
            match event.command_name.as_str() {
                "find" => seen.finds += 1,
                "getMore" => seen.get_mores += 1,
                "insert" | "update" | "delete" | "findAndModify" => seen.writes += 1,
                _ => {}
            }
        }
    }));
    (Client::with_options(options).unwrap(), commands)
}

fn query(head: &Head, through: u64) -> Document {
    doc! {
        "agent_id":&head.agent_id, "incarnation":&head.incarnation,
        "evidence_generation":&head.evidence_generation, "kind":"output_change",
        "turn_sequence":3_i64, "change_ordinal":{"$gte":0_i64,"$lt":through as i64},
    }
}

fn changes(head: &Head, rows: Vec<Document>) -> Vec<AgentOutputChange> {
    rows.into_iter()
        .enumerate()
        .map(|(ordinal, row)| {
            let key = format!("3:{ordinal}");
            let segments::projection::EvidenceRecord::OutputChange { change } =
                segments::projection::read_evidence(head, "output_change", &key, row).unwrap()
            else {
                panic!("expected output-change evidence")
            };
            change
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn full_output_pages_avoid_empty_get_more_and_preserve_traversal() {
    let (client, commands) = observed_client().await;
    let name = format!("cma_output_batch_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = fixtures::source(&store, 2, true).await;
    fixtures::certify(&client, &db, &agent, 2).await;
    let head = Head::read(
        store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut reader = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    let total = reader.metadata().index.as_ref().unwrap().changes;
    assert!(total > u64::from(OUTPUT_PAGE_CHANGES));
    assert_ne!(
        total % u64::from(OUTPUT_PAGE_CHANGES),
        0,
        "include a partial final page"
    );
    let expected = changes(
        &head,
        store
            .segment_logs
            .evidence
            .find(query(&head, total))
            .sort(doc! {"change_ordinal":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap(),
    );
    assert_eq!(expected.len() as u64, total);

    // Reproduce the previous full-page query: the validation lookahead is in
    // the limit, but only the requested page fits in the first batch.
    let mut session = client.start_session().snapshot(true).await.unwrap();
    *commands.lock().unwrap() = Commands::default();
    let mut cursor = store
        .segment_logs
        .evidence
        .find(query(&head, u64::from(OUTPUT_PAGE_CHANGES)))
        .hint(Hint::Name("agent_log_output_change_seek".into()))
        .sort(doc! {"change_ordinal":1})
        .limit(i64::from(OUTPUT_PAGE_CHANGES) + 1)
        .batch_size(u32::from(OUTPUT_PAGE_CHANGES))
        .max_time(std::time::Duration::from_secs(2))
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap();
    let mut rows = Vec::new();
    while let Some(row) = cursor.next(&mut session).await {
        rows.push(row.unwrap());
    }
    let previous = std::mem::take(&mut *commands.lock().unwrap());
    assert_eq!(
        previous,
        Commands {
            finds: 1,
            get_mores: 1,
            writes: 0
        }
    );
    assert_eq!(
        changes(&head, rows),
        expected[..usize::from(OUTPUT_PAGE_CHANGES)]
    );

    for limit in [1, 3, OUTPUT_PAGE_CHANGES] {
        let mut all = Vec::new();
        let mut pages = 0;
        while (all.len() as u64) < total {
            *commands.lock().unwrap() = Commands::default();
            let page = reader.changes(all.len() as u64, limit).await.unwrap();
            let actual = std::mem::take(&mut *commands.lock().unwrap());
            assert_eq!(
                actual,
                Commands {
                    finds: 1,
                    get_mores: 0,
                    writes: 0
                },
                "limit={limit}, from={}",
                all.len()
            );
            let from = all.len();
            let end = (from + usize::from(limit)).min(expected.len());
            assert_eq!(page, expected[from..end]);
            all.extend(page);
            pages += 1;
        }
        assert_eq!(
            serde_json::to_vec(&all).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        *commands.lock().unwrap() = Commands::default();
        assert!(reader.changes(total, limit).await.unwrap().is_empty());
        assert_eq!(*commands.lock().unwrap(), Commands::default());
        eprintln!("output_batch limit={limit} rows={total} pages={pages} finds_per_page=1 get_more_per_page=0 previous_full_find=1 previous_full_get_more=1");
    }
    for (from, limit) in [(0, 0), (0, OUTPUT_PAGE_CHANGES + 1), (total + 1, 1)] {
        assert!(matches!(
            reader.changes(from, limit).await,
            Err(layer2_core::sandbox::store::SandboxStoreError::InvalidPage)
        ));
    }
    assert_eq!(*commands.lock().unwrap(), Commands::default());
    drop(reader);
    db.drop().await.unwrap();
}
