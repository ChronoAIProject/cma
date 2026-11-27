use super::*;
use base64::Engine as _;
use fixtures::NOW;
use layer2_core::resource_access::{ResourceAccessSnapshot, ResourceLineage, ResourceOwner};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::SubjectId;

pub(in crate::store) fn key(byte: u8) -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([byte; 32]))
        .unwrap()
}

fn snapshot() -> AgentProvenanceSnapshot {
    let mut sandbox = fixtures::sandbox("access");
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "access-runtime".into(),
        runtime_version: "retained-image".into(),
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "access-home".into(),
            capacity_gib: 8,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "access-home-pvc".into(),
        storage_generation: "access-home-uid".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    let workspace = fixtures::workspace(&sandbox, "access");
    let creation = fixtures::admission(&workspace, "access")
        .materialize(&workspace)
        .unwrap();
    let provenance = AgentCreationProvenance::for_admission(
        &creation.agent,
        &sandbox,
        ResourceOwner::Person {
            subject: sandbox.owner.clone(),
        },
        None,
    )
    .unwrap();
    let mut agent = creation.agent;
    agent.work.step = AgentWorkStep::OpenConversation;
    agent.work.next_fence = 2;
    agent.work.lease = Some(AgentWorkLease {
        holder: "access-worker".into(),
        fence: 1,
        expires_at: layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        ),
    });
    AgentProvenanceSnapshot {
        resource: ResourceAccessSnapshot {
            lineage: ResourceLineage {
                sandbox,
                workspace: Some(workspace),
                worktree: Some(creation.worktree),
                agent: Some(agent),
            },
            policy: None,
            shares: vec![],
        },
        creation: Some(provenance),
        first_log: Some(creation.initial_log),
    }
}

pub(in crate::store) fn request(snapshot: &AgentProvenanceSnapshot) -> AgentOpenRequest {
    let agent = snapshot.resource.lineage.agent.as_ref().unwrap();
    AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(
            agent,
            &snapshot.resource.lineage.sandbox,
            snapshot.resource.lineage.worktree.as_ref().unwrap(),
        ),
        None,
    )
}

#[test]
fn access_capability_tracks_incarnation_not_worker_or_resume_request() {
    let state = snapshot();
    let runtime = RuntimeHandle("access-runtime".into());
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let signing = key(11).agent_access_signing_key("deployment-a");
    let initial = records::derive_capability(&signing, &scope).ok().unwrap();
    assert!(records::valid_capability(&initial));
    let mut takeover = state.clone();
    let agent = takeover.resource.lineage.agent.as_mut().unwrap();
    agent.work.next_fence += 1;
    agent.work.lease.as_mut().unwrap().fence += 1;
    agent.conversation.conversation_id = Some("original-native".into());
    agent.conversation.runtime_generation = Some(1);
    assert_eq!(validation::scope(&takeover, &runtime).ok().unwrap(), scope);
    assert!(validation::request(&takeover, &request(&takeover), 2).is_ok());
    assert!(validation::request(&takeover, &request(&takeover), 1).is_err());
    for altered in [
        key(12).agent_access_signing_key("deployment-a"),
        key(11).agent_access_signing_key("deployment-b"),
    ] {
        assert_ne!(
            records::derive_capability(&altered, &scope).ok().unwrap(),
            initial
        );
    }
    for case in 0..5 {
        let mut changed = scope.clone();
        match case {
            0 => changed.runtime.boot_id = Some("c".repeat(32)),
            1 => changed.runtime.provision_generation = Some(2),
            2 => changed.runtime.runtime_id = "replacement".into(),
            3 => changed.creation.agent_created_at = "2026-09-06T00:00:00Z".into(),
            _ => changed.creation.creation_fingerprint = "d".repeat(64),
        }
        assert_ne!(
            records::derive_capability(&signing, &changed).ok().unwrap(),
            initial
        );
    }
}

