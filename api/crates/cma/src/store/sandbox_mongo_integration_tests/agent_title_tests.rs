use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentLogStore, AgentStore, PageRequest};
use layer2_core::sandbox::{
    AgentLogEntry, AgentLogKind, AgentTitle, AgentTitleClaimMode, AgentTitleClaimRequest,
    AgentTitleSource, AgentTitleStore, AgentTitleStoreError, SetAgentTitleOutcome,
};

use super::*;
use crate::store::agent_log::log_document;
use crate::store::agent_log_integrity::NORMAL_AGENT_LOG_ENTRIES;

fn claim_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000)
}

fn completed_turn(owner: &str, assistant: &str, sequence: u64) -> Vec<AgentLogKind> {
    vec![
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: sequence,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: owner.into(),
        },
        AgentLogKind::AgentMessage {
            content: assistant.into(),
        },
        AgentLogKind::TurnCompleted {
            turn_sequence: sequence,
        },
    ]
}

async fn create_title_agent(store: &MongoSandboxStore, id: &str) -> AgentId {
    let agent = agent_for_worker(id, NOW);
    let id = agent.id.clone();
    store.create_agent(agent).await.unwrap();
    id
}

async fn append_kinds(store: &MongoSandboxStore, id: &AgentId, kinds: Vec<AgentLogKind>) {
    for kind in kinds {
        store.append_agent_log(id, NOW.into(), kind).await.unwrap();
    }
}

