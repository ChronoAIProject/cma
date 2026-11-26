use super::*;

pub(super) async fn failpoint(admin: &Client, app: &str, mode: &str) {
    let mut data = match mode {
        "committed" => doc! {
            "failCommands":["commitTransaction"],
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected accepted-work commit uncertainty"}
        },
        "uncommitted" => doc! {
            "failCommands":["commitTransaction"],"errorCode":50_i32,
            "errorLabels":["UnknownTransactionCommitResult"]
        },
        "abort" => doc! {"failCommands":["update"],"errorCode":2_i32},
        _ => panic!("unknown fault mode"),
    };
    data.insert("appName", app);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"alwaysOn","data":data
        })
        .await
        .unwrap();
}

pub(super) async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"off"
        })
        .await
        .unwrap();
}

pub(super) async fn clients() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_capacity_composition_{suffix}");
    let app = format!("capacity-composition-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    (admin, Client::with_options(options).unwrap(), name, app)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn capacity_composition_claim_abort_and_uncertainty_never_attach() {
    for mode in ["abort", "uncommitted", "committed"] {
        let (admin, client, name, app) = clients().await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (caller, request, original) = admitted(&store).await;
        let before = snapshot(&store, &request).await;
        let service = service(&store, false);
        let native = Native::default();
        failpoint(&admin, &app, mode).await;
        let outcome = service.run_accepted(&native, &request, "uncertain").await;
        disable(&admin).await;
        if mode == "abort" {
            assert_eq!(outcome, Err(RecoveryError::Unavailable));
        } else {
            assert_eq!(outcome, Ok(CommitOutcome::Unknown));
        }
        assert_eq!(native.attaches.load(Ordering::SeqCst), 0);
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
        if mode == "committed" {
            crate::store::test_visibility::majority_barrier(&admin, &name).await;
        }
        let after = snapshot(&store, &request).await;
        if mode == "committed" {
            let RecoveryLookup::Recorded(work) = service.lookup(&caller, &request).await.unwrap()
            else {
                panic!("committed claim must retain its exact receipt");
            };
            assert_eq!(work.phase, RecoveryPhase::Attaching);
            assert_eq!(work.attempts, 1);
            assert_eq!(after.publications, before.publications + 1);
            assert_eq!(after.agent.revision, before.agent.revision + 1);
        } else {
            assert_eq!(after, before, "{mode}: failed claim must be atomic");
            assert_eq!(
                service.lookup(&caller, &request).await.unwrap(),
                RecoveryLookup::Recorded(Box::new(original))
            );
        }
        client.database(&name).drop().await.unwrap();
    }
}

struct CompletionFault {
    native: Native,
    admin: Client,
    app: String,
    mode: &'static str,
}

#[async_trait::async_trait]
impl CapacityRecoveryRuntime for CompletionFault {
    type Attachment = (String, u64);
    async fn reattach_existing(
        &self,
        claim: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        let attachment = self.native.reattach_existing(claim).await?;
        // The claim is already durable. Only completion sees this fault.
        failpoint(&self.admin, &self.app, self.mode).await;
        Ok(attachment)
    }
    async fn acknowledge(
        &self,
        attachment: Self::Attachment,
        work: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        self.native.acknowledge(attachment, work).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn capacity_composition_completion_reconciles_only_the_exact_committed_result() {
    for mode in ["abort", "uncommitted", "committed"] {
        let (admin, client, name, app) = clients().await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (caller, request, _) = admitted(&store).await;
        let before = snapshot(&store, &request).await;
        let service = service(&store, false);
        let runtime = CompletionFault {
            native: Native::default(),
            admin: admin.clone(),
            app,
            mode,
        };
        let outcome = service.run_accepted(&runtime, &request, "completing").await;
        disable(&admin).await;
        assert_eq!(runtime.native.attaches.load(Ordering::SeqCst), 1);
        let after = snapshot(&store, &request).await;
        let RecoveryLookup::Recorded(work) = service.lookup(&caller, &request).await.unwrap()
        else {
            panic!("accepted work must retain its last committed receipt");
        };
        if mode == "committed" {
            assert_eq!(outcome, Ok(CommitOutcome::Recorded(work.as_ref().clone())));
            assert_eq!(work.phase, RecoveryPhase::Completed);
            assert_eq!(runtime.native.acknowledges.load(Ordering::SeqCst), 1);
            assert_eq!(after.publications, before.publications + 2);
            assert_eq!(after.agent.revision, before.agent.revision + 2);
        } else {
            if mode == "abort" {
                assert_eq!(outcome, Err(RecoveryError::Unavailable));
            } else {
                assert_eq!(outcome, Ok(CommitOutcome::Unknown));
            }
            assert_eq!(work.phase, RecoveryPhase::Attaching);
            assert_eq!(runtime.native.acknowledges.load(Ordering::SeqCst), 0);
            assert_eq!(after.publications, before.publications + 1);
            assert_eq!(after.agent.revision, before.agent.revision + 1);
        }
        assert_eq!(work.attempts, 1);
        client.database(&name).drop().await.unwrap();
    }
}
