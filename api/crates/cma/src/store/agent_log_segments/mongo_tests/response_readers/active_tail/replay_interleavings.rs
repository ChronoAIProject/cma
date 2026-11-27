use super::*;
use layer2_core::sandbox::{
    store::{SandboxEventStore, SandboxStore},
    SandboxEventKind,
};
use std::time::{Duration, Instant};

const PAGE: u16 = 200;

#[tokio::test]
#[ignore = "requires a disposable replica set with profiler access"]
async fn tc045_replay_interleavings_reconnect_with_exact_durable_cursor() {
    let (client, trace) = measure::client().await;
    let name = format!("cma_replay_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let result = AssertUnwindSafe(campaign(&client, &db, &trace))
        .catch_unwind()
        .await;
    let cleanup = db.drop().await;
    match result {
        Ok(()) => cleanup.unwrap(),
        Err(original) => {
            if let Err(error) = cleanup {
                eprintln!("fixture cleanup after failure: {error}");
            }
            std::panic::resume_unwind(original);
        }
    }
}

async fn campaign(client: &Client, db: &mongodb::Database, trace: &measure::Trace) {
    schema::migrate(db).await.unwrap();
    let store = MongoSandboxStore::open(client, db.name()).await.unwrap();
    let case = fixture::CASES[2];
    let fixture = fixture::install(&store, case).await;
    let writer = prepare_active_proof(store, db, &fixture.agent).await;
    use segments::{writer_control, writer_gate};
    writer_control::execute(
        client,
        db,
        writer_gate::GateName::Segments,
        writer_control::Action::Replace {
            expected_revision: 0,
            policy: writer_gate::Policy {
                enabled: true,
                agents: vec![fixture.agent.id.to_string()],
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut reader = MongoSandboxStore::open(&Client::with_uri_str(&uri).await.unwrap(), db.name())
        .await
        .unwrap();
    let mut reopened =
        MongoSandboxStore::open(&Client::with_uri_str(&uri).await.unwrap(), db.name())
            .await
            .unwrap();
    reader.active_log_proofs = true;
    reopened.active_log_proofs = true;
    let mut sandbox =
        crate::store::sandbox_mongo_integration_tests::sandbox(fixture.agent.sandbox_id.as_str());
    sandbox.owner = fixture.agent.owner.clone();
    writer.create_sandbox(sandbox.clone()).await.unwrap();

    let mut expected = [fixture.target.clone(), fixture.active.clone()].concat();
    // Score the same compatibility page port used for event replay, including
    // a page spanning the initial seal and the empty committed tail.
    for after in [0, 255, 256, expected.len() as u64] {
        let (page, measured) = measure::sample(
            db,
            trace,
            case,
            &fixture,
            0,
            "page",
            writer.agent_log_range(&fixture.agent.id, after, expected.len() as u64, PAGE),
        )
        .await;
        assert_eq!(
            page.unwrap(),
            expected[after as usize..(after as usize + PAGE as usize).min(expected.len())]
        );
        assert!(
            measured.docs_examined <= 250,
            "cursor {after} scanned {} documents",
            measured.docs_examined
        );
    }

    let id = &fixture.agent.id;
    let mut latencies = Vec::new();
    let mut sandbox_events = Vec::new();
    // Ten cursor positions, 25 acknowledgement boundaries and four
    // publication orderings exercise 1,000 distinct replay schedules.
    for position in 0..10 {
        for acknowledged in 0..25usize {
            for schedule in 0..4 {
                let round = latencies.len();
                let head = expected.len() as u64;
                let after = match position {
                    0..=4 => 253 + position,
                    _ => head - 200 - (position - 5),
                };
                let end = after + u64::from(PAGE);
                let kind = AgentLogKind::ModeChanged {
                    mode: format!("publication-{round}"),
                };
                let identity = AgentLogPublicationIdentity {
                    key: format!("tc045-{round}"),
                    appended_at: NOW.into(),
                    fingerprint: segments::publication::request_digest(&kind)
                        .unwrap()
                        .bytes
                        .try_into()
                        .unwrap(),
                };
                let append = || writer.append_agent_log_keyed(id, &identity, kind.clone());
                let first = || reader.agent_log_range(id, after, end, PAGE);
                let mut published = None;
                let page = match schedule {
                    0 => {
                        published = Some(append().await.unwrap());
                        first().await.unwrap()
                    }
                    1 => {
                        let (page, entry) = tokio::join!(first(), append());
                        published = Some(entry.unwrap());
                        page.unwrap()
                    }
                    _ => first().await.unwrap(),
                };
                assert_eq!(
                    page,
                    expected[after as usize..end as usize],
                    "round {round}"
                );

                // Model a connection dropping after only this prefix was
                // durably acknowledged. The unacknowledged suffix is lost.
                let mut delivered = page[..acknowledged].to_vec();
                drop(page);
                let interrupted = Instant::now();
                let cursor = after + acknowledged as u64;
                let resume = || reopened.agent_log_range(id, cursor, end, PAGE);
                let suffix = match schedule {
                    2 => {
                        published = Some(append().await.unwrap());
                        resume().await.unwrap()
                    }
                    3 => {
                        let (entry, suffix) = tokio::join!(append(), resume());
                        published = Some(entry.unwrap());
                        suffix.unwrap()
                    }
                    _ => resume().await.unwrap(),
                };
                delivered.extend(suffix);
                assert_eq!(
                    delivered,
                    expected[after as usize..end as usize],
                    "round {round}"
                );
                let entry = published.expect("each interleaving publishes one durable event");
                assert_eq!(entry.seq, head + 1);
                assert_eq!(entry.kind, kind);
                assert!(reopened
                    .agent_log_range(id, head, head, PAGE)
                    .await
                    .unwrap()
                    .is_empty());
                assert_eq!(
                    reopened
                        .agent_log_range(id, head, entry.seq, PAGE)
                        .await
                        .unwrap(),
                    [entry.clone()]
                );
                latencies.push(interrupted.elapsed());
                assert_eq!(
                    writer
                        .append_agent_log_keyed(id, &identity, kind)
                        .await
                        .unwrap(),
                    entry
                );
                expected.push(entry);

                let event = writer
                    .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
                    .await
                    .unwrap();
                assert_eq!(event.seq, round as u64 + 1);
                assert_eq!(event.sandbox_id, sandbox.id);
                sandbox_events.push(event);
            }
        }
    }
    assert_eq!(latencies.len(), 1_000);
    for (index, rows) in expected.chunks(PAGE as usize).enumerate() {
        let after = (index * PAGE as usize) as u64;
        assert_eq!(
            reopened
                .agent_log_range(id, after, expected.len() as u64, PAGE)
                .await
                .unwrap(),
            rows
        );
    }
    for (index, rows) in sandbox_events.chunks(PAGE as usize).enumerate() {
        assert_eq!(
            reopened
                .sandbox_events(&sandbox.id, (index * PAGE as usize) as u64, PAGE)
                .await
                .unwrap(),
            rows
        );
    }
    assert!(writer
        .sandbox_events(&sandbox.id, 1_000, PAGE)
        .await
        .unwrap()
        .is_empty());
    assert!(writer
        .agent_log(id, expected.len() as u64, PAGE)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        writer
            .agent_log
            .count_documents(doc! {"agent_id": id.as_str()})
            .await
            .unwrap(),
        expected.len() as u64
    );
    assert_eq!(
        collection(&writer, PUBLICATIONS)
            .count_documents(doc! {"agent_id":id.as_str()})
            .await
            .unwrap(),
        1_000
    );
    latencies.sort_unstable();
    let p99 = latencies[989];
    eprintln!("tc045 interleavings={} agent_rows={} sandbox_events={} recovery_p99_ms={} recovery_max_ms={}", latencies.len(), expected.len(), sandbox_events.len(), p99.as_millis(), latencies[999].as_millis());
    assert!(p99 <= Duration::from_secs(10));
}
