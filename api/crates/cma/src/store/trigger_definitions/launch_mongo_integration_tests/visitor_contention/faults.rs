use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::Mutex;

struct Observed {
    store: MongoTriggerDefinitionStore,
    app: String,
    trace: Arc<Mutex<Vec<(String, Document)>>>,
}

impl Observed {
    async fn new(seed: &Seeded, policy: LaunchAdmissionPolicy) -> Self {
        let app = format!("visitor-admission-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(app.clone());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let observed = trace.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
        }));
        let client = mongodb::Client::with_options(options).unwrap();
        let store =
            MongoTriggerDefinitionStore::open(&client, &seed.database, seed.profiles.clone())
                .await
                .unwrap();
        assert_eq!(
            store.launch_admission_policy(),
            LaunchAdmissionPolicy::Baseline
        );
        trace.lock().unwrap().clear();
        let store = store.with_launch_admission_policy(policy);
        assert!(
            trace.lock().unwrap().is_empty(),
            "policy injection performs no data or schema commands"
        );
        Self { store, app, trace }
    }

    async fn fault(&self, seed: &Seeded, commands: &[&str], count: i32, mut data: Document) {
        data.insert("appName", &self.app);
        data.insert(
            "failCommands",
            commands
                .iter()
                .map(|v| mongodb::bson::Bson::String((*v).into()))
                .collect::<Vec<_>>(),
        );
        seed.client
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": count }, "data": data,
            })
            .await
            .unwrap();
    }

    async fn clear_fault(&self, seed: &Seeded) {
        seed.client
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .unwrap();
    }

    fn inserts(&self, collection: &str) -> usize {
        self.trace
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, command)| {
                name == "insert" && command.get_str("insert").ok() == Some(collection)
            })
            .count()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set with failpoints"]
async fn transient_failures_keep_the_eight_attempt_baseline_and_enable_sixteen_attempts() {
    for (policy, attempts) in [
        (LaunchAdmissionPolicy::Baseline, 8),
        (LaunchAdmissionPolicy::VisitorAllowance, 16),
    ] {
        let seed = seeded("visitor_retry_exhaustion").await;
        let trigger = published(&seed, "exhaustion").await;
        let observed = Observed::new(&seed, policy).await;
        let candidate = admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT);
        observed
            .fault(
                &seed,
                &["insert"],
                attempts,
                doc! { "errorCode": 112, "errorLabels": ["TransientTransactionError"] },
            )
            .await;
        let result = observed.store.admit_launch(candidate.clone()).await;
        observed.clear_fault(&seed).await;
        assert!(
            matches!(result, Err(LaunchStoreError::Unavailable(ref detail)) if detail.contains("exhausted its retry budget")),
            "{result:?}"
        );
        assert_eq!(observed.inserts("visitor_launch_days"), attempts as usize);
        assert_eq!(observed.inserts(LAUNCHES), 0);
        assert_eq!(stats(&seed, &trigger).await.launches_total, 0);
        assert!(observed
            .store
            .admit_launch(candidate.clone())
            .await
            .unwrap()
            .was_created());
        assert!(!observed
            .store
            .admit_launch(candidate)
            .await
            .unwrap()
            .was_created());
        assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 1);
        seed.client.database(&seed.database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set with failpoints"]
async fn transient_insert_retries_but_permanent_driver_and_domain_refusals_do_not() {
    for policy in [
        LaunchAdmissionPolicy::Baseline,
        LaunchAdmissionPolicy::VisitorAllowance,
    ] {
        let seed = seeded("visitor_retry_classification").await;
        let trigger = published(&seed, "classification").await;
        let observed = Observed::new(&seed, policy).await;
        observed
            .fault(
                &seed,
                &["insert"],
                1,
                doc! { "errorCode": 112, "errorLabels": ["TransientTransactionError"] },
            )
            .await;
        let result = observed
            .store
            .admit_launch(admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT))
            .await;
        observed.clear_fault(&seed).await;
        assert!(result.unwrap().was_created());
        assert_eq!(observed.inserts("visitor_launch_days"), 2);
        assert_eq!(observed.inserts(LAUNCHES), 1);
        let other = admission(&trigger, "other", 1, BEFORE_MIDNIGHT);
        observed.trace.lock().unwrap().clear();
        observed
            .fault(&seed, &["insert"], 1, doc! { "errorCode": 121 })
            .await;
        let result = observed.store.admit_launch(other).await;
        observed.clear_fault(&seed).await;
        assert!(matches!(result, Err(LaunchStoreError::Unavailable(_))));
        assert_eq!(observed.inserts("visitor_launch_days"), 1);
        assert_eq!(observed.inserts(LAUNCHES), 0);
        for key in 1..5 {
            seed.store
                .admit_launch(admission(&trigger, "visitor", key, BEFORE_MIDNIGHT))
                .await
                .unwrap();
        }
        observed.trace.lock().unwrap().clear();
        let result = observed
            .store
            .admit_launch(admission(&trigger, "visitor", 5, BEFORE_MIDNIGHT))
            .await;
        assert!(matches!(
            result,
            Err(LaunchStoreError::VisitorQuota { limit: 5, .. })
        ));
        assert_eq!(observed.inserts("visitor_launch_days"), 0);
        assert_eq!(observed.inserts(LAUNCHES), 0);
        let reads = observed
            .trace
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, command)| {
                name == "find" && command.get_str("find").ok() == Some("visitor_launch_days")
            })
            .count();
        assert_eq!(reads, 1, "domain refusal must not repeat the transaction");
        assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 5);
        seed.client.database(&seed.database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set with failpoints"]
async fn unknown_commit_reconciles_without_replaying_the_admission_body() {
    for policy in [
        LaunchAdmissionPolicy::Baseline,
        LaunchAdmissionPolicy::VisitorAllowance,
    ] {
        let seed = seeded("visitor_unknown_commit").await;
        let trigger = published(&seed, "unknown").await;
        let observed = Observed::new(&seed, policy).await;
        let candidate = admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT);
        observed.fault(&seed, &["commitTransaction"], 5, doc! {
            "writeConcernError": { "code": 50, "codeName": "MaxTimeMSExpired", "errmsg": "synthetic commit ambiguity" },
        }).await;
        let result = observed.store.admit_launch(candidate.clone()).await;
        observed.clear_fault(&seed).await;
        assert!(result.unwrap().was_created());
        assert_eq!(observed.inserts(LAUNCHES), 1);
        assert_eq!(observed.inserts("visitor_launch_days"), 1);
        let commits = observed
            .trace
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == "commitTransaction")
            .count();
        assert_eq!(
            commits, 5,
            "the real commit-uncertainty branch must be reached"
        );
        assert!(!seed
            .store
            .admit_launch(candidate)
            .await
            .unwrap()
            .was_created());
        assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 1);
        assert_eq!(stats(&seed, &trigger).await.launches_total, 1);
        seed.client.database(&seed.database).drop().await.unwrap();
    }
}
