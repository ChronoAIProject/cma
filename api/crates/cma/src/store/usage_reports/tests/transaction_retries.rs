use super::read_models::binding;
use super::*;
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tokio::sync::Notify;

#[path = "transaction_cancellation.rs"]
mod cancellation;

#[derive(Default)]
struct Commands {
    started: Mutex<Vec<(String, Document)>>,
    hold_abort: AtomicBool,
    abort_started: Notify,
    resume_abort: Notify,
    terminal_command: Mutex<Option<&'static str>>,
    terminal_started: Notify,
    terminal_completed: Notify,
}
impl Commands {
    fn clear(&self) {
        self.started.lock().unwrap().clear();
    }
    fn count(&self, name: &str) -> usize {
        self.started
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .count()
    }
    fn attempts(&self) -> usize {
        self.started
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, d)| d.get_bool("startTransaction").ok() == Some(true))
            .count()
    }
}

struct Fixture {
    store: Arc<ReportingStore>,
    successor: Arc<ReportingStore>,
    commands: Arc<Commands>,
    app: String,
}
impl Fixture {
    async fn new() -> Self {
        let successor = fixture().await;
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let app = format!("report-txn-{}", uuid::Uuid::new_v4().simple());
        let commands = Arc::new(Commands::default());
        let capture = commands.clone();
        let mut options = ClientOptions::parse(uri).await.unwrap();
        configure_pool(&mut options, 4);
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::async_callback(move |event| {
            // Record inline: queued setup events must not satisfy a later
            // cancellation barrier or appear after its command log is cleared.
            if let CommandEvent::Started(event) = &event {
                capture
                    .started
                    .lock()
                    .unwrap()
                    .push((event.command_name.clone(), event.command.clone()));
            }
            match &event {
                CommandEvent::Started(event)
                    if capture.terminal_command.lock().unwrap().as_deref()
                        == Some(event.command_name.as_str()) =>
                {
                    capture.terminal_started.notify_one();
                }
                CommandEvent::Succeeded(event)
                    if capture.terminal_command.lock().unwrap().as_deref()
                        == Some(event.command_name.as_str()) =>
                {
                    capture.terminal_completed.notify_one();
                }
                _ => {}
            }
            let capture = capture.clone();
            Box::pin(async move {
                if let CommandEvent::Started(event) = event {
                    let abort = event.command_name == "abortTransaction";
                    if abort && capture.hold_abort.load(Ordering::SeqCst) {
                        capture.abort_started.notify_one();
                        capture.resume_abort.notified().await;
                    }
                }
            })
        }));
        let client = Client::with_options(options).unwrap();
        let mut store = (*successor).clone();
        store.persistence = store.persistence.with_client(client);
        Self {
            store: Arc::new(store),
            successor,
            commands,
            app,
        }
    }
    async fn fail(&self, commands: &[&str], times: i32, data: Document) {
        let mut data = data;
        data.insert("failCommands", commands.to_vec());
        data.insert("appName", &self.app);
        self.successor
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":times}, "data":data,
            })
            .await
            .unwrap();
    }
    async fn disable(&self) {
        self.successor
            .client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off",
            })
            .await
            .unwrap();
    }
    async fn reconcile(&self, holder: &str) -> Result<()> {
        let at = now() - 1000;
        self.store
            .reconcile_bindings(
                holder,
                "sandbox",
                vec![binding("sandbox", BindingKind::Home, at)],
                at,
            )
            .await
    }
    async fn prepare(&self) {
        assert!(self.store.lease("observer", "original", 70).await.unwrap());
        self.reconcile("original").await.unwrap();
        self.commands.clear();
    }
    async fn takeover(&self) {
        self.store.release("observer", "original").await.unwrap();
        assert!(self
            .successor
            .lease("observer", "successor", 70)
            .await
            .unwrap());
        self.commands.clear();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner"]
async fn fenced_writer_waits_for_abort_before_successor_reconciles() {
    let f = Fixture::new().await;
    f.prepare().await;
    f.takeover().await;
    let revision = f.successor.mapping_revision().await.unwrap();
    f.commands.hold_abort.store(true, Ordering::SeqCst);
    let store = f.store.clone();
    let pending = tokio::spawn(async move {
        // Close the existing binding and increment CONTROL before the last fence refuses.
        store
            .reconcile_bindings("original", "sandbox", vec![], now() - 900)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), f.commands.abort_started.notified())
        .await
        .unwrap();
    let returned_before_abort = pending.is_finished();
    f.commands.resume_abort.notify_one();
    assert!(
        !returned_before_abort,
        "transaction returned before its explicit abort completed"
    );
    assert!(matches!(
        pending.await.unwrap(),
        Err(MetricsError::Unavailable)
    ));
    assert_eq!(f.commands.attempts(), 1, "a fence refusal must not retry");
    f.successor
        .reconcile_bindings("successor", "sandbox", vec![], now() - 900)
        .await
        .unwrap();
    assert_eq!(f.successor.mapping_revision().await.unwrap(), revision + 1);
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn transient_body_restarts_and_fenced_retry_is_still_refused() {
    let f = Fixture::new().await;
    f.prepare().await;
    for holder in ["original", "original-after-takeover"] {
        if holder.ends_with("takeover") {
            f.takeover().await;
        }
        f.commands.clear();
        f.fail(
            &["update"],
            1,
            doc! {"errorCode":112,"errorLabels":["TransientTransactionError"]},
        )
        .await;
        let outcome = f.reconcile("original").await;
        f.disable().await;
        assert_eq!(f.commands.attempts(), 2);
        if holder == "original" {
            outcome.unwrap();
        } else {
            assert!(matches!(outcome, Err(MetricsError::Unavailable)));
            assert_eq!(f.commands.count("commitTransaction"), 0);
            assert_eq!(f.commands.count("abortTransaction"), 2);
        }
        assert_eq!(f.successor.mapping_revision().await.unwrap(), 1);
    }
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn transient_exhaustion_and_nontransient_failures_rollback() {
    let f = Fixture::new().await;
    f.prepare().await;
    for (labels, attempts) in [(vec!["TransientTransactionError"], 5), (vec![], 1)] {
        f.commands.clear();
        f.fail(&["update"], 30, doc! {"errorCode":112,"errorLabels":labels})
            .await;
        let outcome = f.reconcile("original").await;
        f.disable().await;
        assert!(matches!(outcome, Err(MetricsError::Unavailable)));
        assert_eq!(f.commands.attempts(), attempts);
        assert_eq!(f.commands.count("abortTransaction"), attempts);
        assert_eq!(f.successor.mapping_revision().await.unwrap(), 1);
    }
    f.reconcile("original").await.unwrap();
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn uncertain_commits_never_repeat_binding_effects() {
    let f = Fixture::new().await;
    f.prepare().await;
    for (times, committed) in [(2, false), (2, true), (20, true)] {
        f.commands.clear();
        let data = if committed {
            doc! {"writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"injected commit uncertainty"}}
        } else {
            doc! {"errorCode":50,"errorLabels":["UnknownTransactionCommitResult"]}
        };
        f.fail(&["commitTransaction"], times, data).await;
        let outcome = f.reconcile("original").await;
        f.disable().await;
        if times == 20 {
            assert!(matches!(outcome, Err(MetricsError::Unavailable)));
        } else {
            outcome.unwrap();
        }
        assert_eq!(
            f.commands.attempts(),
            1,
            "uncertain commits must not replay the body"
        );
        assert_eq!(
            f.commands.count("commitTransaction"),
            if times == 20 { 5 } else { 3 }
        );
        assert_eq!(f.successor.mapping_revision().await.unwrap(), 1);
    }
    let observer = f
        .successor
        .collection(CONTROL)
        .find_one(doc! {"_id":"observer"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observer.get_i64("control_write_fence").unwrap(), 4);
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn failed_abort_preserves_admission_budget_refusal() {
    let f = Fixture::new().await;
    let mut store = (*f.store).clone();
    store.budgets.queue = 0;
    f.fail(&["abortTransaction"], 10, doc! {"errorCode":2})
        .await;
    let result = store
        .admit_new(
            scope("owner", false),
            ReportInput::default().validate(now()).unwrap(),
            now(),
            "abort-budget".into(),
            None,
        )
        .await;
    f.disable().await;
    assert!(
        matches!(result, Err(MetricsError::Busy)),
        "abort must preserve Busy: {result:?}"
    );
    assert!(f.commands.count("abortTransaction") > 0);
    // The driver intentionally ignores server abort errors; release the owned
    // fixture transaction after disabling the injected failure.
    let abort = f
        .commands
        .started
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(name, _)| name == "abortTransaction")
        .unwrap()
        .1
        .clone();
    f.successor
        .client
        .database("admin")
        .run_command(doc! {"killSessions":[abort.get_document("lsid").unwrap()]})
        .await
        .unwrap();
    assert_eq!(f.commands.attempts(), 1);
    assert_eq!(
        f.successor
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn known_aborted_commit_restarts_the_entire_fenced_transaction() {
    let f = Fixture::new().await;
    f.prepare().await;
    f.fail(
        &["commitTransaction"],
        1,
        doc! {
            "errorCode":112,"errorLabels":["TransientTransactionError"]
        },
    )
    .await;
    let outcome = f.reconcile("original").await;
    f.disable().await;
    outcome.unwrap();
    assert_eq!(f.commands.attempts(), 2);
    let observer = f
        .successor
        .collection(CONTROL)
        .find_one(doc! {"_id":"observer"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observer.get_i64("control_write_fence").unwrap(), 2);
    assert_eq!(f.successor.mapping_revision().await.unwrap(), 1);
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn admission_retry_reuses_committed_identity_before_charging_budget() {
    let f = Fixture::new().await;
    let mut store = (*f.store).clone();
    store.budgets.tenant_bytes = store.budgets.report_bytes;
    let at = now();
    let query = ReportInput::default().validate(at).unwrap();
    let winner = store
        .admit_new(
            scope("owner", false),
            query.clone(),
            at,
            "same-request".into(),
            None,
        )
        .await
        .unwrap();
    f.commands.clear();
    f.fail(
        &["update"],
        1,
        doc! {"errorCode":112,"errorLabels":["TransientTransactionError"]},
    )
    .await;
    let contender = store
        .admit_new(
            scope("owner", false),
            query.clone(),
            at,
            "same-request".into(),
            None,
        )
        .await;
    f.disable().await;
    assert_eq!(contender.unwrap().report_id, winner.report_id);
    assert_eq!(f.commands.attempts(), 2);
    assert!(matches!(
        store
            .admit_new(
                scope("owner", false),
                query,
                at,
                "different-request".into(),
                None
            )
            .await,
        Err(MetricsError::Busy)
    ));
    assert_eq!(
        f.successor
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.store.db.drop().await.unwrap();
}