async fn create_eligible_agent(store: &MongoSandboxStore, id: &str) -> AgentId {
    let id = create_title_agent(store, id).await;
    append_kinds(
        store,
        &id,
        completed_turn("First owner request", "First assistant answer", 7),
    )
    .await;
    id
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_candidates_page_raw_agents_and_read_the_exact_first_turn() {
    let (client, name) = database("agent_titles_candidates").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let a = create_title_agent(&store, "agt_title_page_a").await;
    let b = create_eligible_agent(&store, "agt_title_page_b").await;
    let c = create_title_agent(&store, "agt_title_page_c").await;
    append_kinds(
        &store,
        &b,
        completed_turn("Later owner", "Later assistant", 8),
    )
    .await;

    let first = store
        .agent_title_candidates(&PageRequest {
            limit: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.next.as_deref(), Some(a.as_str()));
    let second = store
        .agent_title_candidates(&PageRequest {
            limit: 1,
            cursor: first.next,
        })
        .await
        .unwrap();
    assert_eq!(second.items, vec![b.clone()]);
    assert_eq!(second.next.as_deref(), Some(b.as_str()));
    let third = store
        .agent_title_candidates(&PageRequest {
            limit: 1,
            cursor: second.next,
        })
        .await
        .unwrap();
    assert!(third.items.is_empty());
    assert_eq!(third.next, None);
    assert_eq!(c.as_str(), "agt_title_page_c");

    let transcript = store.agent_title_transcript(&b).await.unwrap().unwrap();
    assert_eq!(transcript.turn_sequence(), 7);
    assert_eq!(transcript.owner_message(), "First owner request");
    assert_eq!(transcript.assistant_message(), "First assistant answer");
    assert_eq!(store.agent_title(&b).await.unwrap(), None);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_claim_takeover_and_completion_are_fenced_once() {
    let (client, name) = database("agent_titles_claims").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let id = create_eligible_agent(&store, "agt_title_claims").await;
    let mut tasks = Vec::new();
    for index in 0..16 {
        let store = store.clone();
        let id = id.clone();
        tasks.push(tokio::spawn(async move {
            store
                .acquire_agent_title_claim(
                    &AgentTitleClaimRequest::new(id, format!("holder-{index}")),
                    claim_time(),
                )
                .await
                .unwrap()
        }));
    }
    let mut acquired = Vec::new();
    for task in tasks {
        if let Some(claim) = task.await.unwrap() {
            acquired.push(claim);
        }
    }
    assert_eq!(acquired.len(), 1);
    let stale = acquired.pop().unwrap();
    assert_eq!(stale.mode(), AgentTitleClaimMode::Generate);

    let successor = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "takeover"),
            stale.next_attempt_at(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.mode(), AgentTitleClaimMode::Generate);
    let generated = AgentTitle::new("Stale generated title").unwrap();
    assert!(!store
        .complete_agent_title(
            &stale,
            &generated,
            AgentTitleSource::Generated,
            MINUTE.into(),
        )
        .await
        .unwrap());
    let fallback = AgentTitle::new("First owner request").unwrap();
    assert!(store
        .complete_agent_title(
            &successor,
            &fallback,
            AgentTitleSource::Fallback,
            MINUTE.into(),
        )
        .await
        .unwrap());
    assert!(store
        .complete_agent_title(
            &successor,
            &generated,
            AgentTitleSource::Generated,
            MINUTE.into(),
        )
        .await
        .unwrap());

    let stored = store.agent_title(&id).await.unwrap().unwrap();
    assert_eq!(stored.title, fallback);
    assert_eq!(stored.source, AgentTitleSource::Fallback);
    let agent = store.get_agent(&id).await.unwrap().unwrap();
    assert_eq!(agent.title.as_deref(), Some("First owner request"));
    assert_eq!(agent.revision, 2);
    assert_eq!(agent.conversation.last_seq, 5);
    let events = store.agent_log(&id, 0, 200).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TitleSet { .. }))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn user_titles_are_write_once_and_atomically_fence_automatic_work() {
    let (client, name) = database("agent_titles_user").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let id = create_eligible_agent(&store, "agt_title_user_first").await;
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "generator"),
            claim_time(),
        )
        .await
        .unwrap()
        .unwrap();
    let user = AgentTitle::new("Chosen by user").unwrap();
    assert_eq!(
        store
            .set_user_agent_title(&id, &user, MINUTE.into())
            .await
            .unwrap(),
        SetAgentTitleOutcome::Set
    );
    assert!(!store
        .complete_agent_title(
            &claim,
            &AgentTitle::new("Generated too late").unwrap(),
            AgentTitleSource::Generated,
            MINUTE.into(),
        )
        .await
        .unwrap());
    assert_eq!(
        store
            .set_user_agent_title(&id, &user, MINUTE.into())
            .await
            .unwrap(),
        SetAgentTitleOutcome::AlreadySet
    );

    let raced = create_title_agent(&store, "agt_title_user_race").await;
    let mut tasks = Vec::new();
    for index in 0..12 {
        let store = store.clone();
        let id = raced.clone();
        tasks.push(tokio::spawn(async move {
            let title = AgentTitle::new(format!("User title {index}")).unwrap();
            let outcome = store
                .set_user_agent_title(&id, &title, MINUTE.into())
                .await
                .unwrap();
            (outcome, title)
        }));
    }
    let mut winner = None;
    for task in tasks {
        let (outcome, title) = task.await.unwrap();
        if outcome == SetAgentTitleOutcome::Set {
            assert!(winner.replace(title).is_none());
        }
    }
    assert_eq!(
        store.agent_title(&raced).await.unwrap().unwrap().title,
        winner.unwrap()
    );
    let events = store.agent_log(&raced, 0, 200).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, AgentLogKind::TitleSet { .. }));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn corrupt_transcript_and_full_log_capacity_fail_without_partial_writes() {
    let (client, name) = database("agent_titles_integrity").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let corrupt_id = create_eligible_agent(&store, "agt_title_gap").await;
    store
        .agent_log
        .delete_one(doc! { "agent_id": corrupt_id.as_str(), "seq": 2_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_title_transcript(&corrupt_id).await,
        Err(AgentTitleStoreError::SequenceConflict)
    );
    assert_eq!(
        store
            .acquire_agent_title_claim(
                &AgentTitleClaimRequest::new(corrupt_id.clone(), "holder"),
                claim_time(),
            )
            .await,
        Err(AgentTitleStoreError::Corrupt)
    );
    assert!(store
        .agents
        .find_one(doc! { "_id": corrupt_id.as_str(), "title_attempted": false })
        .await
        .unwrap()
        .is_some());

    let full_id = create_title_agent(&store, "agt_title_capacity").await;
    let logs = (1..=NORMAL_AGENT_LOG_ENTRIES)
        .map(|seq| AgentLogEntry {
            agent_id: full_id.clone(),
            seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::Deleted,
        })
        .map(|entry| log_document(&entry).unwrap())
        .collect::<Vec<_>>();
    store.agent_log.insert_many(logs).await.unwrap();
    let mut agent = store.get_agent(&full_id).await.unwrap().unwrap();
    agent.conversation.last_seq = NORMAL_AGENT_LOG_ENTRIES;
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert("last_seq", i64::try_from(NORMAL_AGENT_LOG_ENTRIES).unwrap());
    store
        .agents
        .update_one(doc! { "_id": full_id.as_str() }, doc! { "$set": fields })
        .await
        .unwrap();
    assert_eq!(
        store
            .set_user_agent_title(
                &full_id,
                &AgentTitle::new("No capacity").unwrap(),
                MINUTE.into(),
            )
            .await,
        Err(AgentTitleStoreError::AgentLogCapacityExceeded)
    );
    assert_eq!(store.agent_title(&full_id).await.unwrap(), None);
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": full_id.as_str() })
            .await
            .unwrap(),
        NORMAL_AGENT_LOG_ENTRIES
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn title_claim_and_completion_reconcile_ambiguous_majority_commits() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-agent-title-{suffix}");
    let name = format!("cma_811_agent_title_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let id = create_eligible_agent(&store, "agt_title_ambiguous").await;

    commands.lock().unwrap().clear();
    enable_ambiguous_commits(&admin, &app_name).await;
    let claim_result = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "holder"),
            claim_time(),
        )
        .await;
    disable_failpoint(&admin).await;
    let claim = claim_result.unwrap().unwrap();
    assert_ambiguous_transaction(&commands.lock().unwrap());

    commands.lock().unwrap().clear();
    enable_ambiguous_commits(&admin, &app_name).await;
    let completion = store
        .complete_agent_title(
            &claim,
            &AgentTitle::new("Ambiguous durable title").unwrap(),
            AgentTitleSource::Generated,
            MINUTE.into(),
        )
        .await;
    disable_failpoint(&admin).await;
    assert!(completion.unwrap());
    assert_ambiguous_transaction(&commands.lock().unwrap());
    assert_eq!(
        store.agent_title(&id).await.unwrap().unwrap().title,
        AgentTitle::new("Ambiguous durable title").unwrap()
    );
    assert_eq!(
        store
            .agent_log(&id, 0, 200)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TitleSet { .. }))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

