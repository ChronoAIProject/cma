//! Score terminal command recovery through the real Mongo transaction root.
use super::*;
use crate::store::agent_log_segments::{publication, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, CommitOutcome};
use futures_util::{FutureExt, TryStreamExt};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;
use std::{
    panic::AssertUnwindSafe,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
enum Fault {
    BodyAbort,
    CommittedUnknown,
    UncommittedUnknown,
    Disconnect,
}

impl Fault {
    fn data(self, application: &str) -> Document {
        let mut data = doc! {"appName":application};
        match self {
            Self::BodyAbort => {
                data.insert("failCommands", vec!["update"]);
                data.insert("errorCode", 112_i32);
                data.insert("errorLabels", vec!["TransientTransactionError"]);
            }
            Self::CommittedUnknown => {
                data.insert("failCommands", vec!["commitTransaction"]);
                data.insert("writeConcernError", doc! {"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected uncertain settlement acknowledgement"});
            }
            Self::UncommittedUnknown => {
                data.insert("failCommands", vec!["commitTransaction"]);
                data.insert("errorCode", 50_i32);
                data.insert("errorLabels", vec!["UnknownTransactionCommitResult"]);
            }
            Self::Disconnect => {
                data.insert("failCommands", vec!["commitTransaction"]);
                data.insert("closeConnection", true);
            }
        }
        data
    }
}

async fn adopted(store: &MongoSandboxStore, round: usize) -> Agent {
    let agent =
        super::super::retained_assessment::ready_source(store, &format!("tc040_{round}")).await;
    let database = store.client.database(&store.agents.namespace().db);
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &database,
        gate::GateName::Segments,
        round as i64,
        gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let identity = AgentLogPublicationIdentity {
        key: "settlement-campaign-adoption".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    };
    store
        .append_agent_log_keyed(&agent.id, &identity, kind)
        .await
        .unwrap();
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

async fn finish(store: &MongoSandboxStore, agent: &Agent, chunks: usize) {
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();
    for offset in 0..chunks {
        recorder
            .record_update(
                &agent.id,
                1,
                &mut assembly,
                chunk(&format!("output {offset} ")),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let outcome = recorder
        .record_update(
            &agent.id,
            1,
            &mut assembly,
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
}

async fn command_row(
    store: &MongoSandboxStore,
    id: &layer2_core::ids::AgentCommandId,
) -> AdmittedAgentCommand {
    let row = store
        .agent_commands
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    crate::store::agent_commands::read_command(&row).unwrap()
}

#[derive(Debug, PartialEq)]
struct DurableEvidence {
    agent: Agent,
    parent: Sandbox,
    logs: Vec<AgentLogEntry>,
    events: Vec<SandboxEvent>,
    publications: Vec<Document>,
}

async fn evidence(store: &MongoSandboxStore, current: &Agent) -> DurableEvidence {
    let agent = store.get_agent(&current.id).await.unwrap().unwrap();
    let parent = store
        .get_sandbox(&current.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let logs = store.agent_log(&current.id, 0, 200).await.unwrap();
    let events = store
        .sandbox_events(&current.sandbox_id, 0, 200)
        .await
        .unwrap();
    let publications = store
        .client
        .database(&store.agents.namespace().db)
        .collection::<Document>(segments::PUBLICATIONS)
        .find(doc! {"agent_id":current.id.as_str()})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    DurableEvidence {
        agent,
        parent,
        logs,
        events,
        publications,
    }
}

fn failpoint_count(result: &Document) -> i64 {
    result
        .get_i64("count")
        .or_else(|_| result.get_i32("count").map(i64::from))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn tc040_terminal_settlement_reconciles_one_thousand_uncertain_transactions() {
    let (admin, name) = database("tc040_settlement").await;
    let db = admin.database(&name);
    let result = AssertUnwindSafe(campaign(&admin, &name))
        .catch_unwind()
        .await;
    // The failpoint and database belong to this disposable fixture.
    uncertain::clear(&admin).await;
    let cleanup = db.drop().await;
    match result {
        Ok(()) => cleanup.unwrap(),
        Err(original) => {
            if let Err(error) = cleanup {
                eprintln!("fixture cleanup after failure: {error}");
            }
            std::panic::resume_unwind(original);
        }
    }
}

async fn campaign(admin: &Client, name: &str) {
    segments::schema::migrate(&admin.database(name))
        .await
        .unwrap();
    let store = MongoSandboxStore::open(admin, name).await.unwrap();
    let (client, app, original_session) = uncertain::observed_client_for("tc040").await;
    let failing = MongoSandboxStore::open(&client, name).await.unwrap();
    let mut measurements = Vec::new();
    for fault in [
        Fault::BodyAbort,
        Fault::CommittedUnknown,
        Fault::UncommittedUnknown,
        Fault::Disconnect,
    ] {
        for failures in 1..=5_i32 {
            for recovery in 0..5 {
                for chunks in 0..10 {
                    let round = measurements.len();
                    let (agent, admitted) =
                        begin_working(&store, adopted(&store, round).await).await;
                    let id = &admitted.receipt.envelope.command_id;
                    finish(&store, &agent, chunks).await;
                    let started = Instant::now();
                    let original = command_row(&store, id).await;
                    let before = evidence(&store, &agent).await;
                    assert_eq!(before.agent.status, AgentStatus::Ready);
                    assert_eq!(
                        before
                            .logs
                            .iter()
                            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
                            .count(),
                        1
                    );
                    assert_eq!(
                        before
                            .logs
                            .iter()
                            .filter(|entry| matches!(
                                entry.kind,
                                AgentLogKind::TurnCompleted { .. }
                            ))
                            .count(),
                        1
                    );
                    *original_session.lock().unwrap() = None;
                    let configured = admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":failures},"data":fault.data(&app)}).await.unwrap();
                    let result = failing.reconcile_command(id).await;
                    let cleared = admin
                        .database("admin")
                        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
                        .await
                        .unwrap();
                    assert!(
                        failpoint_count(&cleared) > failpoint_count(&configured),
                        "round {round} did not reach {fault:?}"
                    );
                    if result.is_err() && original_session.lock().unwrap().is_some() {
                        uncertain::release_abandoned_transaction(admin, &original_session).await;
                    }
                    let unresolved = command_row(&store, id).await;
                    assert_eq!(unresolved.receipt, original.receipt);
                    assert!(
                        unresolved == original
                            || unresolved.status == AgentResponseStatus::Completed
                    );
                    assert_eq!(
                        evidence(&store, &agent).await,
                        before,
                        "round {round}: settlement changed durable source"
                    );

                    let (first, second) = match recovery {
                        0 => (
                            store.reconcile_command(id).await,
                            failing.reconcile_command(id).await,
                        ),
                        1 => (
                            failing.reconcile_command(id).await,
                            store.reconcile_command(id).await,
                        ),
                        2 => {
                            tokio::join!(store.reconcile_command(id), failing.reconcile_command(id))
                        }
                        3 => {
                            tokio::join!(failing.reconcile_command(id), store.reconcile_command(id))
                        }
                        _ => {
                            // A fresh store has no process-local command or transaction state.
                            let reopened = MongoSandboxStore::open(admin, name).await.unwrap();
                            (
                                reopened.reconcile_command(id).await,
                                store.reconcile_command(id).await,
                            )
                        }
                    };
                    let settled =
                        first.unwrap_or_else(|error| panic!("round {round} {fault:?}: {error:?}"));
                    assert_eq!(second.unwrap(), settled);
                    if let Ok(first) = result {
                        assert_eq!(first, settled);
                    }
                    assert_eq!(settled.status, AgentResponseStatus::Completed);
                    assert_eq!(settled.receipt, admitted.receipt);
                    assert!(settled.work.lease.is_none());
                    assert_eq!(
                        settled.work.dispatch.as_ref().unwrap().claim,
                        original.work.dispatch.as_ref().unwrap().claim
                    );
                    assert_eq!(
                        settled.work.dispatch.as_ref().unwrap().operation_sequence,
                        original.work.dispatch.as_ref().unwrap().operation_sequence
                    );
                    assert_eq!(command_row(&store, id).await, settled);
                    assert_eq!(
                        store
                            .agent_commands
                            .count_documents(doc! {"agent_id":agent.id.as_str()})
                            .await
                            .unwrap(),
                        1
                    );
                    assert_eq!(evidence(&store, &agent).await, before);
                    let elapsed = started.elapsed();
                    measurements.push(elapsed);
                    assert!(
                        elapsed <= Duration::from_secs(30),
                        "round {round} {fault:?} terminal lag {elapsed:?}"
                    );
                }
            }
        }
        eprintln!("tc040 fault={fault:?} completed={}", measurements.len());
    }
    assert_eq!(measurements.len(), 1_000);
    measurements.sort_unstable();
    eprintln!(
        "tc040 interleavings=1000 terminal_lag_p99_ms={} max_ms={}",
        measurements[989].as_millis(),
        measurements[999].as_millis()
    );
    assert!(measurements[989] <= Duration::from_secs(5));
    client.shutdown().await;
}
