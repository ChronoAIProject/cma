//! A result must survive an aborted context read without model redispatch.
use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{atomic::AtomicBool, Mutex};

#[derive(Default)]
struct Trace {
    armed: AtomicBool,
    reads: AtomicUsize,
    transactions: Mutex<std::collections::BTreeSet<String>>,
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    for (collection, skip, failures, unknown) in [
        ("agents", 0, 1, false),
        ("sandboxes", 0, 1, false),
        ("agent_commands", 0, 1, false),
        ("agents", 1, 1, false),
        ("agent_log", 0, 1, false),
        ("sandboxes", 0, 5, false),
        ("sandboxes", 0, 1, true),
    ] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("suggestion_context_{suffix}");
        let app = format!("suggestion-context-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let trace = Arc::new(Trace::default());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let control = admin.clone();
        let observed = trace.clone();
        options.command_event_handler = Some(EventHandler::async_callback(move |event| {
            let control = control.clone();
            let observed = observed.clone();
            let app = app.clone();
            Box::pin(async move {
                let CommandEvent::Started(event) = event else {
                    return;
                };
                if !observed.armed.load(Ordering::SeqCst) {
                    return;
                }
                if event.command.get_bool("startTransaction") == Ok(true) {
                    observed.transactions.lock().unwrap().insert(format!(
                        "{:?}/{:?}",
                        event.command.get("lsid"),
                        event.command.get("txnNumber")
                    ));
                }
                if event.command.get_str("find") != Ok(collection) {
                    return;
                }
                let read = observed.reads.fetch_add(1, Ordering::SeqCst);
                if read < skip || read >= skip + failures {
                    return;
                }
                let mut labels = vec!["TransientTransactionError"];
                if unknown {
                    labels.push("UnknownTransactionCommitResult");
                }
                control
                    .database("admin")
                    .run_command(doc! {
                        "configureFailPoint":"failCommand", "mode":{"times":1},
                        "data":{"appName":&app,"failCommands":["find"],
                            "errorCode":112_i32,"errorLabels":labels},
                    })
                    .await
                    .unwrap();
            })
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
        if failures == 1 && !unknown {
            assert!(
                result.is_ok_and(|ran| ran),
                "known-aborted {collection} context read discarded retained output"
            );
            assert_eq!(reading["status"], "ready");
            assert_eq!(attempts, 2, "context retry must use a fresh transaction");
        } else {
            assert!(result.is_err());
            assert_eq!(reading["status"], "running");
            assert_eq!(attempts, if unknown { 1 } else { 5 });
        }
        assert!(!second.unwrap());
        assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    }
}
