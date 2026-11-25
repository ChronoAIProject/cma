use super::filesystem::context;
use super::*;
use crate::resource_access::*;
use crate::sandbox::answer_deliverables::*;

struct NoRuntimeResources;
#[async_trait::async_trait]
impl SandboxResourceReader for NoRuntimeResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("answer reads do not request runtime metrics")
    }
}

#[tokio::test]
async fn answer_deliverables_list_immutable_metadata_without_reading_bodies() {
    let store = Arc::new(store::InMemorySandboxStore::default());
    store.create_sandbox(sandbox()).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    let mut agent = agent();
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some("native-delivery".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 6;
    store.create_agent(agent.clone()).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(super::filesystem::NoOrganizations),
    ));
    let artifacts = agent_artifacts::AgentArtifactService::new(store.clone(), access.clone());
    let service = AnswerDeliverableService::new(
        AgentReadService::new(store.clone(), Arc::new(NoRuntimeResources))
            .with_resource_access(access),
        artifacts.clone(),
    );
    let owner = context("owner");
    let authority = DeliverableAccess {
        caller: &owner.caller,
        context: None,
        credential: None,
        document_token: None,
    };
    let published = artifacts
        .publish(
            &owner.caller,
            None,
            None,
            &agent.id,
            "immutable-result",
            agent_artifacts::PublishAgentArtifact {
                name: "result.txt".into(),
                media_type: "text/plain".into(),
                content_base64: "YWJj".into(),
            },
        )
        .await
        .unwrap();
    let described = service
        .describe(
            &authority,
            &agent.id,
            DeliverableResource::AgentArtifact { id: published.id },
        )
        .await
        .unwrap();
    for kind in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-delivery".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            model: None,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Produce a result".into(),
        },
        AgentLogKind::AgentMessage {
            content: described.marker.clone(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let before = store.artifact_body_reads();
    let listed = service.list(&authority, &agent.id, 5).await.unwrap();
    assert_eq!(listed.items[0].status, DeliverableStatus::Ready);
    assert_eq!(listed.items[0].reference.size_bytes, 3);
    assert_eq!(
        store.artifact_body_reads(),
        before,
        "delivery metadata must not fetch artifact bodies"
    );
    assert_eq!(
        service
            .download(&authority, &agent.id, 5, &listed.items[0].id)
            .await
            .unwrap()
            .1,
        b"abc"
    );
    for gap_after_answer in [false, true] {
        let turn_sequence = agent.conversation.last_seq + 1;
        let mut events = vec![
            AgentLogKind::TurnStarted {
                turn_sequence,
                model: None,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Produce another result".into(),
            },
        ];
        if !gap_after_answer {
            events.push(AgentLogKind::ConversationGap { frames: 1 });
        }
        events.push(AgentLogKind::AgentMessage {
            content: described.marker.clone(),
        });
        let answer_sequence = agent.conversation.last_seq + events.len() as u64;
        if gap_after_answer {
            events.push(AgentLogKind::ConversationGap { frames: 1 });
        }
        events.push(AgentLogKind::TurnCompleted { turn_sequence });
        agent.conversation.last_seq += events.len() as u64;
        agent.revision += 1;
        store
            .save_agent(agent.revision - 1, agent.clone())
            .await
            .unwrap();
        for kind in events {
            store
                .append_agent_log(&agent.id, NOW.into(), kind)
                .await
                .unwrap();
        }
        let delivery_id = &listed.items[0].id;
        assert!(matches!(
            service.list(&authority, &agent.id, answer_sequence).await,
            Err(DeliverableError::Answer(AgentReadError::NotFound))
        ));
        for verify in [false, true] {
            assert!(matches!(
                service
                    .read_item(&authority, &agent.id, answer_sequence, delivery_id, verify)
                    .await,
                Err(DeliverableError::Answer(AgentReadError::NotFound))
            ));
        }
        assert!(matches!(
            service
                .inspect(&authority, &agent.id, answer_sequence, delivery_id)
                .await,
            Err(DeliverableError::Answer(AgentReadError::NotFound))
        ));
        assert!(matches!(
            service
                .download(&authority, &agent.id, answer_sequence, delivery_id)
                .await,
            Err(DeliverableError::Answer(AgentReadError::NotFound))
        ));
    }
}

#[tokio::test]
async fn answer_deliverables_reject_unverified_descriptor_metadata() {
    let store = Arc::new(store::InMemorySandboxStore::default());
    store.create_sandbox(sandbox()).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    let mut agent = agent();
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some("native-delivery".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 6;
    store.create_agent(agent.clone()).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(super::filesystem::NoOrganizations),
    ));
    let artifacts = agent_artifacts::AgentArtifactService::new(store.clone(), access.clone());
    let service = AnswerDeliverableService::new(
        AgentReadService::new(store.clone(), Arc::new(NoRuntimeResources))
            .with_resource_access(access),
        artifacts,
    );
    let owner = context("owner");
    let authority = DeliverableAccess {
        caller: &owner.caller,
        context: None,
        credential: None,
        document_token: None,
    };
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "agent_id":"agt_test", "resource":{"kind":"live_file","path":"../private"},
            "name":"../private", "media_type":"text/html", "size_bytes":0, "sha256":"not-a-digest",
        }))
        .unwrap(),
    );
    for kind in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-delivery".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            model: None,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Produce a result".into(),
        },
        AgentLogKind::AgentMessage {
            content: format!("<!--cma-deliverable:v1:{encoded}-->"),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    assert!(matches!(
        service.list(&authority, &agent.id, 5).await,
        Err(DeliverableError::Invalid)
    ));
}