async fn enable_ambiguous_commits(admin: &Client, app_name: &str) {
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous title commit",
                },
            },
        })
        .await
        .unwrap();
}

async fn disable_failpoint(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");
}

fn assert_ambiguous_transaction(commands: &[(String, Document)]) {
    let commit_positions = commands
        .iter()
        .enumerate()
        .filter_map(|(position, (name, _))| (name == "commitTransaction").then_some(position))
        .collect::<Vec<_>>();
    assert_eq!(commit_positions.len(), 5);
    assert!(commit_positions.iter().all(|position| {
        commands[*position]
            .1
            .get_document("writeConcern")
            .ok()
            .and_then(|concern| concern.get_str("w").ok())
            == Some("majority")
    }));
    let reconciliation = commands
        .iter()
        .skip(commit_positions.last().copied().unwrap() + 1)
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(
        reconciliation
            .iter()
            .map(|command| command.get_str("find").unwrap())
            .collect::<Vec<_>>(),
        vec!["agents", "agents", "agent_log", "agent_log", "agent_log"],
        "receipt includes the aggregate, integrity envelope, tail, extra-row probe and full log"
    );
    let at = reconciliation[0]
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    assert!(reconciliation.iter().all(|command| {
        command_uses_snapshot_read(command)
            && command.get("lsid") == reconciliation[0].get("lsid")
            && command
                .get_document("readConcern")
                .ok()
                .and_then(|concern| concern.get_timestamp("atClusterTime").ok())
                == Some(at)
    }));
}
