//! A result must survive an aborted context read without model redispatch.
use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize},
    Mutex,
};

#[derive(Default)]
struct Trace {
    armed: AtomicBool,
    reads: AtomicUsize,
    injected: AtomicUsize,
    transactions: Mutex<std::collections::BTreeSet<String>>,
}

fn inject_before_send(uri: String, app: String, unknown: bool) {
    // Mongo spawns async event callbacks without awaiting them. Join an isolated
    // runtime instead: the intercepted query cannot race its failpoint setup,
    // and no client task needs to run on the blocked test runtime.
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create isolated failpoint runtime")
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let control = Client::with_uri_str(uri)
                        .await
                        .map_err(|_| "isolated failpoint connection failed")
                        .expect("connect isolated failpoint client");
                    let mut labels = vec!["TransientTransactionError"];
                    if unknown {
                        labels.push("UnknownTransactionCommitResult");
                    }
                    let configured = control
                        .database("admin")
                        .run_command(doc! {
                            "configureFailPoint":"failCommand", "mode":{"times":1},
                            "data":{"appName":app,"failCommands":["find"],
                                "errorCode":112_i32,"errorLabels":labels},
                        })
                        .await;
                    control.shutdown().await;
                    configured
                        .map_err(|_| "isolated failpoint command failed")
                        .expect("acknowledge failpoint before intercepted query");
                })
                .await
                .expect("failpoint setup must finish within five seconds");
            });
    })
    .join()
    .expect("failpoint setup must finish before command callback returns");
}

struct Model {
    trace: Arc<Trace>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl SuggestionGenerator for Model {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.trace.armed.store(true, Ordering::SeqCst);
        Ok(SuggestionGeneration {
            output: r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"}]}"#.into(),
            usage: SuggestionUsage::default(),
        })
    }
}

async fn injected_log_context_failure(unknown: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("suggestion_context_{suffix}");
    let app = format!("suggestion-context-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let trace = Arc::new(Trace::default());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let control_uri = uri.clone();
    let observed_database = name.clone();
    let observed = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        if event.db != observed_database || !observed.armed.load(Ordering::SeqCst) {
            return;
        }
        if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
            observed.transactions.lock().unwrap().insert(format!(
                "{:?}/{:?}",
                event.command.get("lsid"),
                event.command.get("txnNumber")
            ));
        }
        if event.command.get_str("find").ok() != Some("agents") {
            return;
        }
        let read = observed.reads.fetch_add(1, Ordering::SeqCst);
        // The first Agent read verifies scope. The second opens the log range.
        if read != 1 {
            return;
        }
        inject_before_send(control_uri.clone(), app.clone(), unknown);
        observed.injected.fetch_add(1, Ordering::SeqCst);
    }));
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        &suffix,
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let model = Model {
        trace: trace.clone(),
        calls: AtomicUsize::new(0),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        SuggestionGenerationService::new(store.clone()).run_next(&model),
    )
    .await;
    trace.armed.store(false, Ordering::SeqCst);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"off"
        })
        .await
        .unwrap();
    let reading = read(&router(store.clone()), &agent, "owner").await.1;
    let second = SuggestionGenerationService::new(store)
        .run_next(&model)
        .await;
    client.database(&name).drop().await.unwrap();
    let result = result.expect("context retry must stay bounded");
    let attempts = trace.transactions.lock().unwrap().len();
    assert_eq!(trace.injected.load(Ordering::SeqCst), 1);
    if unknown {
        assert!(result.is_err());
        assert_eq!(reading["status"], "running");
        assert_eq!(attempts, 1);
    } else {
        assert!(
            result.unwrap(),
            "a known log read abort must retain the generated result"
        );
        assert_eq!(reading["status"], "ready");
        assert_eq!(attempts, 2);
    }
    assert!(!second.unwrap());
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn suggestion_context_driver_error_retries_known_abort() {
    injected_log_context_failure(false).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn suggestion_context_driver_error_preserves_uncertainty() {
    injected_log_context_failure(true).await;
}