#[test]
fn access_authority_refuses_changed_lineage_provenance_and_open_fence() {
    let original = snapshot();
    let runtime = RuntimeHandle("access-runtime".into());
    let open = request(&original);
    assert!(validation::scope(&original, &runtime).is_ok());
    assert!(validation::request(&original, &open, 1).is_ok());
    assert!(validation::request(&original, &open, 0).is_err());
    assert!(validation::scope(&original, &RuntimeHandle("foreign".into())).is_err());
    for case in 0..11 {
        let mut changed = original.clone();
        match case {
            0 => changed.resource.lineage.agent.as_mut().unwrap().owner = SubjectId::new("foreign"),
            1 => changed.resource.lineage.sandbox.owner = SubjectId::new("foreign"),
            2 => {
                changed.resource.lineage.workspace.as_mut().unwrap().owner =
                    SubjectId::new("foreign")
            }
            3 => changed.creation.as_mut().unwrap().agent_created_at = "changed".into(),
            4 => {
                changed
                    .resource
                    .lineage
                    .agent
                    .as_mut()
                    .unwrap()
                    .requested_stop_reason = Some(AgentStopReason::Owner)
            }
            5 => changed.resource.lineage.agent.as_mut().unwrap().status = AgentStatus::Stopped,
            6 => changed.resource.lineage.sandbox.status = SandboxStatus::Paused,
            7 => {
                changed.resource.lineage.worktree.as_mut().unwrap().status =
                    WorktreeStatus::Removing
            }
            8 => {
                changed.resource.lineage.sandbox.credential.pending =
                    changed.resource.lineage.sandbox.credential.current.clone()
            }
            9 => changed.resource.lineage.agent.as_mut().unwrap().deleted_at = Some(NOW.into()),
            _ => {
                changed
                    .resource
                    .lineage
                    .sandbox
                    .credential
                    .current
                    .as_mut()
                    .unwrap()
                    .expires_at = "2000-01-01T00:00:00Z".into();
            }
        }
        assert!(
            validation::scope(&changed, &runtime).is_err(),
            "case {case}"
        );
    }
    let mut changed = original.clone();
    changed
        .resource
        .lineage
        .agent
        .as_mut()
        .unwrap()
        .work
        .lease
        .as_mut()
        .unwrap()
        .expires_at = NOW.into();
    assert!(validation::request(&changed, &open, 1).is_err());
    let mut open = open;
    open.options.home_directory.push_str("/foreign");
    open.request_sha256 = open.computed_sha256();
    assert!(validation::request(&original, &open, 1).is_err());
}

#[test]
fn access_rollout_enrolls_only_initial_open_and_capability_parser_is_exact() {
    let state = snapshot();
    let initial = state.resource.lineage.agent.unwrap();
    assert!(validation::new_agent(&initial));
    for case in 0..4 {
        let mut retained = initial.clone();
        match case {
            0 => retained.ready_at = Some(NOW.into()),
            1 => retained.conversation.conversation_id = Some("retained".into()),
            2 => retained.status = AgentStatus::Ready,
            _ => retained.work.step = AgentWorkStep::PublishConnection,
        }
        assert!(!validation::new_agent(&retained));
    }
    for malformed in [
        "".into(),
        "nyx_opaque".into(),
        format!("cma_at_{}", "A".repeat(64)),
        format!("cma_at_{}", "a".repeat(63)),
        format!("cma_at_{}\n", "a".repeat(64)),
    ] {
        assert!(!records::valid_capability(&malformed));
    }
}

