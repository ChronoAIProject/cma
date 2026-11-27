use super::*;
use crate::agent_manager::AgentManager;
use crate::sandbox::native_retirement_delivery::CaptureDelivery;
use crate::store::native_retirement_action::{RetirementPhase, RetirementRecords};
use layer2_core::agent_ownership::MaintenancePolicy;
use layer2_core::AgentId;
use mongodb::event::{command::CommandEvent, EventHandler};
use std::{io::Write, os::unix::process::ExitStatusExt, process::Stdio};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const PROCESS_DATABASE: &str = "CMA_TEST_RETIREMENT_CRASH_DATABASE";
const PROCESS_POINT: &str = "CMA_TEST_RETIREMENT_CRASH_POINT";
const PROCESS_SANDBOX: &str = "CMA_TEST_RETIREMENT_CRASH_SANDBOX";
const PROCESS_AGENT: &str = "CMA_TEST_RETIREMENT_CRASH_AGENT";
const TEST: &str = concat!(
    module_path!(),
    "::retirement_capture_survives_manager_process_loss"
);
const BOUND: Duration = Duration::from_secs(25);

#[derive(Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    BeforeBind,
    ReportReply,
    AfterAdmission,
}

impl CrashPoint {
    fn name(self) -> &'static str {
        match self {
            Self::BeforeBind => "before_bind",
            Self::ReportReply => "report_reply",
            Self::AfterAdmission => "after_admission",
        }
    }
}

async fn manager(
    client: &Client,
    name: &str,
    id: &str,
    policy: MaintenancePolicy,
) -> (Arc<MongoSandboxStore>, Arc<AgentManager>) {
    let store = Arc::new(
        MongoSandboxStore::open(client, name)
            .await
            .unwrap()
            .enable_agent_ownership(id.into())
            .await
            .unwrap(),
    );
    let manager = AgentManager::register_with_policy(
        store.clone(),
        Arc::new(LiveAgentRegistry::default()),
        "http://127.0.0.1:1".into(),
        4,
        false,
        policy,
    )
    .await
    .unwrap();
    manager
        .register_lifecycle(crate::agent_lifecycle::LifecycleDrain::fixture(1))
        .unwrap();
    manager.activate().await.unwrap();
    (store, manager)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn both_manager_drain_policies_join_capture_before_releasing_ownership() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let name = format!("retirement_drain_{}", Uuid::new_v4().simple());
        let (store, manager) = manager(&client, &name, "draining-owner", policy).await;
        let agent = AgentId("captured-agent".into());
        manager.coordinator.acquire(&agent).await.unwrap().unwrap();
        let delivery = CaptureDelivery::new(store, 1);
        manager
            .register_retirement_capture(delivery.tasks.clone())
            .unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(delivery.tasks.spawn(async move { wait.await.unwrap() }));
        let draining = tokio::spawn({
            let manager = manager.clone();
            async move { manager.drain().await }
        });
        timeout(Duration::from_secs(3), async {
            while !delivery.tasks.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!manager.is_ready());
        assert!(!delivery.tasks.spawn(async {}));
        assert!(manager
            .coordinator
            .store
            .owner(&agent)
            .await
            .unwrap()
            .is_some());
        assert!(!draining.is_finished());
        release.send(()).unwrap();
        timeout(Duration::from_secs(5), draining)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(manager
            .coordinator
            .store
            .owner(&agent)
            .await
            .unwrap()
            .is_none());
        client.database(&name).drop().await.unwrap();
    }
}

