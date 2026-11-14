use super::*;
use futures_util::TryStreamExt;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::Mutex;

const AGE_INDEX: &str = "kind_1_state_1_reserved_at_1";
const LEASE_INDEX: &str = "kind_1_state_1_delete_lease_until_1";

struct Scan {
    returned: i64,
    keys: i64,
    docs: i64,
    command: Document,
    plan: String,
}

fn count(stats: &Document, key: &str) -> i64 {
    stats
        .get(key)
        .unwrap()
        .as_i64()
        .unwrap_or_else(|| i64::from(stats.get_i32(key).unwrap()))
}

async fn explain(db: &Database, active: usize, eligible: usize, command: Document) -> Scan {
    let mut find = Document::new();
    for key in ["find", "filter", "hint", "limit", "batchSize", "maxTimeMS"] {
        find.insert(key, command.get(key).unwrap().clone());
    }
    let result = db
        .run_command(doc! {"explain": &find, "verbosity": "executionStats"})
        .await
        .unwrap();
    let stats = result.get_document("executionStats").unwrap();
    let plan = serde_json::to_string(result.get_document("queryPlanner").unwrap()).unwrap();
    println!(
        "executionStats active={active} eligible={eligible} command={} stats={} plan={plan}",
        serde_json::to_string(&find).unwrap(),
        serde_json::to_string(stats).unwrap(),
    );
    Scan {
        returned: count(stats, "nReturned"),
        keys: count(stats, "totalKeysExamined"),
        docs: count(stats, "totalDocsExamined"),
        command: find,
        plan,
    }
}

async fn measured_candidates(active: usize, eligible: usize) -> (Scan, Scan) {
    let (db, quota, _) = fixture().await;
    let before = 10_000;
    let now = before + RECLAIM_GRACE.as_millis() as i64;
    let metadata = record().await;
    let mut eligible_sources = Vec::new();
    let rows: Vec<_> = (0..active + eligible)
        .map(|index| {
            let reservation = source(&metadata, format!("lease-index-{index}").as_bytes());
            let mut row = object_row(&reservation).unwrap();
            row.insert("state", "condemned");
            row.insert("reserved_at", DateTime::from_millis(1));
            if index < active || index % 2 == 0 {
                row.insert("delete_holder", uuid::Uuid::new_v4().to_string());
                row.insert("delete_fence", 1_i64);
                // Equality remains active; eligible rows alternate expired/missing leases.
                let until = if index < active { now } else { now - 1 };
                row.insert("delete_lease_until", DateTime::from_millis(until));
            }
            if index >= active {
                eligible_sources.push(reservation);
            }
            row
        })
        .collect();
    quota.rows.insert_many(rows).await.unwrap();
    let mut other = source(&metadata, b"other-owner-reserved");
    other.owner_hash = digest(b"other-index-owner");
    quota.reserve(&other).await.unwrap();
    age(&quota, &other).await;

    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && event.command.get_str("find").ok() == Some(QUOTA_COLLECTION)
                && event.command.contains_key("hint")
            {
                observed.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let reclaim = MongoDocumentReclaimStore::new(&client.database(db.name()), true);
    let candidates = reclaim.candidates(before, RECLAIM_BATCH).await.unwrap();
    let captured = commands.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    let mut scans = Vec::new();
    for command in captured {
        scans.push(explain(&db, active, eligible, command).await);
    }
    db.drop().await.unwrap();
    let condemned: Vec<_> = candidates.iter().filter(|row| row.condemned).collect();
    assert_eq!(condemned.len(), eligible.min(RECLAIM_BATCH));
    assert!(condemned
        .iter()
        .all(|row| eligible_sources.contains(&row.source)));
    let reserved: Vec<_> = candidates.iter().filter(|row| !row.condemned).collect();
    assert_eq!(reserved.len(), 1);
    assert_eq!(reserved[0].source, other);
    let reserved_scan = scans.pop().unwrap();
    (scans.pop().unwrap(), reserved_scan)
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_lease_index_bounds_actual_candidate_scans() {
    let mut measurements = Vec::new();
    for (active, eligible) in [(256, 4), (512, 4), (512, 72)] {
        measurements.push((
            active,
            eligible,
            measured_candidates(active, eligible).await,
        ));
    }
    // All samples are printed before assertions, preserving both RED scan counts.
    for (active, eligible, (condemned, reserved)) in measurements {
        let returned = eligible.min(RECLAIM_BATCH) as i64;
        assert_eq!(condemned.returned, returned);
        // Two OR ranges may examine terminal/seek keys. Four extra keys allow
        // that overhead, independent of the number of active leases.
        assert!(
            condemned.keys <= returned + 4 && condemned.docs <= returned + 4,
            "active={active}: eligible scan examined keys={} docs={} for {returned} returned",
            condemned.keys,
            condemned.docs,
        );
        assert_eq!(
            condemned.command.get_document("hint").unwrap(),
            &doc! {"kind":1,"state":1,"delete_lease_until":1}
        );
        assert!(condemned.plan.contains(LEASE_INDEX));
        assert_eq!(reserved.returned, 1);
        assert_eq!(
            reserved.command.get_document("hint").unwrap(),
            &doc! {"kind":1,"state":1,"reserved_at":1}
        );
        assert!(reserved.plan.contains(AGE_INDEX));
        for scan in [condemned, reserved] {
            assert_eq!(count(&scan.command, "limit"), RECLAIM_BATCH as i64);
            assert_eq!(count(&scan.command, "maxTimeMS"), 3_000);
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_candidate_indexes_are_additive_and_idempotent() {
    let (db, _, _) = fixture().await;
    MongoDocumentStore::new(db.clone())
        .ensure_indexes()
        .await
        .unwrap();
    let indexes: Vec<_> = db
        .collection::<Document>(QUOTA_COLLECTION)
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    db.drop().await.unwrap();
    for (name, keys) in [
        (AGE_INDEX, doc! {"kind":1,"state":1,"reserved_at":1}),
        (
            LEASE_INDEX,
            doc! {"kind":1,"state":1,"delete_lease_until":1},
        ),
    ] {
        let matching: Vec<_> = indexes.iter().filter(|index| index.keys == keys).collect();
        assert_eq!(matching.len(), 1, "missing or duplicate {name}");
        let options = matching[0].options.as_ref().unwrap();
        assert_eq!(options.name.as_deref(), Some(name));
        assert_ne!(options.unique, Some(true));
        assert_ne!(options.sparse, Some(true));
        assert!(options.partial_filter_expression.is_none());
        assert!(options.expire_after.is_none());
    }
}
