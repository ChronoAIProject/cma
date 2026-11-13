use super::*;
use std::time::Instant;

async fn sweep(store: &MongoSandboxStore, launches: &MongoTriggerDefinitionStore) -> Vec<usize> {
    let page = PageRequest::new(10, None).unwrap();
    let (a, b, c, d, e, f, g, h, i) = tokio::join!(
        store.command_work_page(&page),
        store.queued_sandbox_controls(&page),
        store.due_agent_work(NOW, 10),
        store.due_sandbox_work(NOW, 10),
        store.due_workspace_work(NOW, 10),
        store.pending_bindings(None, 10),
        store.pending_binding_conversations(None, 10),
        store.claim_access_request("measure"),
        launches.due_launches(NOW, 10)
    );
    vec![
        a.unwrap().items.len(),
        b.unwrap().items.len(),
        c.unwrap().len(),
        d.unwrap().len(),
        e.unwrap().len(),
        f.unwrap().len(),
        g.unwrap().len(),
        usize::from(h.unwrap().is_some()),
        i.unwrap().len(),
    ]
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn empty_mixed_and_busy_discovery_compare_actual_commands_and_latency() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_discovery_measure_{}", uuid::Uuid::new_v4().simple());
    let probes = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = probes.clone();
    let commands = Arc::new(AtomicU64::new(0));
    let observed = commands.clone();
    let db = name.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(e) = event {
            if e.db == db
                && matches!(
                    e.command_name.as_str(),
                    "find" | "aggregate" | "listCollections"
                )
            {
                observed.fetch_add(1, Ordering::Relaxed);
                if e.command_name == "aggregate" && captured.lock().unwrap().is_empty() {
                    if let Ok(pipeline) = e.command.get_array("pipeline") {
                        if pipeline.iter().any(|stage| {
                            stage
                                .as_document()
                                .and_then(|d| d.get_document("$project").ok())
                                .is_some_and(|d| d.contains_key("probe"))
                        }) {
                            captured.lock().unwrap().push(doc! {"aggregate":e.command.get_str("aggregate").unwrap(),"pipeline":pipeline.clone(),"cursor":{}});
                        }
                    }
                }
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let baseline = MongoSandboxStore::open(&client, &name).await.unwrap();
    let profiles = Arc::new(MongoProfileStore::open(&client, &name).await.unwrap());
    let launches = MongoTriggerDefinitionStore::open(&client, &name, profiles)
        .await
        .unwrap();
    for population in ["empty", "mixed_future", "mixed_due"] {
        if population != "empty" {
            let ready = if population == "mixed_future" {
                MINUTE
            } else {
                NOW
            };
            baseline.workspaces.delete_many(doc! {}).await.unwrap();
            baseline.workspaces.insert_many((0..200).map(|n|doc! {"_id":format!("wsp_{n:04}"),"sandbox_id":"sbx_measurement","name":format!("workspace-{n}"),
                "document_schema_version":DOCUMENT_SCHEMA_VERSION,"work_pending":true,"work_ready_at":ready,
                "work_lease_expires_at":Bson::Null})).await.unwrap();
        }
        let expected = sweep(&baseline, &launches).await;
        let mut on = baseline.clone();
        let mut on_launches = launches.clone();
        let policy = Policy::new(true);
        on.empty_discovery = policy.clone();
        on_launches.empty_discovery = policy;
        assert_eq!(sweep(&on, &on_launches).await, expected);
        let mut results = Vec::new();
        for (phase, store, launches) in [
            ("off_a", &baseline, &launches),
            ("on_b", &on, &on_launches),
            ("on_b2", &on, &on_launches),
            ("off_a2", &baseline, &launches),
        ] {
            let before = commands.load(Ordering::Relaxed);
            let counts = store.empty_discovery.counts();
            let mut millis = Vec::new();
            for _ in 0..40 {
                let at = Instant::now();
                assert_eq!(sweep(store, launches).await, expected);
                millis.push(at.elapsed().as_secs_f64() * 1000.);
            }
            millis.sort_by(f64::total_cmp);
            let queries = commands.load(Ordering::Relaxed) - before;
            let after = store.empty_discovery.counts();
            results.push(queries);
            println!(
                "EMPTY_DISCOVERY_MEASUREMENT {}",
                serde_json::json!({"population":population,"phase":phase,
                "sweeps":40,"commands":queries,"mean_ms":millis.iter().sum::<f64>()/40.,"p95_ms":millis[37],
                "probe_batches":after.0-counts.0,"coalesced_requests":after.1-counts.1})
            );
        }
        assert!(
            results[1] + results[2] < results[0] + results[3],
            "coalescing must reduce actual commands at {population}: {results:?}"
        );
    }
    let probe = probes.lock().unwrap().first().unwrap().clone();
    let explanation = client
        .database(&name)
        .run_command(doc! {"explain":probe,"verbosity":"executionStats"})
        .await
        .unwrap();
    let plan = serde_json::to_string(&explanation).unwrap();
    assert!(
        !plan.contains("COLLSCAN"),
        "pending probes must use indexes or EOF: {plan}"
    );
    assert!(
        plan.contains("workspaces_due_work"),
        "populated work uses the pending index"
    );
    println!(
        "EMPTY_DISCOVERY_EXPLAIN indexed_pending_workspace=true collection_scans=false branches=9"
    );
    // A nonempty pending result suppresses extra probes while ordinary reads
    // proceed, including when an ordinary due query returns an empty page.
    client.database(&name).drop().await.unwrap();
}
