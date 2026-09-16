use super::*;
use futures_util::FutureExt;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Mutex,
};
use tokio::time::Instant;

struct DelayedRefresh {
    inner: Arc<dyn ChannelControlExecutionStore>,
    first_deadline: Mutex<Option<Instant>>,
}
#[async_trait::async_trait]
impl ChannelControlExecutionStore for DelayedRefresh {
    async fn read_control_execution(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
    ) -> Result<ChannelControlExecutionSnapshot, ChannelControlError> {
        let original = self.first_deadline.lock().unwrap().replace(deadline);
        if let Some(original) = original {
            assert_eq!(deadline, original, "refresh cannot renew the attempt");
            tokio::time::sleep_until(original + Duration::from_secs(1)).await;
            panic!("the original deadline must cancel this delayed refresh");
        }
        self.inner.read_control_execution(command, deadline).await
    }
    async fn release_control_hold(
        &self,
        fence: Arc<ChannelControlExecutionFence>,
        command: &AdmittedAgentCommand,
    ) -> Result<(), ChannelControlError> {
        self.inner.release_control_hold(fence, command).await
    }
    async fn expire_control_hold(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), ChannelControlError> {
        self.inner.expire_control_hold(command).await
    }
    async fn refuse_control_hold(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
    ) -> Result<(), ChannelControlError> {
        self.inner.refuse_control_hold(command, deadline).await
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_refresh_and_ordered_writer_never_renew_attempt_deadline() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "deadline-control").await;
    let delayed = Arc::new(DelayedRefresh {
        inner: Arc::new(crate::store::bridge_invokes::channel_control_execution::MongoChannelControlExecutionStore::new(c.invokes.clone())),
        first_deadline: Mutex::new(None),
    });
    let service = ChannelControlExecutionService::new(delayed, c.f.invokes.clone());
    let authority = service.authorize(&held.command).await.unwrap();
    let original = authority.fence().deadline();
    assert!(matches!(
        authority.refreshed().await,
        Err(ChannelControlError::Unavailable)
    ));
    assert!(Instant::now() >= original);
    assert_eq!(authority.fence().deadline(), original);

    // A fresh worker attempt obtains its own authority, but its ordered writer
    // must retain that attempt's deadline after the durable dispatch binding.
    let hook: worker::Hook = Box::new(|| {
        async {
            tokio::time::sleep(Duration::from_secs(16)).await;
        }
        .boxed()
    });
    let (execution, writes) = worker::execution(&c, Some(hook), false);
    worker::run(&c, &held, execution).await;
    let command =
        c.f.resources
            .get_command(&held.sidecar.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        command.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::PreWriteFailed
    );
    assert!(writes.lock().unwrap().is_empty());
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_ambiguous_conversation_commit_recovers_one_dispatch() {
    let mut c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "ambiguous-commit-control").await;
    let app_name = scope_execution_commits(&mut c).await;
    let admin = c.f.client.database("admin");
    let hook_admin = admin.clone();
    let entered = Arc::new(AtomicI64::new(-1));
    let hook_entered = entered.clone();
    let hook: worker::Hook = Box::new(move || {
        async move {
        let configured = hook_admin.run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":5_i32},
            "data":{"failCommands":["commitTransaction"],"appName":app_name,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous control conversation commit"}}
        }).await.unwrap();
        hook_entered.store(failpoint_count(&configured), Ordering::SeqCst);
    }.boxed()
    });
    // Dispatch resolve zero follows the hold-release and readiness lookups,
    // immediately before the conversation mutation; no earlier commit is hit.
    let (execution, writes) = worker::execution_at(&c, Some(hook), 0, false);
    let result = AgentCommandWorker::new(
        c.f.resources.clone(),
        execution.clone(),
        "ambiguous-control-worker".into(),
    )
    .unwrap()
    .run_one(&held.sidecar.command_id)
    .await;
    let stopped = admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(
        failpoint_count(&stopped) - entered.load(Ordering::SeqCst),
        5,
        "all five ambiguous commit responses must have been injected"
    );
    worker::run(&c, &held, execution).await;
    let command =
        c.f.resources
            .get_command(&held.sidecar.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        command.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::WriteConfirmed
    );
    assert_eq!(writes.lock().unwrap().len(), 1);
    let (reopened, new_writes) = worker::execution(&c, None, false);
    worker::run(&c, &held, reopened).await;
    assert!(new_writes.lock().unwrap().is_empty());
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}