#[tokio::test]
async fn answer_deliverables_live_file_never_becomes_a_snapshot_or_shared_artifact() {
    let root = std::env::temp_dir().join(format!(
        "cma-deliverable-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("result.bin"), b"original").unwrap();
    let store = Arc::new(store::InMemorySandboxStore::default());
    let mut sandbox = sandbox();
    sandbox.runtime = Some(RuntimeBinding {
        boot_id: Some("b".repeat(32)),
        runtime_id: "runtime-test".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.home = Some(Box::new(super::attachments::persistent_home()));
    sandbox.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(sandbox).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    let mut agent = agent();
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some("native-delivery".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 6;
    store.create_agent(agent.clone()).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(super::filesystem::OrganizationAdmin),
    ));
    let artifacts = agent_artifacts::AgentArtifactService::new(store.clone(), access.clone());
    let file_reads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let files = agent_filesystem::AgentFileService::new(
        access.clone(),
        Arc::new(super::attachments::runtime::UnusedRuntime {
            status: None,
            file_root: Some((root.clone(), file_reads.clone())),
        }),
    );
    let service = AnswerDeliverableService::new(
        AgentReadService::new(store.clone(), Arc::new(NoRuntimeResources))
            .with_resource_access(access.clone()),
        artifacts.clone(),
    )
    .with_files(files);
    let owner = context("owner");
    let authority = DeliverableAccess {
        caller: &owner.caller,
        context: None,
        credential: None,
        document_token: None,
    };
    let described = service
        .describe(
            &authority,
            &agent.id,
            DeliverableResource::LiveFile {
                path: "result.bin".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(described.reference.size_bytes, 8);
    assert_eq!(described.reference.media_type, "application/octet-stream");
    for kind in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-delivery".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            model: None,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Produce a binary file".into(),
        },
        AgentLogKind::AgentMessage {
            content: described.marker,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let reads_before_list = file_reads.lock().unwrap().len();
    let original = service.list(&authority, &agent.id, 5).await.unwrap();
    assert_eq!(file_reads.lock().unwrap().len(), reads_before_list);
    let delivery = &original.items[0].id;
    for turn_sequence in (7..10007).step_by(4) {
        for kind in [
            AgentLogKind::TurnStarted {
                turn_sequence,
                model: None,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Unrelated task".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Unrelated answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence },
        ] {
            store
                .append_agent_log(&agent.id, NOW.into(), kind)
                .await
                .unwrap();
        }
    }
    let mut current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let revision = current.revision;
    current.revision += 1;
    current.conversation.last_seq = 10006;
    store.save_agent(revision, current).await.unwrap();
    let before = store.log_rows_read();
    assert_eq!(
        service.list(&authority, &agent.id, 5).await.unwrap().items[0].id,
        *delivery
    );
    assert!(
        store.log_rows_read() - before <= 128,
        "one delivery lookup must not reconstruct a 10,006-event conversation"
    );
    assert_eq!(
        serde_json::to_value(&original.items[0]).unwrap()["status"],
        "unchecked"
    );
    assert_eq!(
        service
            .download(&authority, &agent.id, 5, delivery)
            .await
            .unwrap()
            .1,
        b"original"
    );
    let viewer = context("viewer");
    let shared = DeliverableAccess {
        caller: &viewer.caller,
        context: Some(&viewer),
        credential: Some("credential"),
        document_token: None,
    };
    access
        .change_share(
            &owner,
            Some("credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: agent.id.clone(),
                },
                change: ShareChange::Create {
                    target_org_id: crate::SubjectId("organization".into()),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "view-answer".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service.list(&shared, &agent.id, 5).await.unwrap().items[0].status,
        DeliverableStatus::Denied
    );
    assert!(matches!(
        service.download(&shared, &agent.id, 5, delivery).await,
        Err(DeliverableError::File(
            agent_filesystem::AgentFileError::Access(_)
        ))
    ));
    let write = access
        .create_agent_share_link(
            &owner,
            Some("credential"),
            CreateAgentShareLink {
                agent_id: agent.id.clone(),
                audience: AgentShareAudience::Public,
                mode: ShareMode::ViewWrite,
                token: "d".repeat(64),
                idempotency_key: "view-files".into(),
            },
        )
        .await
        .unwrap();
    let accepted = access
        .redeem_agent_share_link(
            &viewer,
            Some("credential"),
            &write.share.id,
            &"d".repeat(64),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .download(&shared, &agent.id, 5, delivery)
            .await
            .unwrap()
            .1,
        b"original"
    );
    access
        .change_share(
            &owner,
            Some("credential"),
            ShareCommand {
                selector: write.share.selector,
                change: ShareChange::Revoke {
                    share_id: write.share.id,
                },
                expected_share_revision: Some(accepted.revision),
                idempotency_key: "revoke-files".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service.list(&shared, &agent.id, 5).await.unwrap().items[0].status,
        DeliverableStatus::Denied
    );
    let grown = std::fs::File::create(root.join("result.bin")).unwrap();
    grown.set_len(2 * 1024 * 1024).unwrap();
    let reads_before_metadata = file_reads.lock().unwrap().len();
    let metadata = service
        .read_item(&authority, &agent.id, 5, delivery, false)
        .await
        .unwrap();
    assert_eq!(metadata.item.reference.size_bytes, 8);
    assert_eq!(metadata.item.status, DeliverableStatus::Unchecked);
    assert_eq!(file_reads.lock().unwrap().len(), reads_before_metadata);
    assert!(matches!(
        service.download(&authority, &agent.id, 5, delivery).await,
        Err(DeliverableError::Changed)
    ));
    assert_eq!(file_reads.lock().unwrap().last(), Some(&8));
    assert_eq!(
        &file_reads.lock().unwrap()[..2],
        &[layer3_adapter::MAX_AGENT_FILE_BYTES, 8]
    );
    std::fs::write(root.join("result.bin"), b"modified").unwrap();
    assert_eq!(
        service
            .inspect(&authority, &agent.id, 5, delivery)
            .await
            .unwrap()
            .status,
        DeliverableStatus::Changed
    );
    assert!(matches!(
        service.download(&authority, &agent.id, 5, delivery).await,
        Err(DeliverableError::Changed)
    ));
    std::fs::remove_file(root.join("result.bin")).unwrap();
    assert_eq!(
        service
            .inspect(&authority, &agent.id, 5, delivery)
            .await
            .unwrap()
            .status,
        DeliverableStatus::Missing
    );
    let oversized = std::fs::File::create(root.join("result.bin")).unwrap();
    oversized
        .set_len(layer3_adapter::MAX_AGENT_FILE_BYTES + 1)
        .unwrap();
    let unavailable = service
        .inspect(&authority, &agent.id, 5, delivery)
        .await
        .unwrap();
    assert_eq!(unavailable.status, DeliverableStatus::Changed);
    assert!(matches!(
        service
            .describe(
                &authority,
                &agent.id,
                DeliverableResource::LiveFile {
                    path: "result.bin".into(),
                },
            )
            .await,
        Err(DeliverableError::File(
            agent_filesystem::AgentFileError::TooLarge
        ))
    ));
    assert_eq!(
        file_reads.lock().unwrap().last(),
        Some(&layer3_adapter::MAX_AGENT_FILE_BYTES)
    );
    std::fs::write(root.join("result.bin"), b"").unwrap();
    assert!(matches!(
        service.download(&authority, &agent.id, 5, delivery).await,
        Err(DeliverableError::Changed)
    ));
    let empty = service
        .describe(
            &authority,
            &agent.id,
            DeliverableResource::LiveFile {
                path: "result.bin".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(empty.reference.size_bytes, 0);
    assert_eq!(file_reads.lock().unwrap().last(), Some(&1));
    let mut current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let turn_sequence = current.conversation.last_seq + 1;
    let answer_sequence = turn_sequence + 2;
    for kind in [
        AgentLogKind::TurnStarted {
            turn_sequence,
            model: None,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Keep the empty result".into(),
        },
        AgentLogKind::AgentMessage {
            content: empty.marker,
        },
        AgentLogKind::TurnCompleted { turn_sequence },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let revision = current.revision;
    current.revision += 1;
    current.conversation.last_seq = answer_sequence + 1;
    store.save_agent(revision, current).await.unwrap();
    let empty_id = empty.reference.id().unwrap();
    assert!(service
        .download(&authority, &agent.id, answer_sequence, &empty_id)
        .await
        .unwrap()
        .1
        .is_empty());
    for content in [vec![b'x'], vec![b'x'; 2 * 1024 * 1024]] {
        std::fs::write(root.join("result.bin"), content).unwrap();
        assert!(matches!(
            service
                .download(&authority, &agent.id, answer_sequence, &empty_id)
                .await,
            Err(DeliverableError::Changed)
        ));
        assert_eq!(file_reads.lock().unwrap().last(), Some(&1));
    }
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = sandbox.revision;
    sandbox.revision += 1;
    sandbox.status = SandboxStatus::Pausing;
    store.save_sandbox(revision, sandbox).await.unwrap();
    let unavailable =
        serde_json::to_value(service.list(&authority, &agent.id, 5).await.unwrap()).unwrap();
    assert_eq!(unavailable["items"][0]["status"], "not_ready");
    assert!(artifacts
        .list(
            &owner.caller,
            None,
            None,
            &agent.id,
            &agent_artifacts::ArtifactPageRequest {
                cursor: None,
                limit: 20
            }
        )
        .await
        .unwrap()
        .items
        .is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

struct ConfirmedAgent(crate::sandbox::agent_access_tool::AgentAccessRequester);

#[async_trait::async_trait]
impl crate::sandbox::agent_access_tool::AgentAccessToolAuthenticator for ConfirmedAgent {
    async fn authenticate_setup(
        &self,
        _: &str,
    ) -> Result<(), crate::sandbox::agent_access::AgentAccessError> {
        Ok(())
    }

    async fn authenticate(
        &self,
        _: &str,
    ) -> Result<
        crate::sandbox::agent_access_tool::AgentAccessRequester,
        crate::sandbox::agent_access::AgentAccessError,
    > {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn agent_deliverable_paths_resolve_within_the_agents_own_worktree() {
    use sha2::{Digest as _, Sha256};

    let root = std::env::temp_dir().join(format!(
        "cma-deliverable-paths-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // The runtime file API opens paths under /home. Agent worktrees are under /home/cma.
    let worktree_dir = root.join("cma/project/.worktrees/wt-1");
    std::fs::create_dir_all(worktree_dir.join("out")).unwrap();
    std::fs::write(worktree_dir.join("report.docx"), b"docx").unwrap();
    std::fs::write(worktree_dir.join("out/tables.csv"), b"a,b\n").unwrap();
    std::fs::write(worktree_dir.join("both.txt"), b"worktree").unwrap();
    std::fs::write(root.join("both.txt"), b"home").unwrap();
    let store = Arc::new(store::InMemorySandboxStore::default());
    let mut sandbox = sandbox();
    sandbox.runtime = Some(RuntimeBinding {
        boot_id: Some("b".repeat(32)),
        runtime_id: "runtime-test".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.home = Some(Box::new(super::attachments::persistent_home()));
    sandbox.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(sandbox).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    let mut agent = agent();
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some("native-delivery".into());
    agent.conversation.runtime_generation = Some(1);
    store.create_agent(agent.clone()).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(super::filesystem::OrganizationAdmin),
    ));
    let artifacts = agent_artifacts::AgentArtifactService::new(store.clone(), access.clone());
    let files = agent_filesystem::AgentFileService::new(
        access.clone(),
        Arc::new(super::attachments::runtime::UnusedRuntime {
            status: None,
            file_root: Some((root.clone(), Arc::default())),
        }),
    );
    let reads = AgentReadService::new(store.clone(), Arc::new(NoRuntimeResources))
        .with_resource_access(access.clone());
    let caller = context("owner").caller;
    let lineage = reads.lineage(&caller, &agent.id).await.unwrap();
    let service = AnswerDeliverableService::new(reads, artifacts).with_files(files);
    let auth = ConfirmedAgent(crate::sandbox::agent_access_tool::AgentAccessRequester {
        agent_id: agent.id.clone(),
        owner: agent.owner.clone(),
        sandbox_id: agent.sandbox_id.clone(),
        conversation: "native-delivery".into(),
        runtime: lineage.sandbox.runtime.clone().unwrap(),
        creation: crate::sandbox::provenance::AgentCreationProvenance {
            schema_version: 1,
            agent_id: agent.id.clone(),
            agent_created_at: agent.created_at.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            sandbox_created_at: lineage.sandbox.created_at.clone(),
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
            owner: ResourceOwner::Person {
                subject: agent.owner.clone(),
            },
            creation_origin: crate::sandbox::provenance::AgentCreationOrigin::Manual,
            created_by: None,
            provisioning_origin: agent.origin,
            creation_fingerprint: agent.creation_fingerprint.clone(),
        },
    });
    let describe = |path: &str| {
        service.describe_for_agent(
            &auth,
            "capability",
            DeliverableResource::LiveFile { path: path.into() },
        )
    };
    let report = "cma/project/.worktrees/wt-1/report.docx";
    let mut descriptions = Vec::new();
    for input in [
        "report.docx",
        "./report.docx",
        "/home/cma/project/.worktrees/wt-1/report.docx",
        report,
    ] {
        descriptions.push((input, describe(input).await));
    }
    assert!(
        descriptions.iter().all(|(_, result)| result.is_ok()),
        "{descriptions:?}"
    );
    let marker = &descriptions[3].1.as_ref().unwrap().marker;
    for (input, result) in &descriptions {
        let described = result.as_ref().unwrap();
        assert_eq!(
            described.reference.resource,
            DeliverableResource::LiveFile {
                path: report.into()
            },
            "{input}"
        );
        assert_eq!(described.reference.size_bytes, 4, "{input}");
        assert_eq!(
            described.reference.sha256,
            format!("{:x}", Sha256::digest(b"docx")),
            "{input}"
        );
        assert_eq!(&described.marker, marker, "{input}");
    }
    let table = describe("out/tables.csv").await.unwrap().reference;
    assert_eq!(
        table.resource,
        DeliverableResource::LiveFile {
            path: "cma/project/.worktrees/wt-1/out/tables.csv".into()
        }
    );
    assert_eq!(table.media_type, "text/csv");
    // The canonical /home-relative path wins when both files exist.
    let shared_name = describe("both.txt").await.unwrap().reference;
    assert_eq!(
        shared_name.resource,
        DeliverableResource::LiveFile {
            path: "both.txt".into()
        }
    );
    assert_eq!(shared_name.sha256, format!("{:x}", Sha256::digest(b"home")));
    assert!(matches!(
        describe("absent.docx").await,
        Err(DeliverableError::File(
            crate::sandbox::agent_filesystem::AgentFileError::NotFound
        ))
    ));
    for refused in [
        "/etc/hosts",
        "../report.docx",
        "",
        ".",
        "./",
        "/home/cma",
        "/home/cma/",
    ] {
        assert!(
            matches!(
                describe(refused).await,
                Err(DeliverableError::File(
                    crate::sandbox::agent_filesystem::AgentFileError::Invalid
                ))
            ),
            "{refused}"
        );
    }
    // A trailing separator names a directory; it never reaches a file.
    assert!(describe("report.docx/").await.is_err());
    let _ = std::fs::remove_dir_all(root);
}
