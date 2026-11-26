use cma_image_manager::{
    cloud::{BuildExecutor, Executor, Observation},
    model::{Build, BuildInput, BuildPhase, CreateImage, Error, Result},
    schema,
    store::Store,
    worker,
};
use mongodb::{
    bson::doc,
    options::{ClientOptions, Credential},
    Client, Database,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Notify};

#[path = "worker_dispatch/failures.rs"]
mod failures;

struct Fixture {
    admin: Client,
    db: Database,
    dispatcher: Store,
    observer: Store,
    app: String,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_handler(None).await
    }

    async fn with_handler(
        handler: Option<mongodb::event::EventHandler<mongodb::event::command::CommandEvent>>,
    ) -> Self {
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_im_worker_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&name);
        db.run_command(schema::runtime_role(&name)).await.unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        db.run_command(doc! {"createUser":"worker", "pwd":&password,
        "roles":[{"role":"cma_im_runtime","db":&name}]})
            .await
            .unwrap();
        schema::migrate(&db).await.unwrap();
        let app = format!("dispatcher-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username("worker".to_owned())
                .password(password)
                .source(name.clone())
                .build(),
        );
        options.app_name = Some(app.clone());
        options.retry_writes = Some(false);
        options.command_event_handler = handler;
        let dispatcher = Client::with_options(options.clone()).unwrap();
        options.app_name = Some(format!("observer-{}", uuid::Uuid::new_v4().simple()));
        options.command_event_handler = None;
        let observer = Client::with_options(options).unwrap();
        Self {
            dispatcher: Store::open(dispatcher.database(&name)).await.unwrap(),
            observer: Store::open(observer.database(&name)).await.unwrap(),
            admin,
            db,
            app,
        }
    }

    async fn build(&self) -> Build {
        self.build_with_content(None).await
    }

    async fn build_with_content(&self, dockerfile_content: Option<String>) -> Build {
        let image = self
            .dispatcher
            .create_image(
                "alice",
                &uuid::Uuid::new_v4().to_string(),
                CreateImage {
                    name: "worker-test".into(),
                    description: String::new(),
                },
            )
            .await
            .unwrap();
        self.dispatcher
            .create_build(
                "alice",
                &image.id,
                &image.id,
                BuildInput {
                    repository: "https://github.com/example/runtime".into(),
                    commit: "a".repeat(40),
                    dockerfile: "Dockerfile".into(),
                    context: ".".into(),
                    dockerfile_content,
                },
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        self.db.drop().await.unwrap();
    }

    async fn submitting(&self, rejected: bool) -> Pending {
        let (submission, mut started) = Submission::new(rejected);
        let executor: Executor = submission.clone();
        let store = self.dispatcher.clone();
        let task = tokio::spawn(async move {
            let mut worker = worker::Worker::default();
            let result = worker.tick(&store, &executor).await;
            (worker, result)
        });
        let claim = tokio::time::timeout(std::time::Duration::from_secs(10), started.recv())
            .await
            .unwrap()
            .unwrap();
        Pending {
            submission,
            claim,
            task,
        }
    }
}

struct Pending {
    submission: Arc<Submission>,
    claim: Build,
    task: tokio::task::JoinHandle<(worker::Worker, Result<()>)>,
}

impl Pending {
    async fn finish(self) -> (worker::Worker, Result<()>) {
        self.submission.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(10), self.task)
            .await
            .unwrap()
            .unwrap()
    }
}

struct Submission {
    started: mpsc::Sender<Build>,
    release: Notify,
    submissions: AtomicUsize,
    rejected: bool,
}

impl Submission {
    fn new(rejected: bool) -> (Arc<Self>, mpsc::Receiver<Build>) {
        let (started, receiver) = mpsc::channel(1);
        (
            Arc::new(Self {
                started,
                release: Notify::new(),
                submissions: AtomicUsize::new(0),
                rejected,
            }),
            receiver,
        )
    }
}

