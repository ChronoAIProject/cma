use super::super::evidence_export::{self as export, Identity, Limits};
use super::*;

#[path = "evidence_export_failure_tests.rs"]
mod failure_tests;
#[path = "evidence_export_test_support.rs"]
mod support;
use std::sync::Arc;
use support::*;
use tokio::sync::Notify;

struct Pause {
    entered: Notify,
    resume: Notify,
}
#[async_trait::async_trait]
impl export::PageHook for Pause {
    async fn after_page(&self, collection: &str, page: usize) -> export::Result<()> {
        if collection == "sandboxes" && page == 1 {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn catalog_export_round_trips_real_codecs_through_node_without_losing_homes() {
    let fixture = ExportFixture::new().await;
    fixture.insert_home("a").await;
    fixture.insert_home("b").await;
    fixture.ready_history("a").await;
    let bytes = fixture.export(Limits::default(), &()).await.unwrap();
    let report = fixture.coverage(&bytes);
    assert_eq!(report["retained_homes"], 2);
    assert_eq!(report["backed_homes"], 1);
    assert_eq!(report["catalog_export_status"], "AVAILABLE");
    assert_eq!(report["status"], "FAIL");
    assert_eq!(report["homes"][1]["schedule_status"], "MISSING");
    fixture.ready_history("b").await;
    let bytes = fixture.export(Limits::default(), &()).await.unwrap();
    let report = fixture.coverage(&bytes);
    assert_eq!(report["status"], "PASS");
    for field in [
        "home_restore_status",
        "full_dr_status",
        "qualification_status",
        "historical_cadence_status",
    ] {
        assert_eq!(report[field], "NOT_RUN");
    }
    fixture.assert_read_only();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn catalog_export_pagination_keeps_one_cut_during_capture_replacement_and_retirement() {
    let fixture = ExportFixture::new().await;
    for id in ["a", "b", "c"] {
        fixture.insert_home(id).await;
    }
    fixture.ready_history("a").await;
    let db = fixture.fixture.client.database(&fixture.fixture.database);
    let parents = db.collection::<Document>("sandboxes");
    let original_b = parents.find_one(doc! {"_id": "b"}).await.unwrap().unwrap();
    let pause = Arc::new(Pause {
        entered: Notify::new(),
        resume: Notify::new(),
    });
    let mutate = async {
        tokio::time::timeout(Duration::from_secs(10), pause.entered.notified())
            .await
            .unwrap();
        let mut b = crate::store::sandboxes::read_sandbox(&original_b).unwrap();
        b.lifecycle
            .home
            .as_mut()
            .unwrap()
            .receipt
            .as_mut()
            .unwrap()
            .storage_generation = "replacement-source".into();
        parents
            .replace_one(
                doc! {"_id": "b"},
                crate::store::sandboxes::sandbox_document(&b, 0).unwrap(),
            )
            .await
            .unwrap();
        let mut c = crate::store::sandboxes::read_sandbox(
            &parents.find_one(doc! {"_id":"c"}).await.unwrap().unwrap(),
        )
        .unwrap();
        c.begin_retirement(layer2_core::now_rfc3339()).unwrap();
        c.lifecycle.home.as_mut().unwrap().released = true;
        parents
            .replace_one(
                doc! {"_id":"c"},
                crate::store::sandboxes::sandbox_document(&c, 0).unwrap(),
            )
            .await
            .unwrap();
        let mut captured = standalone_backup(HomeBackupState::Ready);
        captured.sandbox_id = "a".into();
        captured.epoch = 3;
        captured.id = "hb-a-3".into();
        db.collection(BACKUPS_COLLECTION)
            .insert_one(backup_document(&captured).unwrap())
            .await
            .unwrap();
        db.collection::<Document>(SCHEDULES_COLLECTION)
            .update_one(doc! {"_id":"a"}, doc! {"$inc":{"revision":1_i64}})
            .await
            .unwrap();
        pause.resume.notify_one();
    };
    let (result, ()) = tokio::join!(
        fixture.export(
            Limits {
                page_size: 1,
                ..Limits::default()
            },
            pause.as_ref()
        ),
        mutate
    );
    let bytes = result.unwrap();
    let exported = payload(&bytes);
    let rows = &exported["collections"]["sandboxes"]["rows"];
    assert_eq!(rows.as_array().unwrap().len(), 3);
    assert_eq!(
        rows[1],
        export::projection::project("sandboxes", original_b).unwrap()
    );
    assert_eq!(rows[2]["home"]["released"], false);
    assert_eq!(exported["collections"][BACKUPS_COLLECTION]["count"], 2);
    assert_eq!(
        exported["collections"][SCHEDULES_COLLECTION]["rows"][0]["revision"],
        1
    );
    let commands = fixture.commands.lock().unwrap().clone();
    let reads: Vec<_> = commands.iter().filter(|(name, _)| name == "find").collect();
    assert_eq!(reads.len(), 3);
    for (_, command) in &reads {
        assert_eq!(
            command.get_document("lsid").unwrap(),
            reads[0].1.get_document("lsid").unwrap()
        );
        assert_eq!(
            command
                .get_document("readConcern")
                .unwrap()
                .get_str("level")
                .unwrap(),
            "snapshot"
        );
    }
    let cut = reads[1]
        .1
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    assert_eq!(exported["snapshot"]["time"], cut.time);
    assert_eq!(exported["snapshot"]["increment"], cut.increment);
    assert!(commands.iter().any(|(name, _)| name == "getMore"));
    fixture.assert_read_only();
    fixture.cleanup().await;
}

struct FailPage {
    admin: Client,
    app_name: String,
    code: i32,
}
#[async_trait::async_trait]
impl export::PageHook for FailPage {
    async fn after_page(&self, collection: &str, page: usize) -> export::Result<()> {
        if collection == "sandboxes" && page == 1 {
            self.admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1},
                "data":{"failCommands":["getMore"], "appName": &self.app_name, "errorCode": self.code}}).await.unwrap();
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn catalog_export_midpage_driver_and_expired_snapshot_fail_without_restart() {
    let fixture = ExportFixture::new().await;
    for id in ["a", "b", "c"] {
        fixture.insert_home(id).await;
    }
    for code in [2, 286] {
        fixture.commands.lock().unwrap().clear();
        let hook = FailPage {
            admin: fixture.fixture.client.clone(),
            app_name: fixture.app_name.clone(),
            code,
        };
        let result = fixture
            .export(
                Limits {
                    page_size: 1,
                    ..Limits::default()
                },
                &hook,
            )
            .await;
        hook.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        assert_eq!(result.unwrap_err(), "catalog_export_read_failed");
        let commands = fixture.commands.lock().unwrap();
        assert_eq!(
            commands.iter().filter(|(name, _)| name == "find").count(),
            1
        );
        assert_eq!(
            commands
                .iter()
                .filter(|(name, _)| name == "getMore")
                .count(),
            1
        );
    }
    fixture.assert_read_only();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn catalog_export_limits_timeout_and_cancellation_never_publish_partial_success() {
    let fixture = ExportFixture::new().await;
    fixture.insert_home("a").await;
    fixture.insert_home("b").await;
    let bytes = fixture.export(Limits::default(), &()).await.unwrap();
    let previous = fixture.directory.join("earlier.json");
    export::cli::publish(&previous, &bytes).unwrap();
    for limits in [
        Limits {
            rows: 1,
            ..Limits::default()
        },
        Limits {
            pages: 1,
            page_size: 1,
            ..Limits::default()
        },
        Limits {
            source_bytes: 1,
            ..Limits::default()
        },
        Limits {
            output_bytes: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            fixture.export(limits, &()).await.unwrap_err(),
            "catalog_export_limit_exhausted"
        );
    }
    let pause = Pause {
        entered: Notify::new(),
        resume: Notify::new(),
    };
    let limits = Limits {
        timeout: Duration::from_millis(100),
        page_size: 1,
        ..Limits::default()
    };
    assert_eq!(
        fixture.export(limits, &pause).await.unwrap_err(),
        "catalog_export_timeout"
    );
    let destination = fixture.directory.join("cancelled.json");
    let pause = Pause {
        entered: Notify::new(),
        resume: Notify::new(),
    };
    let work = async {
        let bytes = fixture
            .export(
                Limits {
                    page_size: 1,
                    ..Limits::default()
                },
                &pause,
            )
            .await
            .unwrap();
        export::cli::publish(&destination, &bytes).unwrap();
    };
    tokio::select! { _ = work => panic!("cancelled export completed"),
    result = tokio::time::timeout(Duration::from_secs(10), pause.entered.notified()) => result.unwrap() }
    assert!(!destination.exists());
    assert_eq!(std::fs::read(previous).unwrap(), bytes);
    fixture.assert_read_only();
    fixture.cleanup().await;
}
