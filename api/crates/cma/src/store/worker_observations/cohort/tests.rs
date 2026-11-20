use super::*;
use fold::{Denominator, Summary};
use mongodb::bson::Timestamp;

fn window() -> Window {
    Window {
        name: "previous_day",
        from: 1_700_006_400,
        to: 1_700_092_800,
        observed: 1_700_096_400,
    }
}
fn stamp(seconds: u64) -> String {
    layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds))
}
fn row(id: &str, state: &str) -> Document {
    doc! {"_id":id,"command_id":id,"agent_id":"synthetic-agent","record_agent_id":"synthetic-agent",
    "kind":"message","source":"api","continuation":false,"status":state,"record_status":state,
    "revision":1_i64,"record_revision":1_i64,"admitted_at":stamp(window().from + 1)}
}
fn finish(summary: Summary) -> String {
    summary
        .finish(Timestamp {
            time: 1_700_096_400,
            increment: 1,
        })
        .unwrap()
}
fn metric(text: &str, name: &str, labels: &str) -> u64 {
    let prefix = format!("cma_task_cohort_{name}{{window=\"previous_day\"{labels}}} ");
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn known_cohort_separates_controls_waits_and_cross_window_completion() {
    let mut summary = Summary::new(
        window(),
        Denominator {
            records: 7,
            tasks: 6,
        },
    );
    for (id, state) in [
        ("q", "queued"),
        ("r", "in_progress"),
        ("w", "waiting_action"),
        ("s", "completed"),
        ("f", "failed"),
        ("c", "cancelled"),
    ] {
        let mut row = row(id, state);
        if id == "s" {
            row.insert("source", "trigger");
        }
        if id == "w" {
            row.insert("continuation", true);
        }
        summary.observe(row).unwrap();
    }
    let mut control = row("control", "completed");
    control.insert("kind", "interrupt");
    summary.observe(control).unwrap();
    let text = finish(summary);
    assert_eq!(metric(&text, "accepted_tasks", ""), 6);
    assert_eq!(metric(&text, "control_commands", ""), 1);
    assert_eq!(metric(&text, "tasks", ",status=\"completed\""), 1);
    assert_eq!(metric(&text, "tasks", ",status=\"waiting_action\""), 1);
    assert_eq!(metric(&text, "continuation_tasks", ""), 1);
    assert_eq!(metric(&text, "from_timestamp_seconds", ""), window().from);
    assert_eq!(
        metric(&text, "unfinished_oldest_age_seconds", ""),
        window().observed - window().from - 1
    );
    assert_eq!(
        metric(
            &text,
            "diagnostic_tasks",
            ",reason=\"terminal_evidence_unchecked\""
        ),
        3
    );
    assert_eq!(
        metric(&text, "evidence_supported", ",source=\"native_timestamp\""),
        0
    );
    assert_eq!(metric(&text, "classification_complete", ""), 1);
}

#[test]
fn malformed_and_new_statuses_cannot_disappear_from_the_denominator() {
    let mut summary = Summary::new(
        window(),
        Denominator {
            records: 3,
            tasks: 3,
        },
    );
    let mut bad = row("bad", "completed");
    bad.insert("record_status", "in_progress");
    summary.observe(bad).unwrap();
    summary.observe(row("future", "new_state")).unwrap();
    summary.observe(row("good", "completed")).unwrap();
    let text = finish(summary);
    assert_eq!(metric(&text, "accepted_tasks", ""), 3);
    assert_eq!(metric(&text, "tasks", ",status=\"completed\""), 1);
    assert_eq!(metric(&text, "invalid_records", ""), 2);
    assert_eq!(metric(&text, "classification_complete", ""), 0);
    let mut incomplete = Summary::new(
        window(),
        Denominator {
            records: 2,
            tasks: 2,
        },
    );
    incomplete.observe(row("one", "queued")).unwrap();
    assert_eq!(
        incomplete.finish(Timestamp {
            time: 1,
            increment: 1
        }),
        Err("invalid_snapshot")
    );
}

#[test]
fn duplicate_identity_fails_closed_and_projection_never_selects_payloads() {
    let mut summary = Summary::new(
        window(),
        Denominator {
            records: 2,
            tasks: 2,
        },
    );
    summary.observe(row("one", "queued")).unwrap();
    assert_eq!(
        summary.observe(row("one", "queued")),
        Err("invalid_snapshot")
    );
    let projection = fold::projection().to_string();
    for sensitive in [
        "input",
        "metadata",
        "owner",
        "authority",
        "reference",
        "queued_input",
    ] {
        assert!(!projection.contains(sensitive));
    }
    let windows = windows(1_700_096_400);
    assert_eq!(windows[1].to - windows[1].from, 3_600);
    assert_eq!(windows[2].to - windows[2].from, 86_400);
    assert_eq!(windows[3].to - windows[3].from, 604_800);
    assert_eq!(windows[3].to % 86_400, 0);
}

#[test]
fn held_delivery_is_diagnostic_and_never_an_extra_lifecycle_state() {
    let mut summary = Summary::new(
        window(),
        Denominator {
            records: 1,
            tasks: 1,
        },
    );
    let mut held = row("held", "waiting_action");
    held.insert("work", doc! {"lease":null,"failure":null,"dispatch":{
        "claim":{"agent_id":"synthetic-agent","command_id":"held","holder":"synthetic-worker","fence":1_i64},
        "operation_sequence":2_i64,"turn_sequence":3_i64,"runtime_generation":1_i64,
        "fenced_at":stamp(window().from + 1),"delivery":"recovery_required"}});
    summary.observe(held).unwrap();
    let text = finish(summary);
    assert_eq!(metric(&text, "accepted_tasks", ""), 1);
    assert_eq!(metric(&text, "tasks", ",status=\"waiting_action\""), 1);
    for reason in ["waiting_action", "recovery_required", "delivery_uncertain"] {
        assert_eq!(
            metric(&text, "diagnostic_tasks", &format!(",reason=\"{reason}\"")),
            1
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable MongoDB 5+ replica set"]
async fn snapshot_census_bounds_and_failures_are_real_mongo_contracts() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!("cma_cohort_{}", uuid::Uuid::new_v4().simple()));
    let commands = crate::store::durability::majority_collection::<Document>(&db, "agent_commands");
    commands
        .insert_one(doc! {"_id":"outside","record":{"receipt":{"admitted_at":stamp(window().to)}}})
        .await
        .unwrap();
    // Install via the reviewed migration descriptor, not a serving/startup call.
    let index = crate::store::migrations::command_cohort_index::index();
    commands
        .create_index(
            mongodb::IndexModel::builder()
                .keys(index.get_document("key").unwrap().clone())
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name(index.get_str("name").unwrap().to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    for (id, state, kind) in [
        ("success", "completed", "message"),
        ("running", "in_progress", "message"),
        ("control", "completed", "interrupt"),
    ] {
        let document = doc! {"_id":id,"agent_id":"synthetic-agent","status":state,"revision":1_i64,
        "record":{"status":state,"revision":1_i64,"receipt":{"admitted_at":stamp(window().from),
            "envelope":{"command_id":id,"agent_id":"synthetic-agent","source":{"kind":"trigger"},"command":{"type":kind,"input":"never exported"}}}}};
        commands.insert_one(document).await.unwrap();
    }
    let cache = Cache::default();
    let observer = Observer::new(db.clone(), cache.clone());
    let explanation = db.run_command(doc! {"explain":{"find":"agent_commands","filter":window().filter(),"hint":INDEX},"verbosity":"executionStats"}).await.unwrap();
    let stats = explanation.get_document("executionStats").unwrap();
    let count = |key| {
        stats
            .get_i64(key)
            .or_else(|_| stats.get_i32(key).map(i64::from))
            .unwrap()
    };
    assert_eq!(count("nReturned"), 3);
    assert!(count("totalKeysExamined") <= 4);
    assert!(count("totalDocsExamined") <= 3);
    let before = observer.read(window()).await.unwrap();
    assert_eq!(metric(&before, "accepted_tasks", ""), 2);
    assert_eq!(metric(&before, "control_commands", ""), 1);
    assert_eq!(metric(&before, "classification_complete", ""), 1);
    assert!(!before.contains("never exported"));
    observer.sample().await;
    let exported = cache.render();
    for window in WINDOWS {
        assert!(exported.contains(&format!(
            "cma_task_cohort_sample_success{{window=\"{window}\"}} 1"
        )));
    }
    // Another replica reads the same global population, never a partition.
    let replica = Observer::new(db.clone(), Cache::default());
    assert_eq!(
        metric(&replica.read(window()).await.unwrap(), "accepted_tasks", ""),
        2
    );
    // Establish the denominator snapshot, then race a committed transition and
    // new admission before reading classification rows in that same session.
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let denominator = observer.denominator(window(), &mut session).await.unwrap();
    commands
        .update_one(
            doc! {"_id":"running"},
            doc! {"$set":{"status":"completed","record.status":"completed"}},
        )
        .await
        .unwrap();
    let mut cursor = commands
        .aggregate([
            doc! {"$match":window().filter()},
            doc! {"$project":fold::projection()},
        ])
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap();
    let mut summary = Summary::new(window(), denominator);
    while let Some(row) = cursor.stream(&mut session).try_next().await.unwrap() {
        summary.observe(row).unwrap();
    }
    let raced = finish(summary);
    assert_eq!(metric(&raced, "tasks", ",status=\"in_progress\""), 1);
    assert_eq!(
        metric(
            &observer.read(window()).await.unwrap(),
            "tasks",
            ",status=\"completed\""
        ),
        2
    );
    // Invalid classification remains a counted bad record, not healthy zero.
    commands
        .update_one(
            doc! {"_id":"success"},
            doc! {"$set":{"record.status":"failed"}},
        )
        .await
        .unwrap();
    assert_eq!(
        metric(
            &observer.read(window()).await.unwrap(),
            "invalid_records",
            ""
        ),
        1
    );
    let oversized: Vec<_> = (0..MAX_ROWS).map(|i| doc! {"_id":format!("over-{i}"),"record":{"receipt":{"admitted_at":stamp(window().from),"envelope":{"command":{"type":"message"}}}}}).collect();
    commands.insert_many(oversized).await.unwrap();
    assert_eq!(observer.read(window()).await, Err("budget"));
    commands
        .delete_many(doc! {"_id":{"$regex":"^over-"}})
        .await
        .unwrap();
    let too_large: Vec<_> = (0..9).map(|i| doc! {"_id":format!("large-{i}"),"record":{
        "receipt":{"admitted_at":stamp(window().from),"envelope":{"command":{"type":"message"}}},
        "work":{"unrecognized":"x".repeat(1_000_000)}}}).collect();
    commands.insert_many(too_large).await.unwrap();
    assert_eq!(observer.read(window()).await, Err("budget"));
    commands.drop_index(INDEX).await.unwrap();
    assert_eq!(observer.read(window()).await, Err("database"));
    commands
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"record.receipt.admitted_at":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name(INDEX.to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    assert_eq!(observer.read(window()).await, Err("database"));
    commands.drop().await.unwrap();
    // Mongo can ignore a hint on an absent collection and return an empty
    // snapshot. A missing source must not become a healthy zero population.
    assert_eq!(observer.read(window()).await, Err("database"));
    observer.sample().await;
    let exported = cache.render();
    for window in WINDOWS {
        assert!(exported.contains(&format!(
            "cma_task_cohort_sample_success{{window=\"{window}\"}} 0"
        )));
    }
    assert!(db.list_collection_names().await.unwrap().is_empty());
    db.drop().await.unwrap();
}
