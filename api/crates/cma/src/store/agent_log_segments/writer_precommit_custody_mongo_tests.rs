//! Close local authority only after actual keyed publication reaches precommit.
use super::*;
use crate::store::agent_log_segments::publication;
use crate::store::sandbox_mongo::precommit_probe;
use futures_util::FutureExt;
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

const FIELD: &str = "keyed_publication_attempt_v1";
type Trace = Arc<Mutex<Vec<CommandEvent>>>;

fn now() -> String {
    layer2_core::now_rfc3339()
}
fn deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
}

async fn authority(f: &Fixture) -> Sandbox {
    let mut parent = lifecycle_caller::parent(f).await;
    parent
        .work
        .schedule(SandboxWorkStep::RunLifecycleOperation, now());
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
        .claim_sandbox_work(&parent.id, "precommit-worker", now(), deadline())
        .await
        .unwrap()
        .unwrap()
}

async fn observed(f: &mut Fixture) -> (Client, String, Trace) {
    let admin = f.client.clone();
    let app = format!("m4-precommit-{}", uuid::Uuid::new_v4().simple());
    let trace: Trace = Arc::default();
    let seen = trace.clone();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        seen.lock().unwrap().push(event);
    }));
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap()
        .with_renewable_work(true);
    (admin, app, trace)
}

fn succeeded_command(events: &[CommandEvent], request_id: i32) -> bool {
    events.iter().any(|event| {
        matches!(event,
        CommandEvent::Succeeded(reply) if reply.request_id == request_id)
    })
}

fn update(command: &Document) -> Option<(&Document, &Document)> {
    if command.get_str("update").ok() != Some("agents") {
        return None;
    }
    let row = command.get_array("updates").ok()?.first()?.as_document()?;
    Some((row.get_document("q").ok()?, row.get_document("u").ok()?))
}

fn acquired_marker(events: &[CommandEvent]) -> Document {
    events
        .iter()
        .find_map(|event| {
            let CommandEvent::Started(event) = event else {
                return None;
            };
            if event.command.contains_key("autocommit")
                || !succeeded_command(events, event.request_id)
            {
                return None;
            }
            let (_, change) = update(&event.command)?;
            if event
                .command
                .get_document("writeConcern")
                .ok()?
                .get_str("w")
                .ok()
                != Some("majority")
            {
                return None;
            }
            change
                .get_document("$set")
                .ok()?
                .get_document(FIELD)
                .ok()
                .cloned()
        })
        .expect("the exact majority durable claim must exist before publication")
}

fn close_at_guard(
    trace: Trace,
    authority: WorkAuthority,
    calls: Arc<AtomicUsize>,
) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        assert_eq!(
            calls.fetch_add(1, Ordering::SeqCst),
            0,
            "guard must not be retried"
        );
        let events = trace.lock().unwrap();
        let marker = acquired_marker(&events);
        assert!(
            events.iter().any(|event| {
                let CommandEvent::Started(event) = event else {
                    return false;
                };
                let Some((query, change)) = update(&event.command) else {
                    return false;
                };
                event.command.get_bool("autocommit").ok() == Some(false)
                    && query.get_document(FIELD).ok() == Some(&marker)
                    && change
                        .get_document("$unset")
                        .is_ok_and(|fields| fields.contains_key(FIELD))
                    && succeeded_command(&events, event.request_id)
            }),
            "publication must have consumed this exact durable marker transactionally"
        );
        assert!(
            events.iter().any(|event| matches!(event,
            CommandEvent::Started(start)
                if start.command.get_str("insert").ok() == Some(PUBLICATIONS)
                    && succeeded_command(&events, start.request_id))),
            "actual publication receipt insertion must finish before the guard"
        );
        drop(events);
        // Closing a polled clone uses the real shared CloseOnDrop validity flag.
        // No clock advance can let the outer biased timeout cancel cleanup first.
        // The enclosing run has already completed its actual Mongo heartbeat.
        let pending = authority.run(
            std::future::pending::<Result<(), SandboxStoreError>>(),
            || std::future::pending::<Result<Option<WorkLeaseGrant>, SandboxStoreError>>(),
        );
        assert!(pending.now_or_never().is_none());
        assert!(
            WorkAuthority::current().is_err(),
            "guard must see closed authority"
        );
    })
}