pub(super) async fn scope_execution_commits(c: &mut ControlFixture) -> String {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!("control-execution-{}", uuid::Uuid::new_v4().simple());
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let resources = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &c.f.database)
            .await
            .unwrap(),
    );
    let bridges = Arc::new(
        MongoBridgeStore::open(&client, &c.f.database, "fixture-deployment", &custody_key())
            .await
            .unwrap(),
    );
    c.invokes = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(bridges, resources.clone(), &c.f.database)
            .await
            .unwrap(),
    );
    // Only the execution's persistence clients receive the scoped failpoint.
    // The live source fixture and administrative cleanup retain their clients.
    c.f.resources = resources;
    app_name
}

pub(super) fn failpoint_count(document: &Document) -> i64 {
    document
        .get_i64("count")
        .or_else(|_| document.get_i32("count").map(i64::from))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_expired_held_control_is_permanently_refused() {
    let c = ControlFixture::new().await;
    let mut held = worker::admit(&c, false, "expired-control").await;
    // Model passage beyond the retained five-minute queue window while keeping
    // all persisted timestamp and intent digest invariants consistent.
    let admitted = std::time::SystemTime::now() - Duration::from_secs(301);
    held.sidecar.intent.captured_at = admitted
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    held.sidecar.intent_sha256 = held.sidecar.intent.fingerprint().unwrap();
    held.command.receipt.admitted_at = layer2_core::rfc3339_at(admitted);
    held.command.queue_deadline = layer2_core::rfc3339_at(admitted + Duration::from_secs(300));
    held.validate().unwrap();
    let mut row = crate::store::agent_commands::command_document(&held.command).unwrap();
    row.insert(
        "channel_control_admission",
        record_bson(&held.sidecar).unwrap(),
    );
    let database = c.f.client.database(&c.f.database);
    database
        .collection::<Document>("agent_commands")
        .replace_one(doc! {"_id":held.sidecar.command_id.as_str()}, row)
        .await
        .unwrap();
    database.collection::<Document>("bridge_channel_control_intents").update_one(doc! {"_id":&held.sidecar.intent.id}, doc! {"$set":{"record":record_bson(&held.sidecar.intent).unwrap(),"intent_sha256":&held.sidecar.intent_sha256}}).await.unwrap();
    worker::service(&c)
        .release_held(&held.command, &worker::operations(&c))
        .await
        .unwrap();
    let row = database
        .collection::<Document>("agent_commands")
        .find_one(doc! {"_id":held.sidecar.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let refused = AdmittedChannelControl {
        command: crate::store::agent_commands::read_command(&row).unwrap(),
        sidecar: mongodb::bson::deserialize_from_bson(
            row.get("channel_control_admission").unwrap().clone(),
        )
        .unwrap(),
    };
    refused.validate().unwrap();
    assert_eq!(refused.sidecar.execution_hold, ChannelControlHold::Refused);
    assert_eq!(refused.command.status, AgentResponseStatus::Failed);
    assert_eq!(
        refused.command.work.failure.as_ref().unwrap().code,
        CommandFailureCode::QueueTimeout
    );
    assert!(worker::service(&c)
        .authorize(&refused.command)
        .await
        .is_err());
    let (execution, writes) = worker::execution(&c, None, false);
    worker::run(&c, &refused, execution).await;
    assert!(writes.lock().unwrap().is_empty());
    c.f.close().await;
}
