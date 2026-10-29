use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use layer2_core::sandbox::store::AgentLogStore;
use layer2_core::sandbox::{
    binding::BindingConversationState,
    store::{AgentStore, WorktreeStore},
    *,
};
use layer2_core::{Authorization, Capability, SubjectId};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "lifecycle_fixture.rs"]
mod lifecycle_fixture;
use lifecycle_fixture::Fixture;

const BINDING: &str = "binding-lifecycle";
const PATH: &str = "/api/v1/cmaeg/bindings/binding-lifecycle";
const CONVERSATION: &str = "conversation_lifecycle";

#[allow(clippy::too_many_arguments)]
async fn call(
    f: &Fixture,
    method: &str,
    path: &str,
    input: Option<Value>,
    owner: bool,
    generation: Option<u64>,
    operation: Option<&str>,
    status: StatusCode,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(generation) = generation {
        request = request.header("x-cma-binding-generation", generation.to_string());
    }
    if let Some(operation) = operation {
        request = request.header("idempotency-key", operation);
    }
    let mut request = request
        .body(input.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId::new(
            if owner { "owner" } else { "manager" },
        )));
    request.extensions_mut().insert(Authorization::new([
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
        Capability::DeleteOwnSandbox,
        Capability::UseOwnSandboxes,
    ]));
    let response = f.app.clone().oneshot(request).await.unwrap();
    let actual = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(
        actual,
        status,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

fn profile(key: &ProvidedCredential) -> Value {
    let profile = json!({"schema_version":1,"metadata":{"name":"lifecycle fixture","owner":{"kind":"human","subject":"owner"},"provider":"cma_codex"},
        "model":{"access":"managed","service":layer2_core::profiles::AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"low"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    json!({"schema_version":1,"profile_digest":format!("sha256:{}",content_digest(&profile).unwrap()),"profile":profile,"agent_key":key.expose(),"tools":{"native_mcp_url":"https://gateway.example/mcp","nyxid_service_slug":"cmaeg"}})
}

async fn ready(f: &Fixture) -> ProviderBinding {
    for _ in 0..40 {
        let binding = f.store.binding(BINDING).await.unwrap().unwrap();
        if binding.state == BindingState::Ready {
            return binding;
        }
        f.tick(&binding.sandbox_id, None).await;
    }
    panic!(
        "fixture binding did not become ready: {:?}",
        f.store.binding(BINDING).await.unwrap()
    );
}

async fn conversation(f: &Fixture, binding: &ProviderBinding) -> BindingConversation {
    call(
        f,
        "PUT",
        &format!("{PATH}/conversations/{CONVERSATION}"),
        Some(json!({})),
        false,
        Some(binding.generation),
        None,
        StatusCode::CREATED,
    )
    .await;
    for _ in 0..30 {
        let conversation = f
            .store
            .binding_conversation(BINDING, binding.generation, CONVERSATION)
            .await
            .unwrap()
            .unwrap();
        if conversation.state == BindingConversationState::Ready {
            return conversation;
        }
        f.tick(&binding.sandbox_id, Some(&conversation.agent_id))
            .await;
    }
    panic!("fixture conversation did not become ready");
}

async fn retire(
    f: &Fixture,
    binding: &ProviderBinding,
    conversation: &BindingConversation,
    key: &ProvidedCredential,
) -> Sandbox {
    call(
        f,
        "DELETE",
        &format!("/api/v1/sandboxes/{}", binding.sandbox_id),
        None,
        true,
        None,
        Some(&format!("retire-generation-{}", binding.generation)),
        StatusCode::ACCEPTED,
    )
    .await;
    // Retirement has begun through the public owner API, but no terminal proof exists.
    call(
        f,
        "POST",
        &format!("{PATH}/preparation-inspections"),
        Some(inspection_event(key, "while-retiring")),
        false,
        None,
        None,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        f.store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Retiring
    );
    for _ in 0..30 {
        f.tick(&binding.sandbox_id, Some(&conversation.agent_id))
            .await;
        let sandbox = f
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        if sandbox.status == SandboxStatus::Retired {
            assert!(sandbox.lifecycle.home.as_ref().unwrap().released);
            assert_eq!(
                f.store
                    .get_agent(&conversation.agent_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                AgentStatus::Stopped
            );
            assert_eq!(
                f.store
                    .get_worktree(&conversation.worktree_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                WorktreeStatus::Ready
            );
            assert_eq!(f.store.binding(BINDING).await.unwrap().unwrap(), *binding);
            assert_eq!(
                f.store
                    .binding_conversation(BINDING, binding.generation, CONVERSATION)
                    .await
                    .unwrap()
                    .unwrap(),
                *conversation
            );
            return sandbox;
        }
    }
    panic!("public owner retirement did not settle");
}

fn inspection_event(key: &ProvidedCredential, event: &str) -> Value {
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    json!({"schema_version":1,"admission_id":event,"target_id":"consumer-one","request_hash":"a".repeat(64),"owner":"owner","admitted_at":now,"expires_at":now+600,"agent_key":key.expose()})
}

async fn fresh_inspection(
    f: &Fixture,
    key: &ProvidedCredential,
    retired: &Sandbox,
    event: &str,
) -> Value {
    let retired_seconds =
        layer2_core::seconds_from_rfc3339(retired.retired_at.as_deref().unwrap()).unwrap();
    for _ in 0..30 {
        if layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap() > retired_seconds
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    call(
        f,
        "POST",
        &format!("{PATH}/preparation-inspections"),
        Some(inspection_event(key, event)),
        false,
        None,
        None,
        StatusCode::OK,
    )
    .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_public_owner_retirement_preserves_ready_history_and_replaces_once() {
    let f = Fixture::new().await;
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    call(
        &f,
        "PUT",
        PATH,
        Some(profile(&key)),
        false,
        None,
        None,
        StatusCode::OK,
    )
    .await;
    let original = ready(&f).await;
    let original_conversation = conversation(&f, &original).await;
    let original_sandbox_policy = f
        .store
        .get_sandbox(&original.sandbox_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .execution;
    let original_workspace_policy = f
        .store
        .get_workspace(&original.workspace_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .execution;
    assert!(original_sandbox_policy.is_some());
    assert!(original_workspace_policy.is_some());
    assert_eq!(
        f.store
            .get_agent(&original_conversation.agent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Ready
    );
    let retired = retire(&f, &original, &original_conversation, &key).await;
    assert_eq!(retired.work.execution, original_sandbox_policy);
    assert_eq!(
        f.store
            .get_workspace(&original.workspace_id)
            .await
            .unwrap()
            .unwrap()
            .work
            .execution,
        original_workspace_policy
    );
    let original_workspace = f.store.get_workspace(&original.workspace_id).await.unwrap();
    let original_agent = f
        .store
        .get_agent(&original_conversation.agent_id)
        .await
        .unwrap();
    let original_worktree = f
        .store
        .get_worktree(&original_conversation.worktree_id)
        .await
        .unwrap();
    let history = f
        .store
        .agent_log(&original_conversation.agent_id, 0, 100)
        .await
        .unwrap();
    assert!(!history.is_empty());
    let inspected = fresh_inspection(&f, &key, &retired, "fresh-after-owner-retirement").await;
    assert_eq!(inspected["state"], "pending");
    let intent: BindingPreparationIntent =
        serde_json::from_value(inspected["intent"].clone()).unwrap();
    let operation = intent.operation_id().unwrap();
    let reservation = json!({"schema_version":1,"intent":intent,"agent_key":key.expose()});
    let path = format!("{PATH}/preparations");
    reject_unsettled_children(&f, &original_conversation, &reservation, &operation).await;
    let receipt = call(
        &f,
        "POST",
        &path,
        Some(reservation.clone()),
        false,
        None,
        Some(&operation),
        StatusCode::OK,
    )
    .await;
    assert_eq!(receipt["generation"], 2);
    assert_eq!(
        call(
            &f,
            "POST",
            &path,
            Some(reservation),
            false,
            None,
            Some(&operation),
            StatusCode::OK
        )
        .await,
        receipt
    );
    call(
        &f,
        "PUT",
        &format!("{PATH}/conversations/{CONVERSATION}"),
        Some(json!({})),
        false,
        Some(1),
        None,
        StatusCode::CONFLICT,
    )
    .await;
    let replacement = ready(&f).await;
    assert_ne!(replacement.sandbox_id, original.sandbox_id);
    assert_ne!(replacement.workspace_id, original.workspace_id);
    let replacement_conversation = conversation(&f, &replacement).await;
    assert_ne!(
        replacement_conversation.agent_id,
        original_conversation.agent_id
    );
    assert_eq!(
        f.store.get_sandbox(&original.sandbox_id).await.unwrap(),
        Some(retired.clone())
    );
    assert_eq!(
        f.store.get_workspace(&original.workspace_id).await.unwrap(),
        original_workspace
    );
    assert_eq!(
        f.store
            .get_agent(&original_conversation.agent_id)
            .await
            .unwrap(),
        original_agent
    );
    assert_eq!(
        f.store
            .get_worktree(&original_conversation.worktree_id)
            .await
            .unwrap(),
        original_worktree
    );
    assert_eq!(
        f.store
            .agent_log(&original_conversation.agent_id, 0, 100)
            .await
            .unwrap(),
        history
    );
    assert_eq!(
        f.store
            .binding_conversation(BINDING, 1, CONVERSATION)
            .await
            .unwrap(),
        Some(original_conversation)
    );
    assert_eq!(
        f.store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let archived_first = f
        .store
        .bindings
        .preparations
        .find_one(doc! {"_id":&operation})
        .await
        .unwrap()
        .unwrap();
    // A later owner retirement must also work while the former generation's
    // Ready alias remains immutable. Its first receipt supplies archive proof.
    let retired = retire(&f, &replacement, &replacement_conversation, &key).await;
    let inspected = fresh_inspection(&f, &key, &retired, "fresh-after-second-retirement").await;
    let intent: BindingPreparationIntent =
        serde_json::from_value(inspected["intent"].clone()).unwrap();
    let operation = intent.operation_id().unwrap();
    f.store
        .bindings
        .preparations
        .delete_one(doc! {"_id":archived_first.get_str("_id").unwrap()})
        .await
        .unwrap();
    call(
        &f,
        "POST",
        &path,
        Some(json!({"schema_version":1,"intent":intent,"agent_key":key.expose()})),
        false,
        None,
        Some(&operation),
        StatusCode::CONFLICT,
    )
    .await;
    f.store
        .bindings
        .preparations
        .insert_one(archived_first)
        .await
        .unwrap();
    let receipt = call(
        &f,
        "POST",
        &path,
        Some(json!({"schema_version":1,"intent":intent,"agent_key":key.expose()})),
        false,
        None,
        Some(&operation),
        StatusCode::OK,
    )
    .await;
    assert_eq!(receipt["generation"], 3);
    assert_eq!(
        f.store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    f.client.database(&f.database).drop().await.unwrap();
}

async fn reject_unsettled_children(
    f: &Fixture,
    conversation: &BindingConversation,
    reservation: &Value,
    operation: &str,
) {
    let agent = f
        .store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let original = f
        .store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let worktree = f
        .store
        .get_worktree(&conversation.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let original_worktree = f
        .store
        .worktrees
        .find_one(doc! {"_id":worktree.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    for change in [
        "missing_agent",
        "live_agent",
        "active_turn",
        "stop_pending",
        "work_pending",
        "workspace",
        "missing_worktree",
        "removing_worktree",
        "foreign_worktree_agent",
        "foreign_unassigned_worktree_workspace",
    ] {
        let mut changed_agent = agent.clone();
        let mut changed_worktree = worktree.clone();
        match change {
            "live_agent" => changed_agent.status = AgentStatus::Ready,
            "active_turn" => changed_agent.conversation.active_turn = Some(9),
            "stop_pending" => changed_agent.requested_stop_reason = Some(AgentStopReason::Owner),
            "work_pending" => changed_agent.work.step = AgentWorkStep::CloseProcess,
            "workspace" => changed_agent.workspace_id = layer2_core::WorkspaceId::generate(),
            "removing_worktree" => changed_worktree.status = WorktreeStatus::Removing,
            "foreign_worktree_agent" => {
                changed_worktree.agent_id = Some(layer2_core::AgentId::generate())
            }
            "foreign_unassigned_worktree_workspace" => {
                changed_worktree.agent_id = None;
                changed_worktree.workspace_id = layer2_core::WorkspaceId::generate();
            }
            _ => {}
        }
        if change == "missing_agent" {
            f.store
                .agents
                .delete_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap();
        } else {
            f.store
                .agents
                .replace_one(
                    doc! {"_id":agent.id.as_str()},
                    crate::store::agents::agent_document(&changed_agent).unwrap(),
                )
                .await
                .unwrap();
        }
        if change == "missing_worktree" {
            f.store
                .worktrees
                .delete_one(doc! {"_id":worktree.id.as_str()})
                .await
                .unwrap();
        } else {
            f.store
                .worktrees
                .replace_one(
                    doc! {"_id":worktree.id.as_str()},
                    crate::store::worktrees::worktree_document(&changed_worktree).unwrap(),
                )
                .await
                .unwrap();
        }
        call(
            f,
            "POST",
            &format!("{PATH}/preparations"),
            Some(reservation.clone()),
            false,
            None,
            Some(operation),
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(
            f.store
                .bindings
                .preparations
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "accepted {change}"
        );
        f.store
            .agents
            .replace_one(doc! {"_id":agent.id.as_str()}, original.clone())
            .upsert(true)
            .await
            .unwrap();
        f.store
            .worktrees
            .replace_one(doc! {"_id":worktree.id.as_str()}, original_worktree.clone())
            .upsert(true)
            .await
            .unwrap();
    }
}
