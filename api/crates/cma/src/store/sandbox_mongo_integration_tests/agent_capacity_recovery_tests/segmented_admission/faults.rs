//! Faults target the second (publication) transaction, after gate admission.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

#[derive(Default)]
struct Trace {
    armed: AtomicBool,
    inserts: Mutex<Vec<String>>,
    commits: AtomicUsize,
    session: Mutex<Option<Document>>,
}

async fn clients() -> (Client, Client, String, String, Arc<Trace>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("capacity-segmented-{suffix}");
    let name = format!("cma_capacity_segmented_{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let trace = Arc::new(Trace::default());
    let observed = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if !observed.armed.load(Ordering::SeqCst) {
            return;
        }
        if let CommandEvent::Started(event) = event {
            if event.command_name == "insert" {
                observed
                    .inserts
                    .lock()
                    .unwrap()
                    .push(event.command.get_str("insert").unwrap().into());
            }
            if event.command_name == "commitTransaction" {
                observed.commits.fetch_add(1, Ordering::SeqCst);
                *observed.session.lock().unwrap() =
                    Some(event.command.get_document("lsid").unwrap().clone());
            }
        }
    }));
    (
        admin,
        Client::with_options(options).unwrap(),
        name,
        app,
        trace,
    )
}

async fn inject(admin: &Client, app: &str, mode: &str) {
    let (skip, mut data) = match mode {
        // Gate admission and carrier inserts succeed; the seal insert fails
        // after carrier, parent fences and the Agent CAS have been attempted.
        "abort" => (2_i32, doc! {"failCommands":["insert"],"errorCode":2_i32}),
        // Gate-admission commit succeeds; uncertainty applies to publication.
        "uncommitted" => (
            1_i32,
            doc! {"failCommands":["commitTransaction"],"errorCode":50_i32,
            "errorLabels":["UnknownTransactionCommitResult"]},
        ),
        "committed" => (
            1_i32,
            doc! {"failCommands":["commitTransaction"],
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected admission uncertainty"}},
        ),
        _ => panic!("unknown fault mode"),
    };
    data.insert("appName", app);
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand",
        "mode":{"skip":skip},"data":data})
        .await
        .unwrap();
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn capacity_admission_retries_original_commit_after_lost_acknowledgements() {
    let (admin, client, name, app, trace) = clients().await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, caller, request) = exhausted(&store).await;
    let before = snapshot(&store, &request).await;
    // Exhaust the driver's one automatic commit retry. The application must
    // retry this same gate-admission commit before publishing storage recovery.
    admin
        .database("admin")
        .run_command(
            doc! {"configureFailPoint":"failCommand", "mode":{"times":2_i32},
            "data":{"failCommands":["commitTransaction"],"appName":&app,
                "errorCode":50_i32,"errorLabels":["UnknownTransactionCommitResult"]}},
        )
        .await
        .unwrap();
    trace.armed.store(true, Ordering::SeqCst);
    let outcome = service(&store, true).admit(&caller, &request).await;
    trace.armed.store(false, Ordering::SeqCst);
    disable(&admin).await;
    let CommitOutcome::Recorded(work) = outcome.unwrap() else {
        panic!("the original commit must settle after temporary acknowledgement loss")
    };
    assert!(trace.commits.load(Ordering::SeqCst) >= 4);
    assert_eq!(
        *trace.inserts.lock().unwrap(),
        vec![
            "agent_log_admissions",
            "agent_capacity_recovery_work",
            "agent_log_segments",
            "agent_log_publications"
        ],
        "commit retries must not rerun admission or publication mutations"
    );
    let after = snapshot(&store, &request).await;
    assert_admitted(&before, &after, &work);
    close_gates(&store).await;
    let restarted = MongoSandboxStore::open(&admin, &name).await.unwrap();
    assert_eq!(
        service(&restarted, false)
            .admit(&caller, &request)
            .await
            .unwrap(),
        CommitOutcome::Recorded(work)
    );
    assert_eq!(snapshot(&restarted, &request).await, after);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn segmented_capacity_admission_late_abort_rolls_back_carrier_and_successor() {
    let (admin, client, name, app, trace) = clients().await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, caller, request) = exhausted(&store).await;
    let before = snapshot(&store, &request).await;
    inject(&admin, &app, "abort").await;
    trace.armed.store(true, Ordering::SeqCst);
    let outcome = service(&store, true).admit(&caller, &request).await;
    trace.armed.store(false, Ordering::SeqCst);
    disable(&admin).await;
    assert_eq!(outcome, Err(RecoveryError::Unavailable));
    assert_eq!(
        *trace.inserts.lock().unwrap(),
        vec![
            "agent_log_admissions",
            "agent_capacity_recovery_work",
            "agent_log_segments"
        ]
    );
    assert_eq!(
        trace.commits.load(Ordering::SeqCst),
        1,
        "only gate admission commits"
    );
    let original = read_admission(&store, &request).await;
    assert_eq!(
        original.kind,
        writer_admission::AdmissionKind::CapacityRecovery
    );
    assert_eq!(snapshot(&store, &request).await, before);
    // Restart has no positive abort receipt. Absence must not grant a new
    // publication, even though this test knows the injected transaction aborted.
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    assert_eq!(
        service(&reopened, true)
            .admit(&caller, &request)
            .await
            .unwrap(),
        CommitOutcome::Unknown
    );
    assert_eq!(snapshot(&reopened, &request).await, before);
    client.database(&name).drop().await.unwrap();
}

