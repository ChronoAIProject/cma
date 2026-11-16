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
                if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                    observed.transactions.lock().unwrap().insert(format!(
                        "{:?}/{:?}",
                        event.command.get("lsid"),
                        event.command.get("txnNumber")
                    ));
                }
                if !matches!(event.command.get_str("find"), Ok(value) if value == collection) {
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    for (command, failures, unknown) in [
        ("find", 1, false),
        ("update", 1, false),
        ("commitTransaction", 1, false),
        ("update", 5, false),
        ("commitTransaction", 5, true),
    ] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("suggestion_claim_{suffix}");
        let app = format!("suggestion-claim-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let starts = Arc::new(AtomicUsize::new(0));
        let observed = starts.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = enabled_store(&client, &name).await;
        seed(
            &store,
            &suffix,
            "Compare SQLite for local storage.",
            "SQLite suits a local-only app.",
        )
        .await;
        starts.store(0, Ordering::SeqCst);
        let labels = if unknown {
            vec!["UnknownTransactionCommitResult"]
        } else {
            vec!["TransientTransactionError"]
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":failures},
                "data":{"appName":&app,"failCommands":[command],
                    "errorCode":if unknown {1_i32} else {112_i32},"errorLabels":labels},
            })
            .await
            .unwrap();
        let now = unix_millis();
        let result = store.claim_suggestions(now).await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":"off"
            })
            .await
            .unwrap();
        let rows = store.suggestions.count_documents(doc! {}).await.unwrap();
        let attempts = starts.load(Ordering::SeqCst);
        client.database(&name).drop().await.unwrap();
        assert_eq!(rows, 1);
        if failures == 1 {
            let claim = result
                .expect("known-aborted suggestion claim must retry")
                .expect("pending suggestion must be claimed once");
            let mut expected = claim.clone();
            expected.status = SuggestionStatus::Pending;
            expected.deadline_ms = None;
            assert!(expected.claim(true, now));
            assert_eq!(claim, expected, "retry changed original claim time");
            assert_eq!(attempts, 2);
        } else {
            assert!(result.is_err());
            assert_eq!(attempts, if unknown { 1 } else { 5 });
        }
    }
}
