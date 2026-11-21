use super::*;

fn organization_control() -> layer2_core::sandbox::control::SandboxControlRecord {
    use layer2_core::sandbox::control::*;
    let control = SandboxControl::Pause;
    let audit = ResourceAuthorizationAudit {
        target: ResourceTarget::Sandbox {
            sandbox_id: policy().sandbox_id.clone(),
        },
        sandbox_id: policy().sandbox_id,
        sandbox_created_at: policy().sandbox_created_at,
        owner: policy().owner,
        actor: actor("erased"),
        policy_revision: 1,
        capability: ResourceCapability::ManageSandbox,
        captured_at: NOW.into(),
        observations: vec![ResourceAuthorityObservation {
            capabilities: vec![ResourceCapability::ManageSandbox],
            basis: ResourceAuthorizationBasis::GlobalOperator,
        }],
    };
    let record = SandboxControlRecord {
        schema_version: 1,
        envelope: SandboxControlEnvelope {
            command_id: layer2_core::ids::SandboxCommandId("organization-pause".into()),
            sandbox_id: policy().sandbox_id,
            sandbox_created_at: policy().sandbox_created_at,
            owner: policy().owner,
            actor: actor("erased"),
            authorization: AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "organization-authority".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command: control,
            expected_revision: 1,
            request_fingerprint: control.fingerprint(1),
        },
        key_sha256: "a".repeat(64),
        admitted_at: NOW.into(),
        queue_deadline: "2026-11-14T00:05:00Z".into(),
        status: SandboxControlStatus::Applied {
            at: NOW.into(),
            sandbox_revision: 2,
            disposition: SandboxControlDisposition::Scheduled,
        },
        revision: 2,
        admission_audit: audit.clone(),
        execution_audit: Some(audit),
    };
    record.validate().unwrap();
    record
}