async fn uncertain(mode: &str) {
    let (admin, client, name, app, trace) = clients().await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, caller, request) = exhausted(&store).await;
    let before = snapshot(&store, &request).await;
    inject(&admin, &app, mode).await;
    trace.armed.store(true, Ordering::SeqCst);
    let outcome = service(&store, true).admit(&caller, &request).await;
    trace.armed.store(false, Ordering::SeqCst);
    disable(&admin).await;
    assert_eq!(outcome, Ok(CommitOutcome::Unknown));
    assert!(
        trace.commits.load(Ordering::SeqCst) >= 2,
        "publication commit must be reached"
    );
    assert_eq!(
        *trace.inserts.lock().unwrap(),
        vec![
            "agent_log_admissions",
            "agent_capacity_recovery_work",
            "agent_log_segments",
            "agent_log_publications"
        ]
    );
    // Model process loss/server cleanup without conveying KnownAbort to a new
    // process. Release any uncommitted transaction's locks before observation.
    let original_session = trace.session.lock().unwrap().clone().unwrap();
    admin
        .database("admin")
        .run_command(doc! {"killSessions":[original_session]})
        .await
        .unwrap();
    if mode == "committed" {
        crate::store::test_visibility::majority_barrier(&admin, &name).await;
    }
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let after = snapshot(&reopened, &request).await;
    if mode == "committed" {
        let RecoveryLookup::Recorded(work) = service(&reopened, false)
            .lookup(&caller, &request)
            .await
            .unwrap()
        else {
            panic!("committed publication must reconcile its original witness")
        };
        assert_admitted(&before, &after, &work);
        close_gates(&reopened).await;
        assert_eq!(
            service(&reopened, false)
                .admit(&caller, &request)
                .await
                .unwrap(),
            CommitOutcome::Recorded(*work)
        );
    } else {
        assert_eq!(after, before);
        assert_eq!(
            service(&reopened, true)
                .lookup(&caller, &request)
                .await
                .unwrap(),
            RecoveryLookup::Unknown
        );
        for _ in 0..2 {
            assert_eq!(
                service(&reopened, true)
                    .admit(&caller, &request)
                    .await
                    .unwrap(),
                CommitOutcome::Unknown
            );
        }
    }
    assert_eq!(
        snapshot(&reopened, &request).await,
        after,
        "restart must not publish again"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn segmented_capacity_admission_committed_unknown_restart_keeps_original_witness() {
    uncertain("committed").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn segmented_capacity_admission_uncommitted_unknown_restart_never_replans() {
    uncertain("uncommitted").await;
}
