//! Real transaction aborts must reach Agent admission's fresh-snapshot retry.
use super::*;
use crate::store::bindings::admission_tests::{sandbox, workspace};
use crate::store::bindings::mongo_tests::{binding, NOW};
use layer2_core::profiles::{content_digest, AgentProfileSnapshot};
use layer2_core::sandbox::binding::{BindingAdmissionFence, BindingState, BindingWork};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentProfileId, RuntimeCapabilities, RuntimeCapability};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use mongodb::Client;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

async fn ready_admission(store: &MongoSandboxStore) -> (ProviderBinding, AgentAdmission) {
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    value.state = BindingState::Ready;
    value.work = BindingWork::Idle;
    value.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), value.clone()).await.unwrap();
    let mut parent = sandbox(&value);
    parent.status = SandboxStatus::Active;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(parent).await.unwrap();
    let mut child = workspace(&value);
    child.status = WorkspaceStatus::Ready;
    child.resolved_commit = Some("a".repeat(40));
    child.ready_at = Some(NOW.into());
    child.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    store.create_workspace(child.clone()).await.unwrap();
    let conversation = BindingConversation::new(&value, "contended".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(value.revision, None, conversation.clone())
        .await
        .unwrap();
    let content = value.profile.agent.clone();
    let input = AgentAdmission {
        binding: Some(BindingAdmissionFence::conversation(&value, &conversation)),
        created_by: None,
        owner: value.owner.clone(),
        workspace_id: child.id,
        agent_id: conversation.agent_id.clone(),
        worktree_id: conversation.worktree_id.clone(),
        profile: AgentProfileSnapshot {
            id: AgentProfileId::generate(),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: conversation.creation_key().unwrap(),
        event_at: NOW.into(),
    };
    (value, input)
}

async fn contended(stale_after_release: bool, persistent: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_binding_admission_{}", uuid::Uuid::new_v4().simple());
    let conflicts = Arc::new(Mutex::new((BTreeMap::new(), BTreeSet::new())));
    let observed = conflicts.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut state = observed.lock().unwrap();
        match event {
            CommandEvent::Started(event)
                if event.command.get_str("update").ok() == Some(BINDINGS)
                    && event.command.get_bool("autocommit").ok() == Some(false) =>
            {
                state.0.insert(
                    event.request_id,
                    format!(
                        "{:?}:{:?}",
                        event.command.get("lsid"),
                        event.command.get("txnNumber")
                    ),
                );
            }
            CommandEvent::Failed(event) => {
                if let Some(identity) = state.0.remove(&event.request_id) {
                    if crate::store::sandbox_mongo::is_transient(&event.failure) {
                        state.1.insert(identity);
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                state.0.remove(&event.request_id);
            }
            _ => {}
        }
    }));
    let contender_client = Client::with_options(options).unwrap();
    let holder_client = Client::with_uri_str(&uri).await.unwrap();
    crate::store::migrations::prepare_bindings_for_test(&contender_client.database(&name)).await;
    let store = MongoSandboxStore::open(&contender_client, &name)
        .await
        .unwrap();
    let other = MongoSandboxStore::open(&holder_client, &name)
        .await
        .unwrap();
    let (mut binding, input) = ready_admission(&store).await;
    let mut holder = holder_client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut holder)
        .await
        .unwrap();
    // Hold a real binding mutation in a second transaction. A changed revision
    // remains uncommitted until after the contender has retried fresh snapshots.
    binding.advance_revision(NOW.into()).unwrap();
    let mut replacement = binding_document(&binding).unwrap();
    replacement.remove("_id");
    other
        .bindings
        .records
        .update_one(
            doc! {"_id": &binding.binding_id},
            doc! {"$set": replacement},
        )
        .session(&mut holder)
        .await
        .unwrap();
    let worker = store.clone();
    let request = input.clone();
    let mut task = tokio::spawn(async move { worker.admit_agent(request).await });
    let started = tokio::time::Instant::now();
    if !persistent {
        // Baseline exits on its first real abort. Corrected code must retry;
        // release only after that observation or the bounded observation wait.
        let _ = tokio::time::timeout(Duration::from_secs(4), async {
            while conflicts.lock().unwrap().1.len() < 3 && !task.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        if stale_after_release {
            holder.commit_transaction().await.unwrap();
        } else {
            holder.abort_transaction().await.unwrap();
        }
    }
    let result = tokio::time::timeout(Duration::from_secs(9), &mut task).await;
    if persistent {
        holder.abort_transaction().await.unwrap();
    }
    if result.is_err() {
        task.abort();
        let _ = task.await;
    }
    let elapsed = started.elapsed();
    let attempts = conflicts.lock().unwrap().1.len();
    let agents = store.agents.count_documents(doc! {}).await.unwrap();
    let worktrees = store.worktrees.count_documents(doc! {}).await.unwrap();
    let result = match result {
        Ok(result) => result.unwrap(),
        Err(error) => {
            holder_client.database(&name).drop().await.unwrap();
            panic!("bounded admission task: {error}");
        }
    };
    let replay = if result.is_ok() || persistent {
        let reopened = MongoSandboxStore::open(&holder_client, &name)
            .await
            .unwrap();
        let first = reopened.admit_agent(input.clone()).await;
        let second = reopened.admit_agent(input.clone()).await;
        Some((first, second))
    } else {
        None
    };
    let retained = store.binding(&binding.binding_id).await.unwrap().unwrap();
    // Drain/drop the disposable fixture before any intended RED assertion.
    holder_client.database(&name).drop().await.unwrap();
    assert!(
        attempts >= 3,
        "binding admission discarded a known-aborted driver error after {attempts} attempt(s)"
    );
    if stale_after_release {
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert_eq!((agents, worktrees), (0, 0));
        assert_eq!(retained, binding);
    } else {
        if persistent {
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
            assert_eq!((agents, worktrees), (0, 0));
            assert!(elapsed >= Duration::from_secs(4) && elapsed < Duration::from_secs(9));
        } else {
            let created = result.expect("known-aborted binding fence must retry to creation");
            assert_eq!(created.creation().agent.id, input.agent_id);
            assert_eq!((agents, worktrees), (1, 1));
        }
        let (first, second) = replay.unwrap();
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(first.creation().agent.id, input.agent_id);
        assert_eq!(first.creation().worktree.id, input.worktree_id);
        assert_eq!(first.creation().worktree, second.creation().worktree);
        assert_eq!(first.creation().agent.id, second.creation().agent.id);
        assert!(matches!(second, AgentAdmissionResult::Existing(_)));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_admission_retries_known_aborted_fence() {
    contended(false, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_admission_revalidates_stale_fence_after_contention() {
    contended(true, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_admission_contention_is_bounded_and_same_key_recovers() {
    contended(false, true).await;
}