fn control_row(record: &layer2_core::sandbox::control::SandboxControlRecord) -> Document {
    doc! {"_id":record.envelope.command_id.as_str(),"document_schema_version":1_i64,"sandbox_id":record.envelope.sandbox_id.as_str(),
    "scope":record.envelope.scope(),"key_sha256":&record.key_sha256,"queued":record.queued(),"revision":record.revision as i64,
    "queue_deadline":&record.queue_deadline,"record":bson::serialize_to_bson(record).unwrap()}
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_control_keeps_its_terminal_audits_and_rejects_stale_redaction() {
    use layer2_core::sandbox::control::SandboxControlStore;
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    put(
        &db,
        "resource_policies",
        records::policy_document(&policy()).unwrap(),
    )
    .await;
    let control = organization_control();
    let stale = control_row(&control);
    put(&db, "sandbox_controls", stale.clone()).await;
    db.collection::<Document>("sandbox_controls")
        .update_one(
            doc! {"_id":control.envelope.command_id.as_str()},
            doc! {"$set":{"concurrent_evidence":"preserve"}},
        )
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "sandbox_controls", &stale, &subjects())
            .await
            .unwrap(),
        Redaction::Conflict
    );
    let current = row(
        &db,
        "sandbox_controls",
        control.envelope.command_id.as_str(),
    )
    .await;
    assert_eq!(
        redact(&db, "sandbox_controls", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let stored = row(
        &db,
        "sandbox_controls",
        control.envelope.command_id.as_str(),
    )
    .await;
    assert_eq!(stored.get_str("concurrent_evidence").unwrap(), "preserve");
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    let cleaned = store
        .sandbox_control(&control.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    cleaned.validate().unwrap();
    assert_eq!(cleaned.envelope.owner, control.envelope.owner);
    assert_eq!(cleaned.status, control.status);
    assert_eq!(cleaned.envelope.actor, actor("deleted-account"));
    assert_eq!(cleaned.admission_audit.actor, cleaned.envelope.actor);
    assert_eq!(
        cleaned.execution_audit.unwrap().actor,
        cleaned.envelope.actor
    );
    assert_ne!(cleaned.envelope.scope(), control.envelope.scope());
    assert_eq!(stored.get_str("scope").unwrap(), cleaned.envelope.scope());
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

fn organization_artifact() -> layer2_core::sandbox::agent_artifacts::StoredAgentArtifact {
    use layer2_core::sandbox::agent_artifacts::*;
    let record = StoredAgentArtifact {
        metadata: AgentArtifactMetadata {
            id: format!("art_{}", "a".repeat(64)),
            agent_id: AgentId("agt_org".into()),
            name: "organization.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: 5,
            sha256: layer2_core::document_artifacts::digest(b"hello"),
            created_at: NOW.into(),
        },
        binding: ArtifactBinding {
            sandbox_id: policy().sandbox_id,
            sandbox_created_at: policy().sandbox_created_at,
            workspace_id: WorkspaceId("wsp_org".into()),
            workspace_created_at: NOW.into(),
            worktree_id: WorktreeId("wtr_org".into()),
            worktree_identity_sha256: "c".repeat(64),
            agent_id: AgentId("agt_org".into()),
            agent_created_at: NOW.into(),
            owner: policy().owner,
        },
        content_base64: "aGVsbG8=".into(),
        authorization: ResourceAuthorizationAudit {
            target: ResourceTarget::Agent {
                agent_id: AgentId("agt_org".into()),
            },
            sandbox_id: policy().sandbox_id,
            sandbox_created_at: policy().sandbox_created_at,
            owner: policy().owner,
            actor: actor("erased"),
            policy_revision: 1,
            capability: ResourceCapability::ManageSharing,
            captured_at: NOW.into(),
            observations: vec![ResourceAuthorityObservation {
                capabilities: vec![ResourceCapability::ManageSharing],
                basis: ResourceAuthorizationBasis::ShareRecipient {
                    subject: SubjectId::new("erased"),
                    share: ResourceGrantObservation {
                        id: "organization-grant".into(),
                        revision: 1,
                        mode: ShareMode::ViewWrite,
                        selector: ResourceSelector::Agent {
                            agent_id: AgentId("agt_org".into()),
                        },
                    },
                },
            }],
        },
    };
    record.content().unwrap();
    record
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_artifact_keeps_bytes_and_binding_and_rejects_stale_redaction() {
    use layer2_core::sandbox::agent_artifacts::AgentArtifactStore;
    let db = database().await;
    let artifact = organization_artifact();
    let stale = doc! {"_id":&artifact.metadata.id,"document_schema_version":1_i64,"agent_id":"agt_org",
    "binding":bson::serialize_to_bson(&artifact.binding).unwrap(),"record":bson::serialize_to_bson(&artifact).unwrap()};
    put(&db, "agent_artifacts", stale.clone()).await;
    db.collection::<Document>("agent_artifacts")
        .update_one(
            doc! {"_id":&artifact.metadata.id},
            doc! {"$set":{"concurrent_evidence":"preserve"}},
        )
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "agent_artifacts", &stale, &subjects())
            .await
            .unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "agent_artifacts", &artifact.metadata.id).await;
    assert_eq!(
        redact(&db, "agent_artifacts", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    let cleaned = store
        .agent_artifact(&artifact.binding, &artifact.metadata.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleaned.content().unwrap(), b"hello");
    assert_eq!(cleaned.binding, artifact.binding);
    assert_eq!(cleaned.metadata, artifact.metadata);
    assert_eq!(cleaned.authorization.actor, actor("deleted-account"));
    assert!(
        matches!(&cleaned.authorization.observations[0].basis,ResourceAuthorizationBasis::ShareRecipient {subject,..} if subject.as_str()=="deleted-account")
    );
    assert_eq!(
        row(&db, "agent_artifacts", &artifact.metadata.id)
            .await
            .get_str("concurrent_evidence")
            .unwrap(),
        "preserve"
    );
    assert!(store
        .agent_artifact_metadata(&artifact.binding, &artifact.metadata.id)
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}