fn crash_boundary() -> ! {
    std::io::stdout().write_all(b"!").unwrap();
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

async fn record(
    store: &MongoSandboxStore,
    owner: &AgentManager,
    parent: &Sandbox,
    child: &Agent,
    point: Option<CrashPoint>,
) {
    let offer = store.retirement_capture_offer(parent, child).await.unwrap();
    let authority = owner.coordinator.acquire(&child.id).await.unwrap().unwrap();
    let claim = timeout(
        BOUND,
        authority.run(async {
            loop {
                let now = SystemTime::now();
                if let Some(claim) = store
                    .claim_agent_work(
                        &child.id,
                        &authority.token.instance_id,
                        rfc3339_at(now),
                        rfc3339_at(now + BOUND),
                    )
                    .await
                    .unwrap()
                {
                    break claim;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    if point == Some(CrashPoint::BeforeBind) {
        crash_boundary();
    }
    let bound = authority
        .run(store.bind_retirement_capture(&claim, offer, Uuid::new_v4()))
        .await
        .unwrap()
        .unwrap();
    let capture = receive_for(parent, &bound, authority, "404 Not Found").await;
    if point == Some(CrashPoint::ReportReply) {
        let name = std::env::var(PROCESS_DATABASE).unwrap();
        Client::with_uri_str(local_uri()).await.unwrap().database("admin").run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["update"],"appName":name,
                "writeConcernError":{"code":64,"errmsg":"synthetic retirement reply uncertainty"}},
        }).await.unwrap();
    }
    store
        .complete_retirement_capture(&bound, capture.sealed().unwrap())
        .await
        .unwrap();
}

async fn retirement_crash_owner(name: String) {
    assert!(name
        .strip_prefix("retirement_crash_")
        .is_some_and(|id| Uuid::parse_str(id).is_ok()));
    let point = std::env::var(PROCESS_POINT).unwrap();
    let uri = local_uri();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    options.retry_writes = Some(false);
    options.command_event_handler = Some(EventHandler::callback(|event| {
        if let CommandEvent::Succeeded(event) = event {
            if event.command_name == "update" && event.reply.contains_key("writeConcernError") {
                crash_boundary();
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let (store, owner) = manager(&client, &name, "crash-owner", MaintenancePolicy::Legacy).await;
    let sandbox_id = SandboxId(std::env::var(PROCESS_SANDBOX).unwrap());
    let agent_id = AgentId(std::env::var(PROCESS_AGENT).unwrap());
    let parent = store.get_sandbox(&sandbox_id).await.unwrap().unwrap();
    let child = store.get_agent(&agent_id).await.unwrap().unwrap();
    match point.as_str() {
        "before_bind" => {
            record(
                &store,
                &owner,
                &parent,
                &child,
                Some(CrashPoint::BeforeBind),
            )
            .await
        }
        "report_reply" => {
            record(
                &store,
                &owner,
                &parent,
                &child,
                Some(CrashPoint::ReportReply),
            )
            .await
        }
        "after_admission" => {
            use crate::store::native_retirement_action::{
                capture::RetirementCaptureConsumer, RetirementAction,
            };
            record(&store, &owner, &parent, &child, None).await;
            let child = store.get_agent(&child.id).await.unwrap().unwrap();
            let parent = timeout(BOUND, async {
                loop {
                    let now = SystemTime::now();
                    if let Some(parent) = store
                        .claim_sandbox_work(
                            &sandbox_id,
                            "crash-owner",
                            rfc3339_at(now),
                            rfc3339_at(now + Duration::from_secs(3)),
                        )
                        .await
                        .unwrap()
                    {
                        break parent;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap();
            let participants = [child];
            let captured = store
                .observed(&parent, &participants)
                .await
                .unwrap()
                .unwrap();
            let (observation, fences) = captured.observation();
            let action =
                RetirementAction::new(&parent, observation, &participants, fences).unwrap();
            RetirementCaptureConsumer::admit(store.as_ref(), &parent, &action, &captured)
                .await
                .unwrap();
            crash_boundary();
        }
        _ => panic!("unknown retirement crash checkpoint"),
    }
    panic!("owner continued past the crash checkpoint");
}

fn local_uri() -> String {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let parsed = url::Url::parse(&uri).unwrap();
    assert_eq!(parsed.scheme(), "mongodb");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.username().is_empty() && parsed.password().is_none());
    assert!(parsed
        .query_pairs()
        .any(|(key, value)| key == "directConnection" && value == "true"));
    uri
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable local replica set"]
async fn retirement_capture_survives_manager_process_loss() {
    if let Ok(name) = std::env::var(PROCESS_DATABASE) {
        return retirement_crash_owner(name).await;
    }
    let uri = local_uri();
    let client = Client::with_uri_str(&uri).await.unwrap();
    for point in [
        CrashPoint::BeforeBind,
        CrashPoint::ReportReply,
        CrashPoint::AfterAdmission,
    ] {
        let name = format!("retirement_crash_{}", Uuid::new_v4().simple());
        let result = tokio::spawn(exercise(point, client.clone(), name.clone(), uri.clone())).await;
        client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        client.database(&name).drop().await.unwrap();
        result.unwrap();
    }
}

async fn exercise(point: CrashPoint, client: Client, name: String, uri: String) {
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (fixture, mut parent, mut children, _) = active_siblings().await;
    let mut child = children.remove(0);
    child.requested_stop_reason = Some(layer2_core::sandbox::AgentStopReason::SandboxRetired);
    let now = SystemTime::now();
    child
        .work
        .schedule(AgentWorkStep::CloseProcess, rfc3339_at(now));
    composed::seed(&fixture, &store, &mut parent, &mut child, now).await;
    let delivery = CaptureDelivery::new(store.clone(), 1);
    let worker =
        composed::parent_worker(&fixture, store.clone(), "initial-parent", Some(&delivery));
    assert!(worker.run_one(&parent.id, SystemTime::now()).await.unwrap());
    let parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let child = store.get_agent(&child.id).await.unwrap().unwrap();
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            TEST.strip_prefix("cma::").unwrap_or(TEST),
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env(PROCESS_DATABASE, &name)
        .env(PROCESS_POINT, point.name())
        .env(PROCESS_SANDBOX, parent.id.as_str())
        .env(PROCESS_AGENT, child.id.as_str())
        .env("CHRONO_TEST_MONGODB_FAILPOINT_URI", uri)
        .env("RUST_MIN_STACK", "33554432")
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert_ne!(owner.id().unwrap(), std::process::id());
    let mut output = owner.stdout.take().unwrap();
    timeout(BOUND, async {
        loop {
            if output.read_u8().await.unwrap() == b'!' {
                break;
            }
        }
    })
    .await
    .expect("owner process must reach the selected crash boundary");
    owner.start_kill().unwrap();
    let status = timeout(BOUND, owner.wait()).await.unwrap().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    let (store, survivor) = manager(&client, &name, "survivor", MaintenancePolicy::Legacy).await;
    assert!(survivor.is_ready());

    let rows = client.database(&name).collection::<Document>("sandboxes");
    let row = rows
        .find_one(doc! {"_id":parent.id.as_str()})
        .read_concern(ReadConcern::majority())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        phase(&row),
        if point == CrashPoint::BeforeBind {
            "offered"
        } else {
            "observed"
        }
    );
    let action = store.action(&parent.id).await.unwrap();
    assert_eq!(
        action.as_ref().map(|value| &value.phase),
        (point == CrashPoint::AfterAdmission).then_some(&RetirementPhase::Admitted)
    );
    let receiver = Receiver::new(&fixture, &parent).await;
    fixture
        .backend
        .retirement_incarnation
        .store(1, Ordering::SeqCst);
    let successor = composed::parent_worker(&fixture, store.clone(), "survivor", None);
    composed::run_due(&successor, &parent.id).await;
    assert_eq!(store.action(&parent.id).await.unwrap(), action);
    let pending = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(pending.status, SandboxStatus::Retiring);
    assert_eq!(pending.runtime, parent.runtime);
    assert!(!pending.lifecycle.home.unwrap().released);
    assert!(!fixture
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 0);
    println!(
        "retirement-process-loss checkpoint={} pending=true provider_retirements=0",
        point.name()
    );

    if point != CrashPoint::BeforeBind {
        fixture
            .backend
            .retirement_incarnation
            .store(0, Ordering::SeqCst);
        let admission = (point == CrashPoint::ReportReply).then_some(&delivery);
        let successor = composed::parent_worker(&fixture, store.clone(), "survivor", admission);
        for _ in 0..10 {
            if store.get_sandbox(&parent.id).await.unwrap().unwrap().status
                == SandboxStatus::Retired
            {
                break;
            }
            composed::run_due(&successor, &parent.id).await;
        }
        let retired = store.get_sandbox(&parent.id).await.unwrap().unwrap();
        assert_eq!(retired.status, SandboxStatus::Retired);
        assert!(retired.runtime.is_none());
        assert!(retired.lifecycle.home.unwrap().released);
        assert!(store.action(&parent.id).await.unwrap().is_none());
        assert_eq!(
            fixture
                .backend
                .calls
                .snapshot()
                .iter()
                .filter(|call| call.as_str() == "retire-alternative")
                .count(),
            1
        );
        println!(
            "retirement-process-loss checkpoint={} retired=true provider_retirements=1",
            point.name()
        );
    }
    receiver.server.abort();
}
