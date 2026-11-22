//! Close local authority only after actual keyed publication reaches precommit.
use super::*;
use crate::store::agent_log_segments::publication;
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

#[path = "writer_precommit_custody_mongo_tests/support.rs"]
mod support;
use support::*;
#[path = "writer_precommit_custody_mongo_tests/cleanup.rs"]
mod cleanup;

#[derive(Clone, Copy)]
enum AbortFault {
    None,
    WriteConcern,
    Network,
    Labelled,
    Command,
    CleanupFailure,
    CleanupUncertain,
    Replacement,
}

async fn run(fault: AbortFault) {
    let unacknowledged = matches!(
        fault,
        AbortFault::WriteConcern | AbortFault::Network | AbortFault::Labelled | AbortFault::Command
    );
    let cleanup_fault = matches!(
        fault,
        AbortFault::CleanupFailure | AbortFault::CleanupUncertain | AbortFault::Replacement
    );
    let mut f = fixture(2).await;
    f.store = f.store.clone().with_renewable_work(true);
    let claimed = authority(&f).await;
    let independent = f.store.clone();
    let (admin, app, trace, observer_hook, abort_hook) = observed(&mut f).await;
    if unacknowledged {
        let mut data = doc! {"appName":&app,"failCommands":["abortTransaction"]};
        match fault {
            AbortFault::None
            | AbortFault::CleanupFailure
            | AbortFault::CleanupUncertain
            | AbortFault::Replacement => unreachable!(),
            AbortFault::WriteConcern => {
                data.insert(
                    "writeConcernError",
                    doc! {"code":64_i32,
                    "codeName":"WriteConcernFailed", "errmsg":"abort acknowledgment unavailable"},
                );
            }
            AbortFault::Network => {
                data.insert("closeConnection", true);
            }
            AbortFault::Labelled => {
                data.insert("errorCode", 112_i32);
                data.insert("errorLabels", vec!["TransientTransactionError"]);
            }
            AbortFault::Command => {
                data.insert("errorCode", 1_i32);
            }
        }
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1}, "data":data,
            })
            .await
            .unwrap();
    }
    if cleanup_fault {
        *abort_hook.lock().unwrap() = Some(cleanup::after_abort(
            app.clone(),
            independent.agents.namespace().db,
            f.agent.id.clone(),
            trace.clone(),
            fault,
        ));
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
    *observer_hook.lock().unwrap() = Some(hook);
    let refused = old
        .run(
            async {
                // Keep the real authority guard while isolating precommit
                // custody from the separate start-snapshot ordering boundary.
                f.store
                    .append_agent_log_keyed(&f.agent.id, &operation, kind.clone())
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
    eprintln!("precommit_custody: guard_calls={} receipts={} logs={} marker_present={} abort_acknowledged={} abort_uncertain={} unavailable={}",
        guard_calls.load(Ordering::SeqCst), receipts, logs, row.contains_key(FIELD),
        events.iter().filter(|event| matches!(event, CommandEvent::Succeeded(reply)
            if reply.command_name == "abortTransaction" && !reply.reply.contains_key("writeConcernError"))).count(),
        events.iter().filter(|event| matches!(event, CommandEvent::Succeeded(reply)
            if reply.command_name == "abortTransaction" && reply.reply.contains_key("writeConcernError"))).count(),
        matches!(&refused, Err(SandboxStoreError::Unavailable(_))));
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
                Some(event)
            }
            _ => None,
        })
        .unwrap();
    for event in &events {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "abortTransaction" {
                assert_eq!(event.command.get("lsid"), publication.command.get("lsid"));
                assert_eq!(
                    event.command.get("txnNumber"),
                    publication.command.get("txnNumber")
                );
                assert_eq!(event.connection.address, publication.connection.address);
                assert_eq!(event.service_id, publication.service_id);
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
            let (query, change) = support::update(&event.command)?;
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
                && event.reply.contains_key("writeConcernError"))
                || matches!(event,
                CommandEvent::Failed(event) if event.command_name == "abortTransaction")),
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
    } else if cleanup_fault {
        assert!(
            refused.is_err(),
            "failed cleanup never authorizes another attempt"
        );
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].1.get_document(FIELD).unwrap(), &marker);
        match fault {
            AbortFault::Replacement => {
                let mut replacement = marker.clone();
                replacement.insert("token", "replacement-marker-token");
                assert_eq!(row.get_document(FIELD).unwrap(), &replacement);
                assert_eq!(refused, Err(SandboxStoreError::Conflict));
            }
            AbortFault::CleanupFailure => {
                assert_eq!(row.get_document(FIELD).unwrap(), &marker);
                assert!(matches!(refused, Err(SandboxStoreError::Unavailable(_))));
            }
            AbortFault::CleanupUncertain => {
                // An uncertain write may have applied or left the exact marker.
                // Neither state permits this invocation to replay publication.
                if let Ok(retained) = row.get_document(FIELD) {
                    assert_eq!(retained, &marker);
                }
                assert!(matches!(refused, Err(SandboxStoreError::Unavailable(_))));
                assert!(events.iter().any(|event| matches!(event,
                    CommandEvent::Succeeded(reply) if reply.command_name == "update"
                        && reply.reply.contains_key("writeConcernError"))));
            }
            _ => unreachable!(),
        }
    } else {
        // Establish actual abort acknowledgment before asserting exact cleanup.
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
    run(AbortFault::None).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unacknowledged_precommit_abort_retains_exact_claim_without_commit() {
    run(AbortFault::WriteConcern).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn network_lost_abort_reply_retains_custody() {
    run(AbortFault::Network).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn labelled_abort_reply_retains_custody() {
    run(AbortFault::Labelled).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn command_error_abort_reply_retains_custody() {
    run(AbortFault::Command).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn failed_exact_cleanup_retains_original_marker() {
    run(AbortFault::CleanupFailure).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn uncertain_cleanup_never_authorizes_replay() {
    run(AbortFault::CleanupUncertain).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn exact_cleanup_never_deletes_replacement_marker() {
    run(AbortFault::Replacement).await;
}
