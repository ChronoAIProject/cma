//! Actual keyed and segmented-control roots retain original inputs after aborts.
use super::*;
use crate::store::agent_log_segments::publication;
use layer2_core::sandbox::{store::*, *};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::time::Instant;

#[path = "writer_start_takeover_test_support.rs"]
mod takeover;

type Trace = Arc<Mutex<Vec<Document>>>;

async fn authority(f: &Fixture) -> Sandbox {
    let mut parent = lifecycle_caller::parent(f).await;
    parent
        .work
        .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into());
    parent.revision += 1;
    f.store
        .save_sandbox(parent.revision - 1, parent.clone())
        .await
        .unwrap();
    f.store
        .create_worktree(Worktree {
            id: f.agent.worktree_id.clone(),
            workspace_id: f.agent.workspace_id.clone(),
            sandbox_id: f.agent.sandbox_id.clone(),
            owner: f.agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/receipt/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(f.agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
    f.store
        .claim_sandbox_work(&parent.id, "boundary-worker", now(), deadline())
        .await
        .unwrap()
        .unwrap()
}

fn now() -> String {
    layer2_core::now_rfc3339()
}
fn deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(300))
}

async fn observed(f: &mut Fixture) -> (Client, Trace, Arc<AtomicUsize>) {
    let admin = f.client.clone();
    let app = format!("m4-boundary-{}", uuid::Uuid::new_v4().simple());
    let trace = Arc::new(Mutex::new(Vec::new()));
    let seen = trace.clone();
    let injections = Arc::new(AtomicUsize::new(0));
    let injected = injections.clone();
    let database = f.store.agents.namespace().db;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        seen.lock().unwrap().push(event.command.clone());
        if event.db != database
            || event.command.get_str("update").ok() != Some("sandboxes")
            || event.command.get_bool("startTransaction").ok() != Some(true)
            || injected
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        // Claim acquisition is already durable. Hold the actual start fence
        // before send so the injected abort cannot hit that earlier claim.
        let uri = uri.clone();
        let app = app.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        let control = Client::with_uri_str(uri).await.unwrap();
                        fault(&control, &app, false).await;
                        control.shutdown().await;
                    })
                    .await
                    .expect("start-fence fault must acknowledge before the command is sent");
                });
        })
        .join()
        .unwrap();
    }));
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap()
        .with_renewable_work(true);
    (admin, trace, injections)
}

async fn fault(admin: &Client, app: &str, unknown: bool) {
    let mut labels = vec!["TransientTransactionError"];
    if unknown {
        labels.push("UnknownTransactionCommitResult");
    }
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1},
        "data":{"appName":app,"failCommands":["update"],"errorCode":112_i32,"errorLabels":labels},
    }).await.unwrap();
}
async fn clear(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}
fn identity(kind: &AgentLogKind) -> AgentLogPublicationIdentity {
    AgentLogPublicationIdentity {
        key: "fixture".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    }
}