async fn run(unacknowledged: bool) {
    let mut f = fixture(2).await;
    f.store = f.store.clone().with_renewable_work(true);
    let claimed = authority(&f).await;
    let independent = f.store.clone();
    let (admin, app, trace) = observed(&mut f).await;
    if unacknowledged {
        // Execute the abort but obscure its acknowledgment. Even if the server
        // happened to abort, the caller must not infer that from missing receipts.
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1},
                "data":{"appName":&app,"failCommands":["abortTransaction"],
                    "writeConcernError":{"code":64_i32,"codeName":"WriteConcernFailed",
                        "errmsg":"precommit abort acknowledgment unavailable"}},
            })
            .await
            .unwrap();
    }
    trace.lock().unwrap().clear();
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let operation = AgentLogPublicationIdentity {
        key: "precommit-custody".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    };
    let old = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    );
    let guard_calls = Arc::new(AtomicUsize::new(0));
    let heartbeats = AtomicUsize::new(0);
    let hook = close_at_guard(trace.clone(), old.clone(), guard_calls.clone());
    let refused = old
        .run(
            async {
                precommit_probe::BEFORE_GUARD
                    .scope(
                        hook,
                        crate::store::work_publication::scope(
                            &f.store,
                            f.store
                                .append_agent_log_keyed(&f.agent.id, &operation, kind.clone()),
                        ),
                    )
                    .await
            },
            || async {
                let grant = independent
                    .renew_sandbox_authority(&claimed, &now(), &deadline())
                    .await?;
                if grant.is_some() {
                    heartbeats.fetch_add(1, Ordering::SeqCst);
                }
                Ok(grant)
            },
        )
        .await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"off",
        })
        .await
        .unwrap();
    let events = trace.lock().unwrap().clone();
    let marker = acquired_marker(&events);
    let row = independent
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let receipts = collection(&f.store, PUBLICATIONS)
        .count_documents(doc! {})
        .await
        .unwrap();
    let logs = independent
        .agent_log
        .count_documents(doc! {})
        .await
        .unwrap();
    assert!(
        heartbeats.load(Ordering::SeqCst) >= 1,
        "initial heartbeat must be real and successful"
    );
    assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
    assert!(
        !events.iter().any(|event| matches!(event,
        CommandEvent::Started(event) if event.command_name == "commitTransaction")),
        "authority refusal must happen before any commit send"
    );
    assert_eq!(receipts, 0);
    assert_eq!(logs, 2);
    assert_eq!(row.get_i64("last_seq").unwrap(), 2);
    let publication = events
        .iter()
        .find_map(|event| match event {
            CommandEvent::Started(event)
                if event.command.get_str("insert").ok() == Some(PUBLICATIONS) =>
            {
                Some(&event.command)
            }
            _ => None,
        })
        .unwrap();
    for event in &events {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "abortTransaction" {
                assert_eq!(event.command.get("lsid"), publication.get("lsid"));
                assert_eq!(event.command.get("txnNumber"), publication.get("txnNumber"));
            }
        }
    }
    let released = events
        .iter()
        .filter_map(|event| {
            let CommandEvent::Started(event) = event else {
                return None;
            };
            if event.command.contains_key("autocommit") {
                return None;
            }
            let (query, change) = update(&event.command)?;
            change
                .get_document("$unset")
                .ok()?
                .contains_key(FIELD)
                .then_some((event, query))
        })
        .collect::<Vec<_>>();
    if unacknowledged {
        assert!(matches!(refused, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(row.get_document(FIELD).unwrap(), &marker);
        assert!(
            released.is_empty(),
            "uncertain abort must never release custody"
        );
        assert!(
            events.iter().any(|event| matches!(event,
            CommandEvent::Succeeded(event) if event.command_name == "abortTransaction"
                && event.reply.contains_key("writeConcernError"))),
            "abort uncertainty must be exercised"
        );
        assert!(f
            .store
            .append_agent_log_keyed(&f.agent.id, &operation, kind)
            .await
            .is_err());
        assert_eq!(
            collection(&f.store, PUBLICATIONS)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    } else {
        // A canceled cleanup is not evidence of the retained-marker bug.
        // Establish the real abort acknowledgment before the expected RED.
        assert!(
            events.iter().any(|event| matches!(event,
                CommandEvent::Succeeded(event) if event.command_name == "abortTransaction"
                    && !event.reply.contains_key("writeConcernError"))),
            "the reached precommit guard must finish an acknowledged abort"
        );
        assert!(
            !row.contains_key(FIELD),
            "acknowledged precommit refusal stranded its exact marker"
        );
        assert_eq!(refused, Err(SandboxStoreError::Conflict));
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].1.get_document(FIELD).unwrap(), &marker);
        assert!(succeeded_command(&events, released[0].0.request_id));
        let release_position = events
            .iter()
            .position(|event| {
                matches!(event,
            CommandEvent::Started(event) if event.request_id == released[0].0.request_id)
            })
            .unwrap();
        assert!(
            events[..release_position]
                .iter()
                .any(|event| matches!(event,
            CommandEvent::Succeeded(event) if event.command_name == "abortTransaction"
                && !event.reply.contains_key("writeConcernError"))),
            "the same transaction abort must be acknowledged before exact release"
        );
        let mut parent = independent.get_sandbox(&claimed.id).await.unwrap().unwrap();
        let revision = parent.revision;
        parent.work.lease = None;
        parent.revision += 1;
        independent.save_sandbox(revision, parent).await.unwrap();
        let successor = independent
            .claim_sandbox_work(&claimed.id, "successor", now(), deadline())
            .await
            .unwrap()
            .unwrap();
        let appended = WorkAuthority::new(
            ClaimedWork::Sandbox(Box::new(successor.clone())),
            Instant::now(),
        )
        .run(
            crate::store::work_publication::scope(
                &f.store,
                f.store
                    .append_agent_log_keyed(&f.agent.id, &operation, kind.clone()),
            ),
            || async {
                independent
                    .renew_sandbox_authority(&successor, &now(), &deadline())
                    .await
            },
        )
        .await
        .unwrap();
        assert_eq!(appended.seq, 3);
        assert_eq!(
            f.store
                .append_agent_log_keyed(&f.agent.id, &operation, kind)
                .await
                .unwrap(),
            appended
        );
        assert_eq!(
            collection(&f.store, PUBLICATIONS)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            independent
                .agent_log
                .count_documents(doc! {})
                .await
                .unwrap(),
            3
        );
        assert!(!independent
            .agents
            .find_one(doc! {"_id":f.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .contains_key(FIELD));
    }
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn closed_precommit_authority_releases_exact_aborted_claim_for_one_successor() {
    run(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unacknowledged_precommit_abort_retains_exact_claim_without_commit() {
    run(true).await;
}