#[test]
fn scope_revision_capabilities_require_managed_credentials_even_with_a_forged_expiry() {
    let managed = snapshot();
    let runtime = RuntimeHandle("access-runtime".into());
    assert!(validation::scope(&managed, &runtime).is_ok());
    for source in [
        SandboxCredentialSource::Provided {
            key_sha256: "a".repeat(64),
        },
        SandboxCredentialSource::Binding {
            binding_id: "bnd_custom_verification".into(),
            tool_endpoint: Some("https://gateway.example.test/mcp".into()),
        },
    ] {
        let mut supplied = managed.clone();
        supplied.resource.lineage.sandbox.credential.source = source;
        // A supplied source must never gain this capability even if corrupt
        // custody metadata copied a managed key's apparently valid expiry.
        assert!(validation::scope(&supplied, &runtime).is_err());
        supplied
            .resource
            .lineage
            .sandbox
            .credential
            .current
            .as_mut()
            .unwrap()
            .expires_at
            .clear();
        assert!(validation::scope(&supplied, &runtime).is_err());
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_tools_skip_issuer_owned_keys_without_enrolling_a_capability() {
    let mut state = snapshot();
    let sandbox = &mut state.resource.lineage.sandbox;
    sandbox.credential.source = SandboxCredentialSource::Provided {
        key_sha256: "a".repeat(64),
    };
    let current = sandbox.credential.current.as_mut().unwrap();
    current.provider_key_id = format!("provided-{}", "a".repeat(64));
    current.expires_at.clear();
    // Credential source is immutable after creation; persist its real initial
    // authority rather than rewriting an existing managed sandbox.
    let (client, database, store, state) = persist_snapshot(state).await;
    let source =
        MongoAgentAccessToolSource::new(store.clone(), "https://control.example.test", &key(9))
            .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let open = request(&state);
    assert!(source.resolve(&runtime, &open, 1).await.unwrap().is_none());
    assert_eq!(
        store
            .agent_access_tools
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

pub(in crate::store) async fn persisted() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentProvenanceSnapshot,
) {
    persist_snapshot(snapshot()).await
}

async fn persist_snapshot(
    state: AgentProvenanceSnapshot,
) -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentProvenanceSnapshot,
) {
    let client = mongodb::Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required"),
    )
    .await
    .unwrap();
    let database = format!("cma_access_tools_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    store
        .create_sandbox(state.resource.lineage.sandbox.clone())
        .await
        .unwrap();
    let workspace = state.resource.lineage.workspace.as_ref().unwrap();
    store.create_workspace(workspace.clone()).await.unwrap();
    store
        .admit_agent(fixtures::admission(workspace, "access"))
        .await
        .unwrap();
    let mut agent = state.resource.lineage.agent.clone().unwrap();
    agent.revision += 1;
    store.save_agent(1, agent).await.unwrap();
    let state = store
        .agent_provenance_snapshot(&state.creation.unwrap().agent_id)
        .await
        .unwrap();
    (client, database, store, state)
}

pub(in crate::store) async fn publish(store: &MongoSandboxStore, id: &AgentId) {
    let mut agent = store.get_agent(id).await.unwrap().unwrap();
    let mut tree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    tree.status = WorktreeStatus::Ready;
    tree.revision += 1;
    store.save_worktree(tree.revision - 1, tree).await.unwrap();
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(NOW.into());
    agent.work.step = AgentWorkStep::None;
    agent.work.lease = None;
    agent.conversation.conversation_id = Some("original-native".into());
    agent.conversation.runtime_generation = Some(1);
    agent.revision += 1;
    store.save_agent(agent.revision - 1, agent).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_discovery_does_not_contend_with_agent_publication() {
    let (client, database, store, state) = persisted().await;
    let source =
        MongoAgentAccessToolSource::new(store.clone(), "https://control.example.test", &key(9))
            .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = source.capability(&scope).ok().unwrap();
    let binding = source
        .resolve(&runtime, &request(&state), 1)
        .await
        .unwrap()
        .unwrap();
    source
        .confirm_open(&binding, "original-native")
        .await
        .unwrap();
    publish(&store, &scope.creation.agent_id).await;

    // Hold the same Agent write conflict that first-turn publication and
    // streaming cause. Read-only MCP discovery must still finish; a tool call
    // must continue to refuse an unconfirmed mutation fence.
    let mut publishing = client.start_session().await.unwrap();
    start_transaction(&mut publishing).await.unwrap();
    store
        .agents
        .update_one(
            doc! {"_id": scope.creation.agent_id.as_str()},
            doc! {"$inc": {"resource_share_guard": 1_i64}},
        )
        .session(&mut publishing)
        .await
        .unwrap();
    for _ in 0..3 {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            source.authenticate_setup(&bearer),
        )
        .await
        .expect("discovery must not wait on the Agent writer")
        .unwrap();
    }
    assert!(source.authenticate(&bearer).await.is_err());
    publishing.abort_transaction().await.unwrap();
    assert_eq!(
        source.authenticate(&bearer).await.unwrap().conversation,
        "original-native"
    );

    // A subsequent snapshot still rejects a revoked owner/runtime lineage.
    let mut agent = store
        .get_agent(&scope.creation.agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.requested_stop_reason = Some(AgentStopReason::Owner);
    agent.revision += 1;
    store.save_agent(agent.revision - 1, agent).await.unwrap();
    assert!(source.authenticate_setup(&bearer).await.is_err());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_tools_require_confirmed_open_and_preserve_worker_takeover() {
    let (client, database, store, state) = persisted().await;
    let source =
        MongoAgentAccessToolSource::new(store.clone(), "https://control.example.test", &key(9))
            .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let open = request(&state);
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = source.capability(&scope).ok().unwrap();
    assert!(source.authenticate(&bearer).await.is_err());
    assert!(source.authenticate_setup(&bearer).await.is_err());
    let binding = source.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    source.authenticate_setup(&bearer).await.unwrap();
    assert!(source.authenticate(&bearer).await.is_err());
    source
        .confirm_open(&binding, "original-native")
        .await
        .unwrap();
    source
        .confirm_open(&binding, "original-native")
        .await
        .unwrap();
    assert!(source
        .confirm_open(&binding, "another-native")
        .await
        .is_err());
    assert!(
        source.authenticate(&bearer).await.is_err(),
        "Agent must publish the same conversation"
    );
    let document = store
        .agent_access_tools
        .find_one(doc! {"_id": records::hash(&bearer)})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{document:?}").contains(&bearer));
    assert!(!format!("{binding:?}").contains(&bearer));
    publish(&store, &scope.creation.agent_id).await;
    let identity = source.authenticate(&bearer).await.unwrap();
    source.authenticate_setup(&bearer).await.unwrap();
    assert_eq!(identity.conversation, "original-native");
    assert_eq!(identity.creation, scope.creation);

    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let source =
        MongoAgentAccessToolSource::new(reopened.clone(), "https://control.example.test/", &key(9))
            .unwrap();
    let mut agent = reopened
        .get_agent(&identity.agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.work.step = AgentWorkStep::PublishConnection;
    agent.work.lease = state
        .resource
        .lineage
        .agent
        .as_ref()
        .unwrap()
        .work
        .lease
        .clone();
    agent.work.lease.as_mut().unwrap().fence = 2;
    agent.work.next_fence = 3;
    agent.revision += 1;
    reopened
        .save_agent(agent.revision - 1, agent)
        .await
        .unwrap();
    assert!(source.resolve(&runtime, &open, 1).await.is_err());
    assert!(source
        .confirm_open(&binding, "original-native")
        .await
        .is_err());
    let resumed = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    let expected = HostedAccessToolBinding::new(
        "https://control.example.test",
        bearer.clone().into(),
        runtime.clone(),
        &open,
        2,
        Some("original-native".into()),
    )
    .unwrap();
    assert_eq!(resumed, expected);
    source
        .confirm_open(&resumed, "original-native")
        .await
        .unwrap();
    assert_eq!(source.authenticate(&bearer).await.unwrap(), identity);
    let forged = HostedAccessToolBinding::new(
        "https://other.example.test",
        format!("cma_at_{}", "e".repeat(64)).into(),
        runtime.clone(),
        &open,
        2,
        Some("original-native".into()),
    )
    .unwrap();
    assert!(source
        .confirm_open(&forged, "original-native")
        .await
        .is_err());

    let mut parent = reopened
        .get_sandbox(&identity.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    parent.runtime.as_mut().unwrap().boot_id = Some("c".repeat(32));
    parent.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
    parent.runtime.as_mut().unwrap().provision_generation = Some(2);
    parent.lifecycle.runtime_generation_high_water = 2;
    parent.revision += 1;
    reopened
        .save_sandbox(parent.revision - 1, parent)
        .await
        .unwrap();
    assert!(source.authenticate(&bearer).await.is_err());
    assert!(source.authenticate_setup(&bearer).await.is_err());
    assert!(source
        .confirm_open(&resumed, "original-native")
        .await
        .is_err());
    let runtime = RuntimeHandle("replacement-runtime".into());
    let replacement = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    assert_ne!(replacement, resumed);
    source
        .confirm_open(&replacement, "original-native")
        .await
        .unwrap();
    assert!(source.authenticate(&bearer).await.is_err());

    reopened
        .agent_access_tools
        .delete_many(doc! {})
        .await
        .unwrap();
    assert!(
        source.resolve(&runtime, &open, 2).await.unwrap().is_none(),
        "retained unproven Agent is left attached without a tool"
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_tools_pending_discovery_has_no_conversation_authority() {
    let (client, database, store, state) = persisted().await;
    let source =
        MongoAgentAccessToolSource::new(store.clone(), "https://control.example.test", &key(9))
            .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let open = request(&state);
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = source.capability(&scope).ok().unwrap();
    source.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    source.authenticate_setup(&bearer).await.unwrap();
    assert!(source.authenticate(&bearer).await.is_err());
    let mut agent = store
        .get_agent(&scope.creation.agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.work.lease.as_mut().unwrap().fence = 2;
    agent.work.next_fence = 3;
    agent.revision += 1;
    store.save_agent(agent.revision - 1, agent).await.unwrap();
    assert!(source.authenticate_setup(&bearer).await.is_err());
    source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    source.authenticate_setup(&bearer).await.unwrap();
    assert!(source.authenticate(&bearer).await.is_err());
    let document = store
        .agent_access_tools
        .find_one(doc! {"_id": records::hash(&bearer)})
        .await
        .unwrap()
        .unwrap();
    assert!(source
        .records
        .read(&document)
        .ok()
        .unwrap()
        .conversation
        .is_none());
    client.database(&database).drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_confirmation_waits_for_owned_recorder_writer() {
    let (client, database, store, state) = persisted().await;
    let source =
        MongoAgentAccessToolSource::new(store.clone(), "https://control.example.test", &key(9))
            .unwrap();
    let binding = source
        .resolve(&RuntimeHandle("access-runtime".into()), &request(&state), 1)
        .await
        .unwrap()
        .unwrap();
    let id = AgentId(binding.scope().1 .0.clone());
    let writer = store.agent_writes.lock(&id).await;
    let mut transaction = client.start_session().await.unwrap();
    start_transaction(&mut transaction).await.unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$inc":{"resource_share_guard":1_i64}},
        )
        .session(&mut transaction)
        .await
        .unwrap();
    let confirmation =
        tokio::spawn(async move { source.confirm_open(&binding, "original-native").await });
    tokio::time::sleep(std::time::Duration::from_millis(2400)).await;
    let finished_while_owned = confirmation.is_finished();
    transaction.abort_transaction().await.unwrap();
    drop(writer);
    let result = confirmation.await.unwrap();
    client.database(&database).drop().await.unwrap();
    assert!(
        !finished_while_owned,
        "confirmation must wait for the owned recorder transaction"
    );
    result.unwrap();
}

struct ErasingOwner;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ErasingOwner {
    async fn erasing(
        &self,
        _: &SubjectId,
    ) -> std::result::Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_tools_standing_gate_denies_owner_capability_and_discovery() {
    let (client, database, store, state) = persisted().await;
    let source =
        MongoAgentAccessToolSource::new(store.clone(), "https://control.example.test", &key(9))
            .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = source.capability(&scope).ok().unwrap();
    let binding = source
        .resolve(&runtime, &request(&state), 1)
        .await
        .unwrap()
        .unwrap();
    source
        .confirm_open(&binding, "original-native")
        .await
        .unwrap();
    publish(&store, &scope.creation.agent_id).await;
    source.authenticate(&bearer).await.unwrap();
    let source = source.with_account_standing(Arc::new(ErasingOwner));
    assert!(source.authenticate(&bearer).await.is_err());
    assert!(source.authenticate_setup(&bearer).await.is_err());
    client.database(&database).drop().await.unwrap();
}

#[path = "numbered_open_tests.rs"]
mod numbered_open;