#[async_trait::async_trait]
impl BuildExecutor for Submission {
    async fn submit(&self, build: &Build) -> Result<()> {
        assert_eq!(self.submissions.fetch_add(1, Ordering::SeqCst), 0);
        self.started.send(build.clone()).await.unwrap();
        self.release.notified().await;
        if self.rejected {
            Err(Error::BuildRejected)
        } else {
            Err(Error::Unavailable)
        }
    }

    async fn observe(&self, _: &Build) -> Result<Option<Observation>> {
        Ok(None)
    }

    async fn logs(&self, _: &Build) -> Result<serde_json::Value> {
        unreachable!("the worker does not fetch logs")
    }
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn definitive_rejection_survives_another_workers_observation_defer() {
    let fixture = Fixture::new().await;
    let build = fixture.build().await;
    let pending = fixture.submitting(true).await;
    let submission = pending.submission.clone();
    let executor: Executor = submission.clone();
    worker::Worker::default()
        .tick(&fixture.observer, &executor)
        .await
        .unwrap();
    let deferred = fixture.observer.build("alice", &build.id).await.unwrap();
    assert!(deferred.revision > pending.claim.revision);
    assert_eq!(deferred.phase, BuildPhase::Dispatching);
    pending.finish().await.1.unwrap();
    let settled = fixture.observer.build("alice", &build.id).await.unwrap();
    assert_eq!(settled.phase, BuildPhase::Failed);
    assert_eq!(settled.error.as_deref(), Some("submission_rejected"));
    assert!(settled.cloud_id.is_none());
    let replay = fixture
        .observer
        .create_build("alice", &build.image_id, &build.image_id, build.source)
        .await
        .unwrap();
    assert_eq!(replay, settled);
    for _ in 0..3 {
        worker::Worker::default()
            .tick(&fixture.observer, &executor)
            .await
            .unwrap();
    }
    assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn unknown_submission_is_never_replayed_after_defer_or_worker_restart() {
    let fixture = Fixture::new().await;
    let build = fixture.build().await;
    let pending = fixture.submitting(false).await;
    let submission = pending.submission.clone();
    fixture
        .observer
        .defer_observation(&pending.claim)
        .await
        .unwrap();
    pending.finish().await.1.unwrap();
    let executor: Executor = submission.clone();
    for _ in 0..4 {
        worker::Worker::default()
            .tick(&fixture.observer, &executor)
            .await
            .unwrap();
    }
    let current = fixture.observer.build("alice", &build.id).await.unwrap();
    assert_eq!(current.phase, BuildPhase::Dispatching);
    assert_eq!(
        current.error.as_deref(),
        Some("submission_outcome_unconfirmed")
    );
    assert!(current.cloud_id.is_none());
    assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn late_rejection_preserves_newer_provider_and_terminal_observations() {
    let fixture = Fixture::new().await;
    for phase in [
        BuildPhase::Running,
        BuildPhase::Succeeded,
        BuildPhase::Failed,
        BuildPhase::Cancelled,
    ] {
        let build = fixture.build().await;
        let pending = fixture.submitting(true).await;
        let submission = pending.submission.clone();
        let cloud_id = uuid::Uuid::new_v4().to_string();
        let digest = (phase == BuildPhase::Succeeded).then(|| format!("sha256:{}", "a".repeat(64)));
        assert!(fixture
            .observer
            .observe_build(&pending.claim, &cloud_id, phase.clone(), digest.as_deref())
            .await
            .unwrap());
        pending.finish().await.1.unwrap();
        let current = fixture.observer.build("alice", &build.id).await.unwrap();
        assert_eq!(current.phase, phase);
        assert_eq!(current.cloud_id.as_deref(), Some(cloud_id.as_str()));
        assert_eq!(current.digest, digest);
        assert!(current.error.is_none());
        assert_eq!(submission.submissions.load(Ordering::SeqCst), 1);
    }
    fixture.cleanup().await;
}
