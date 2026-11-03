use super::{super::resource_access_tests::context, Scenario};
use axum::Extension;
use layer2_core::document_artifacts::{
    memory::{MemoryDocumentBlobs, MemoryDocuments},
    CreateDocument, DocumentPermission, DocumentService, GrantDocument, UpdateDocument,
};
use layer2_core::sandbox::{
    agent_artifacts::{AgentArtifactService, PublishAgentArtifact},
    agent_filesystem::AgentFileService,
    answer_deliverables::{AnswerDeliverableService, DeliverableAccess, DeliverableResource},
    store::{AgentLogStore, AgentStore, SandboxStore},
    AgentLogKind,
};
use layer2_core::{AgentId, SandboxId};
use std::sync::Arc;

#[path = "filesystem_runtime.rs"]
mod runtime;

#[async_trait::async_trait]
impl layer3_adapter::BrowserAdapter for runtime::Files {
    async fn browser(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
        _request: &layer3_adapter::BrowserRequest,
    ) -> layer3_adapter::Result<layer3_adapter::BrowserReceipt> {
        Err(layer3_adapter::AdapterError::Unsupported(
            "deliverable fixture has no browser".into(),
        ))
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("deliverable_")?;
    assert!([
        "owner",
        "edited",
        "changed_file",
        "viewer",
        "denied",
        "revoked",
        "foreign",
        "anonymous",
        "unavailable",
    ]
    .contains(&mode));
    let (fixture, access, organization, _) =
        super::super::shared_agent_tests::fixture(super::super::shared_agent_tests::Reply::Member)
            .await;
    let agent_id = AgentId("agt_shared_read".into());
    let owner = context("owner-a");
    let mut sandbox = fixture
        .store
        .get_sandbox(&SandboxId("sbx_route".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    sandbox.lifecycle.home = Some(Box::new(runtime::persistent_home()));
    sandbox.revision += 1;
    fixture.store.save_sandbox(revision, sandbox).await.unwrap();
    let runtime = Arc::new(runtime::Files::new("owner"));
    let files = AgentFileService::new(access.clone(), runtime.clone());
    let artifacts = AgentArtifactService::new(fixture.store.clone(), access);
    let artifact = artifacts
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &agent_id,
            "delivery-contract",
            PublishAgentArtifact {
                name: "result.txt".into(),
                media_type: "text/plain".into(),
                content_base64: "YWJj".into(),
            },
        )
        .await
        .unwrap();
    let documents = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    );
    let document = documents
        .create(
            &owner.caller,
            "delivery-contract",
            CreateDocument {
                title: "Report".into(),
                markdown: "# Original\n".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let base = AnswerDeliverableService::new(fixture.reads(), artifacts);
    let service = base.with_documents(documents.clone()).with_files(files);
    let authority = DeliverableAccess {
        caller: &owner.caller,
        context: Some(&owner),
        credential: None,
        document_token: None,
    };
    let mut markers = Vec::new();
    for resource in [
        DeliverableResource::Document {
            id: document.id.clone(),
            revision: 1,
        },
        DeliverableResource::AgentArtifact { id: artifact.id },
        DeliverableResource::LiveFile {
            path: "cma/hello world.txt".into(),
        },
    ] {
        markers.push(
            service
                .describe(&authority, &agent_id, resource)
                .await
                .unwrap()
                .marker,
        );
    }
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 10,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Deliver the selected results".into(),
        },
        AgentLogKind::AgentMessage {
            content: markers.join("\n"),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 10 },
    ] {
        fixture
            .store
            .append_agent_log(&agent_id, "2026-11-03T00:00:00Z".into(), kind)
            .await
            .unwrap();
    }
    let mut agent = fixture.store.get_agent(&agent_id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.last_seq = 13;
    agent.revision += 1;
    fixture.store.save_agent(revision, agent).await.unwrap();
    if mode == "edited" {
        documents
            .update(
                &owner.caller,
                &document.id,
                "edit",
                UpdateDocument {
                    expected_revision: 1,
                    title: None,
                    markdown: Some("# Edited\n".into()),
                },
            )
            .await
            .unwrap();
    }
    if matches!(mode, "viewer" | "revoked") {
        let granted = documents
            .grant(
                &owner.caller,
                &document.id,
                "grant",
                "recipient",
                GrantDocument {
                    expected_revision: 1,
                    permission: DocumentPermission::Read,
                },
            )
            .await
            .unwrap();
        if mode == "revoked" {
            documents
                .revoke(
                    &owner.caller,
                    &document.id,
                    "revoke",
                    "recipient",
                    granted.revision,
                )
                .await
                .unwrap();
        }
    }
    if mode == "changed_file" {
        runtime.entries.lock().unwrap().insert(
            "cma/hello world.txt".into(),
            Some(b"changed bytes\n".to_vec()),
        );
    }
    let subject = match mode {
        "viewer" | "denied" | "revoked" => "recipient",
        "foreign" => "foreign",
        _ => "owner-a",
    };
    let caller = context(subject);
    if mode == "foreign" {
        *organization.0.lock().unwrap() = super::super::shared_agent_tests::Reply::Missing;
    }
    Some(Scenario {
        router: super::super::answer_deliverables_router(if mode == "unavailable" {
            base
        } else {
            service
        })
        .layer(Extension(caller.clone()))
        .layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        ))),
        subject: (mode != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
