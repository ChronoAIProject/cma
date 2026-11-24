use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Condvar;
use tokio::sync::Notify;

#[derive(Clone, Copy)]
pub(super) enum Pause {
    WorkspaceWrite,
    RetryRead,
    CommitReply,
    ReconcileRead,
}

#[derive(Default)]
struct Probe {
    pause: Mutex<Option<Pause>>,
    entered: Notify,
    released: Mutex<bool>,
    release: Condvar,
    bodies: AtomicUsize,
    commits: AtomicUsize,
    commands: Mutex<Vec<(String, Document)>>,
    conflicts: AtomicUsize,
}

impl Probe {
    fn observe(&self, event: CommandEvent) {
        let matches = match event {
            CommandEvent::Started(e) => {
                self.commands
                    .lock()
                    .unwrap()
                    .push((e.command_name.clone(), e.command.clone()));
                let first_read = e.command.get_bool("startTransaction").unwrap_or(false);
                if first_read {
                    self.bodies.fetch_add(1, Ordering::SeqCst);
                }
                if e.command_name == "commitTransaction" {
                    self.commits.fetch_add(1, Ordering::SeqCst);
                }
                self.pause
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|pause| match pause {
                        Pause::WorkspaceWrite => {
                            e.command.get_str("update").ok() == Some(WORKSPACES)
                        }
                        Pause::RetryRead => first_read && self.bodies.load(Ordering::SeqCst) == 2,
                        Pause::ReconcileRead => {
                            e.command_name == "find" && self.commits.load(Ordering::SeqCst) == 5
                        }
                        Pause::CommitReply => false,
                    })
            }
            CommandEvent::Succeeded(e) => {
                e.command_name == "commitTransaction"
                    && self.commits.load(Ordering::SeqCst) == 5
                    && matches!(*self.pause.lock().unwrap(), Some(Pause::CommitReply))
            }
            CommandEvent::Failed(e) => {
                if is_transient(&e.failure) {
                    self.conflicts.fetch_add(1, Ordering::SeqCst);
                }
                false
            }
            _ => false,
        };
        if matches {
            self.pause.lock().unwrap().take();
            self.entered.notify_one();
            let (released, _) = self
                .release
                .wait_timeout_while(
                    self.released.lock().unwrap(),
                    Duration::from_secs(20),
                    |released| !*released,
                )
                .unwrap();
            assert!(*released, "test must release held database boundary");
        }
    }
}

pub(super) struct Fixture {
    pub store: MongoSandboxStore,
    pub peer: MongoSandboxStore,
    pub admin: Client,
    pub name: String,
    pub app: String,
    pub parent: Sandbox,
    pub original: Workspace,
    probe: Arc<Probe>,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::open("CHRONO_TEST_MONGODB_URI").await
    }
    pub async fn failpoints() -> Self {
        Self::open("CHRONO_TEST_MONGODB_FAILPOINT_URI").await
    }
    async fn open(variable: &str) -> Self {
        let uri = std::env::var(variable).unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_ready_{}", uuid::Uuid::new_v4().simple());
        let app = format!("ready-{}", uuid::Uuid::new_v4());
        let probe = Arc::new(Probe::default());
        let observe = probe.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler =
            Some(EventHandler::callback(move |event| observe.observe(event)));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let peer = MongoSandboxStore::open(&admin, &name).await.unwrap();
        let parent = sandbox("sbx_ready");
        store.create_sandbox(parent.clone()).await.unwrap();
        let original = workspace(&parent, "wks_ready", "ready");
        store.create_workspace(original.clone()).await.unwrap();
        probe.commands.lock().unwrap().clear();
        Self {
            store,
            peer,
            admin,
            name,
            app,
            parent,
            original,
            probe,
        }
    }
    pub fn arm(&self, pause: Pause) {
        self.probe.bodies.store(0, Ordering::SeqCst);
        self.probe.commits.store(0, Ordering::SeqCst);
        self.probe.commands.lock().unwrap().clear();
        *self.probe.pause.lock().unwrap() = Some(pause);
    }
    pub async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(15), self.probe.entered.notified())
            .await
            .unwrap();
    }
    pub fn release(&self) {
        *self.probe.released.lock().unwrap() = true;
        self.probe.release.notify_one();
    }
    pub fn bodies(&self) -> usize {
        self.probe.bodies.load(Ordering::SeqCst)
    }
    pub fn conflicts(&self) -> usize {
        self.probe.conflicts.load(Ordering::SeqCst)
    }
    pub fn commands(&self) -> Vec<(String, Document)> {
        self.probe.commands.lock().unwrap().clone()
    }
    pub async fn rows(&self) -> (Option<Document>, Option<Document>, Vec<SandboxEvent>) {
        (
            self.peer
                .sandboxes
                .find_one(doc! {"_id":self.parent.id.as_str()})
                .await
                .unwrap(),
            self.peer
                .workspaces
                .find_one(doc! {"_id":self.original.id.as_str()})
                .await
                .unwrap(),
            self.peer
                .sandbox_events(&self.parent.id, 0, 20)
                .await
                .unwrap(),
        )
    }
    pub async fn fail(&self, times: i32, mut data: Document) {
        data.insert("appName", &self.app);
        self.admin
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":data},
            )
            .await
            .unwrap();
    }
    pub async fn clear(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
    }
    pub async fn close(&self) {
        self.admin.database(&self.name).drop().await.unwrap();
    }
}