async fn keyed_and_segmented_control_retry_aborted_start_with_original_authority_case(
    control: bool,
) {
    let mut f = fixture(2).await;
    f.store = f.store.clone().with_renewable_work(true);
    let claimed = authority(&f).await;
    let independent = f.store.clone();
    let (admin, trace, injections) = observed(&mut f).await;
    trace.lock().unwrap().clear();
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            crate::store::work_publication::scope(&f.store, async {
                if control {
                    f.store
                        .schedule_agent_stop(AgentStopRequest {
                            target: AgentControlTarget::from(&f.agent),
                            reason: AgentStopReason::Owner,
                            requested_at: NOW.into(),
                        })
                        .await
                        .map(|result| matches!(result, AgentControlResult::Scheduled(_)))
                        .map_err(|e| SandboxStoreError::Unavailable(format!("{e:?}")))
                } else {
                    let kind = AgentLogKind::ConversationGap { frames: 1 };
                    f.store
                        .append_agent_log_keyed(&f.agent.id, &identity(&kind), kind)
                        .await
                        .map(|entry| entry.seq == 3)
                        .map_err(|e| SandboxStoreError::Unavailable(format!("{e:?}")))
                }
            })
            .await
        },
        || async {
            independent
                .renew_sandbox_authority(&claimed, &now(), &deadline())
                .await
        },
    )
    .await;
    clear(&admin).await;
    let starts = trace
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c.get_bool("startTransaction"), Ok(true)))
        .cloned()
        .collect::<Vec<_>>();
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let publications = collection(&f.store, PUBLICATIONS)
        .count_documents(doc! {})
        .await
        .unwrap();
    f.destroy().await;
    assert_eq!(injections.load(Ordering::SeqCst), 1);
    assert_eq!(
        result,
        Ok(true),
        "aborted transaction-start fence must retry original writer"
    );
    assert_eq!(starts.len(), 2);
    assert!(
        starts[0].get("lsid") != starts[1].get("lsid")
            || starts[0].get("txnNumber") != starts[1].get("txnNumber")
    );
    assert_eq!(publications, 1);
    assert!(!row.contains_key("keyed_publication_attempt_v1"));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn keyed_uncertain_precommit_error_retains_original_claim_without_replay() {
    let mut f = fixture(2).await;
    let admin = f.client.clone();
    let app = format!("m4-keyed-unknown-{}", uuid::Uuid::new_v4().simple());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let db = f.store.agents.namespace().db;
    let injections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = injections.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            // Claim acquisition already completed. Arm the exact uncertainty
            // before sending claim consumption, with no detached callback race.
            if event.db == db
                && matches!(event.command.get_str("update"), Ok("agents"))
                && matches!(event.command.get_bool("startTransaction"), Ok(true))
            {
                let uri = uri.clone();
                let app = app.clone();
                std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async {
                            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                                let control = Client::with_uri_str(uri).await.unwrap();
                                fault(&control, &app, true).await;
                                control.shutdown().await;
                            })
                            .await
                            .expect("unknown injection must acknowledge before send");
                        });
                })
                .join()
                .unwrap();
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }));
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let operation = identity(&kind);
    let result = f
        .store
        .append_agent_log_keyed(&f.agent.id, &operation, kind.clone())
        .await;
    clear(&admin).await;
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let second = f
        .store
        .append_agent_log_keyed(&f.agent.id, &operation, kind)
        .await;
    clear(&admin).await;
    let count = f.store.agent_log.count_documents(doc! {}).await.unwrap();
    f.destroy().await;
    assert_eq!(injections.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(result.is_err());
    assert!(second.is_err());
    assert_eq!(count, 2);
    assert!(
        row.contains_key("keyed_publication_attempt_v1"),
        "uncertain precommit error released original claim"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn keyed_start_refuses_stale_parent_without_stranding_the_original_attempt() {
    let mut f = fixture(2).await;
    f.store = f.store.clone().with_renewable_work(true);
    let old = authority(&f).await;
    let old_agent_revision = f.agent.revision as i64;
    let takeover = takeover::install(&mut f, &old).await;
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let operation = identity(&kind);
    let refused = WorkAuthority::new(ClaimedWork::Sandbox(Box::new(old.clone())), Instant::now())
        .run(
            async {
                crate::store::work_publication::scope(
                    &f.store,
                    f.store
                        .append_agent_log_keyed(&f.agent.id, &operation, kind.clone()),
                )
                .await
            },
            || async {
                f.store
                    .renew_sandbox_authority(&old, &now(), &deadline())
                    .await
            },
        )
        .await;
    let (successor, acquired) = takeover
        .lock()
        .unwrap()
        .take()
        .expect("majority-acquisition barrier must observe the durable claim before takeover");
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let before_count = collection(&f.store, PUBLICATIONS)
        .count_documents(doc! {})
        .await
        .unwrap();
    let published = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(successor.clone())),
        Instant::now(),
    )
    .run(
        async {
            crate::store::work_publication::scope(
                &f.store,
                f.store
                    .append_agent_log_keyed(&f.agent.id, &operation, kind.clone()),
            )
            .await
        },
        || async {
            f.store
                .renew_sandbox_authority(&successor, &now(), &deadline())
                .await
        },
    )
    .await;
    let after = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let count = collection(&f.store, PUBLICATIONS)
        .count_documents(doc! {})
        .await
        .unwrap();
    f.destroy().await;
    assert_eq!(acquired.get_str("publication_key").unwrap(), operation.key);
    assert_eq!(acquired.get_i64("expected_seq").unwrap(), 2);
    assert_eq!(
        acquired.get_i64("expected_revision").unwrap(),
        old_agent_revision
    );
    assert!(
        !row.contains_key("keyed_publication_attempt_v1"),
        "known pre-publication refusal must release its exact attempt"
    );
    assert_eq!(refused, Err(SandboxStoreError::Conflict));
    assert_eq!(before_count, 0);
    assert_eq!(row.get_i64("last_seq").unwrap(), 2);
    assert_eq!(published.unwrap().seq, 3);
    assert_eq!(count, 1);
    assert!(!after.contains_key("keyed_publication_attempt_v1"));
}

#[path = "writer_boundary_retry_mongo_tests_cases.rs"]
mod cases;
