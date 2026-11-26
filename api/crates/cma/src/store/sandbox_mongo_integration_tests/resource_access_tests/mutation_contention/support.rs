use super::*;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Default)]
pub(super) struct Trace {
    pub commands: Mutex<Vec<(String, Document)>>,
    pub pause: AtomicBool,
    pub entered: tokio::sync::Notify,
    resume: (Mutex<bool>, std::sync::Condvar),
}

impl Trace {
    pub fn release(&self) {
        *self.resume.0.lock().unwrap() = true;
        self.resume.1.notify_all();
    }

    pub fn writes(&self) -> usize {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == "update")
            .count()
    }

    pub fn snapshots(&self) -> usize {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, command)| {
                name == "aggregate" && matches!(command.get_str("aggregate"), Ok("agents"))
            })
            .count()
    }
}

pub(super) struct Fixture {
    pub admin: Client,
    pub name: String,
    pub app: String,
    pub store: MongoSandboxStore,
    pub target: ResourceTarget,
    pub before: ResourceAccessSnapshot,
    pub trace: Arc<Trace>,
}

impl Fixture {
    pub async fn new(failpoints: bool) -> Self {
        let variable = if failpoints {
            "CHRONO_TEST_MONGODB_FAILPOINT_URI"
        } else {
            "CHRONO_TEST_MONGODB_URI"
        };
        let uri = std::env::var(variable).expect("disposable replica set required");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("cma_access_cas_{suffix}");
        let app = format!("access-cas-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let trace = Arc::new(Trace::default());
        let observed = trace.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                let pause =
                    event.command_name == "update" && observed.pause.swap(false, Ordering::SeqCst);
                observed
                    .commands
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
                if pause {
                    observed.entered.notify_one();
                    let (ready, wake) = &observed.resume;
                    let (_ready, timeout) = wake
                        .wait_timeout_while(ready.lock().unwrap(), Duration::from_secs(5), |r| !*r)
                        .unwrap();
                    assert!(!timeout.timed_out(), "concurrent writer must release fence");
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let (store, target) = fixture(&client, &name).await;
        let before = store.resource_access_snapshot(&target).await.unwrap();
        trace.commands.lock().unwrap().clear();
        Self {
            admin,
            name,
            app,
            store,
            target,
            before,
            trace,
        }
    }

    pub async fn fail_writes(&self, times: i32, labels: &[&str]) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":times},
                "data":{"failCommands":["update"], "appName":&self.app,
                    "errorCode":112_i32, "errorLabels":labels.to_vec()},
            })
            .await
            .unwrap();
    }

    pub async fn clear_failpoint(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off",
            })
            .await
            .unwrap();
    }

    pub async fn assert_no_publication(&self) {
        assert_eq!(
            self.store
                .resource_shares
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            self.store
                .resource_policies
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            self.store
                .resource_share_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            self.store
                .resource_access_snapshot(&self.target)
                .await
                .unwrap(),
            self.before
        );
        for rows in [
            &self.store.sandboxes,
            &self.store.workspaces,
            &self.store.worktrees,
            &self.store.agents,
        ] {
            assert_eq!(
                rows.count_documents(doc! {"resource_share_guard":{"$exists":true}})
                    .await
                    .unwrap(),
                0
            );
        }
    }

    pub async fn destroy(self) {
        self.admin.database(&self.name).drop().await.unwrap();
    }
}
