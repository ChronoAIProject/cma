use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn typed_metadata_pages_use_id_index_and_never_decode_allocated_or_transport_records() {
    let f = Fixture::new().await;
    let prefix = keys::prefix("stream", "fixture");
    for id in [
        mongodb::bson::Bson::String("ordinary-discovery".into()),
        mongodb::bson::Bson::Document(doc! {"usage_transport_v1":"stream/fixture/source"}),
    ] {
        f.store
            .collection(INVENTORY)
            .insert_one(doc! {"_id":id,"document_schema_version":1_i64,"unrelated":true})
            .await
            .unwrap();
    }
    for index in 0..40 {
        let mut source = f.stream.clone();
        source.id = uuid::Uuid::new_v4().to_string();
        source.sandbox_id = format!("sandbox-{index}");
        source.owner = format!("owner-{index}");
        f.store
            .collection(STREAMS)
            .insert_one(source.document().unwrap())
            .await
            .unwrap();
    }
    for _ in 0..4 {
        f.store.discover_measured().await.unwrap();
    }
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..4 {
        let page = f.store.next_measured_page().await.unwrap();
        assert!(page.len() <= 16);
        for key in page {
            assert!(key.starts_with(&prefix));
            assert!(seen.insert(key));
        }
    }
    assert_eq!(seen.len(), 41);
    let mut after = String::new();
    let mut delivery = std::collections::BTreeSet::new();
    loop {
        let page = f.store.export_streams(&after).await.unwrap();
        assert!(page.len() <= 32);
        if page.is_empty() {
            break;
        }
        for stream in page {
            assert!(stream > after);
            after = stream.clone();
            assert!(delivery.insert(stream));
        }
    }
    assert_eq!(
        delivery.len(),
        82,
        "both bases are discovered in the same bounded ordered scan"
    );
    assert_eq!(
        super::super::super::schema::pending(&f.db).await.unwrap(),
        0
    );
    CaptureStore::open(&f.store.client, f.db.name(), f.store.config.clone())
        .await
        .unwrap();
    let explain = f.db.run_command(doc! {"explain":{"find":INVENTORY,"filter":keys::page(&prefix,None).unwrap(),"sort":{"_id":1},"limit":16},"verbosity":"executionStats"}).await.unwrap();
    let plan = format!("{:?}", explain.get_document("queryPlanner").unwrap());
    assert!(plan.contains("_id_"), "{plan}");
    assert!(!plan.contains("COLLSCAN"), "{plan}");
    assert_eq!(
        explain
            .get_document("executionStats")
            .unwrap()
            .get_i32("nReturned")
            .unwrap(),
        16
    );
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn automatic_replay_excludes_old_minutes_and_explicit_retained_repair_admits_them() {
    let f = Fixture::new().await;
    f.bind().await;
    let lease = f.lease().await;
    let publication = f.input(&lease).await.freeze(f.samples(60.0), true).unwrap();
    f.store
        .publish_measured(&lease, &publication)
        .await
        .unwrap();
    f.store.release_measured(&lease).await.unwrap();
    let mut lease = f.lease().await;
    lease.now = f.minute + 31 * 86400;
    lease.stream.next_minute = lease.stream.capture_start + 60;
    lease.stream.replay_minute = lease.stream.capture_start;
    lease.stream.replay_turn = true;
    let input = f.store.prepare_measured(&lease).await.unwrap().unwrap();
    assert!(!input.replay);
    assert_ne!(input.minute, f.minute);
    let events = f.events().await;
    f.store.release_measured(&lease).await.unwrap();
    let request = super::super::repair::MeasuredRepair {
        source_stream: f.stream.id.clone(),
        minute: f.minute,
        evidence_digest: publication.digest,
        predecessor_event: events[1]["event_id"].as_str().unwrap().into(),
    };
    assert!(f.store.repair_measured(&request, true).await.unwrap());
    assert_eq!(f.events().await.len(), 2);
    let job = f
        .store
        .collection(INVENTORY)
        .find_one(keys::page(&keys::prefix("repair", "fixture"), None).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.store
            .collection(EVIDENCE)
            .find_one(doc! {"source_digest":job.get_str("result_manifest").unwrap()})
            .await
            .unwrap()
            .is_some(),
        "completed repair retains its proof even without a changed fact"
    );
    f.db.drop().await.unwrap();
}
