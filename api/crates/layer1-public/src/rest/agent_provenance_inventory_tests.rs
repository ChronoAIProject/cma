use super::{
    agents_tests,
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use layer2_core::{
    resource_access::*,
    sandbox::{
        store::{
            AgentChannelUsageSnapshot, AgentProvenanceInventoryStore, InMemorySandboxStore,
            PageRequest, SandboxStoreError,
        },
        AgentListQuery, AgentProvenanceInventoryService,
    },
    SubjectId,
};
use std::sync::Arc;

const PATH: &str = "/api/v1/agents/provenance";

#[tokio::test]
async fn task_navigation_projects_durable_outcomes_only_to_authorized_owners() {
    use layer2_core::sandbox::{AgentLogKind, AgentStatus};
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    for (id, completed) in [("agt_task_new", false), ("agt_task_done", true)] {
        let events = if completed {
            vec![
                AgentLogKind::TurnStarted {
                    turn_sequence: 3,
                    model: None,
                    origin_key: None,
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ]
        } else {
            vec![]
        };
        agents_tests::insert_agent(
            &fixture.store,
            agents_tests::seeded_agent(id, AgentStatus::Ready, if completed { 4 } else { 2 }, None),
            events,
        )
        .await;
    }
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    let (status, body) = agents_tests::json_response(&router, request("", "owner-a", true)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["items"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row["agent_id"] == "agt_task_new")
            .unwrap()["task_status"],
        "not_started"
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["agent_id"] == "agt_task_done")
            .unwrap()["task_status"],
        "completed"
    );
    for (subject, query) in [
        ("owner-b", ""),
        ("recipient", "?shared=true"),
        ("recipient", "?organization=org"),
    ] {
        let (status, body) =
            agents_tests::json_response(&router, request(query, subject, true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!body.to_string().contains("agt_task_"), "{body}");
    }
}

fn request(query: &str, subject: &str, verified: bool) -> Request<Body> {
    let mut request =
        agents_tests::request(Method::GET, &format!("{PATH}{query}"), subject, None, None);
    if verified {
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
    }
    request
}

#[tokio::test]
async fn task_navigation_distinguishes_setup_completion_using_recorded_purpose_and_turn_identity() {
    use layer2_core::sandbox::{
        AgentLogKind, AgentStatus, FirstMessagePurpose, FirstMessageRecord,
    };
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let cases = [
        (
            "agt_setup_only",
            Some(FirstMessagePurpose::Setup),
            false,
            "not_started",
        ),
        (
            "agt_task_first",
            Some(FirstMessagePurpose::Task),
            false,
            "completed",
        ),
        ("agt_legacy_first", None, false, "unknown"),
        (
            "agt_task_after_setup",
            Some(FirstMessagePurpose::Setup),
            true,
            "completed",
        ),
    ];
    for (id, purpose, later_task, _) in cases {
        let at = "2026-09-05T00:00:00Z";
        let mut first = FirstMessageRecord::new(id, "Identical admission text", at).unwrap();
        first.purpose = purpose;
        first.fence_dispatch_for(3, 1, 1, at.into()).unwrap();
        first.complete(at.into()).unwrap();
        let mut agent = agents_tests::seeded_agent(
            id,
            AgentStatus::Ready,
            if later_task { 6 } else { 4 },
            None,
        );
        agent.conversation.first_message = Some(first);
        let mut events = vec![
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                model: None,
                origin_key: Some(id.into()),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ];
        if later_task {
            events.extend([
                AgentLogKind::TurnStarted {
                    turn_sequence: 5,
                    model: None,
                    origin_key: None,
                },
                AgentLogKind::TurnCompleted { turn_sequence: 5 },
            ]);
        }
        agents_tests::insert_agent(&fixture.store, agent, events).await;
    }
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    let (status, body) = agents_tests::json_response(&router, request("", "owner-a", true)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (id, _, _, expected) in cases {
        let row = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["agent_id"] == id)
            .unwrap();
        assert_eq!(row["task_status"], expected, "{id}");
    }
}

#[tokio::test]
async fn task_navigation_does_not_call_a_completed_agent_done_after_new_work_is_admitted() {
    use layer2_core::{
        ids::{AgentCommandId, AgentResponseId},
        sandbox::{
            agent_io::*,
            store::{SandboxMutationGuard, SandboxStore},
            AgentLogKind, AgentStatus,
        },
    };
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let agent = agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_pending", AgentStatus::Ready, 4, None),
        vec![
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                model: None,
                origin_key: None,
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    let parent = fixture
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let command = AgentCommand::message("Next task");
    let held = access
        .authorize_personal(
            &context("owner-a").caller,
            &ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            },
            ResourceCapability::ViewDetails,
        )
        .await
        .unwrap();
    fixture
        .store
        .reserve_command(CommandReservation {
            binding: None,
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId("cmd_task_navigation".into()),
                agent_id: agent.id.clone(),
                owner: AgentOwner::Person {
                    subject: agent.owner.clone(),
                },
                actor: context("owner-a").actor,
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "task-navigation".into(),
                },
                correlation_id: "task-navigation".into(),
                continuation: None,
                request_fingerprint: command.fingerprint().unwrap(),
                command,
                authorization: AgentAuthorizationRevision {
                    authority_id: "fixture-custody".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
            },
            key_sha256: command_key_digest("task-navigation").unwrap(),
            response_id: Some(AgentResponseId("resp_task_navigation".into())),
            admitted_at: layer2_core::now_rfc3339(),
            queue_deadline: "2030-09-05T00:00:00Z".into(),
            queue_capacity: 64,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&parent),
            policy: held.snapshot.fence(),
            authority_deadline: None,
            admission_audit: None,
        })
        .await
        .unwrap();
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    let (status, body) = agents_tests::json_response(&router, request("", "owner-a", true)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["agent_id"] == "agt_pending")
            .unwrap()["task_status"],
        "working"
    );
}

#[tokio::test]
async fn provenance_inventory_has_safe_personal_org_and_filtered_pages_without_parent_leaks() {
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    for verified in [false, true] {
        let (status, body) = agents_tests::json_response(
            &router,
            request("?limit=1&creation_origin=manual", "owner-a", verified),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert!(body["next"].is_string());
        let row = &body["items"][0];
        assert_eq!(row["creation_origin"], "manual");
        assert!(!row.to_string().contains("private"));
        for field in [
            "details",
            "owner",
            "created_by",
            "profile",
            "sandbox_id",
            "worktree_id",
            "workspace_id",
        ] {
            assert!(row.get(field).is_none());
        }
    }
    let (status, body) = agents_tests::json_response(
        &router,
        request(
            "?organization=org&creation_origin=manual",
            "recipient",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["agent_id"], "agt_shared_read");
    assert!(body.get("next").is_none());
    for parent in ["sandbox=sbx_route", "workspace=wks_route"] {
        let (status, _) = agents_tests::json_response(
            &router,
            request(&format!("?organization=org&{parent}"), "recipient", true),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, body) = agents_tests::json_response(
        &router,
        request(
            "?organization=org&creation_origin=channel_bridge",
            "recipient",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"].as_array().unwrap().is_empty());
    assert!(body.get("next").is_none());
}

#[tokio::test]
async fn provenance_inventory_validates_scope_authority_and_rejects_forged_filters() {
    let (fixture, access, authority, _) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    for query in [
        "?creation_origin=spoofed",
        "?created_by=someone",
        "?limit=0",
        "?limit=201",
        "?cursor=",
        "?organization=%20org",
    ] {
        let (status, body) =
            agents_tests::json_response(&router, request(query, "owner-a", true)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    assert_eq!(
        agents_tests::json_response(&router, request("?organization=org", "recipient", false))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    *authority.0.lock().unwrap() = Reply::Outage;
    assert_eq!(
        agents_tests::json_response(&router, request("?organization=org", "recipient", true))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        agents_tests::json_response(&router, request("", "owner-a", true))
            .await
            .0,
        StatusCode::OK
    );
    *authority.0.lock().unwrap() = Reply::Missing;
    assert_eq!(
        agents_tests::json_response(&router, request("?organization=org", "recipient", true))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn task_navigation_filters_shared_failed_turns_even_when_the_agent_is_ready() {
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    for scope in ["shared=true", "organization=org"] {
        let (status, body) = agents_tests::json_response(
            &router,
            request(&format!("?{scope}&task_status=failed"), "recipient", true),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["task_status"], "failed");
        assert_eq!(body["items"][0]["status"], "ready");
        let (status, body) = agents_tests::json_response(
            &router,
            request(
                &format!("?{scope}&task_status=completed"),
                "recipient",
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["items"].as_array().unwrap().is_empty());
    }
}

#[tokio::test]
async fn task_navigation_shared_titles_keep_owner_priority_and_parent_pause_barriers() {
    use layer2_core::sandbox::{
        store::{AgentLogStore, AgentStore, SandboxStore},
        AgentLogKind, SandboxStatus,
    };
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let mut agent = fixture
        .store
        .get_agent(&layer2_core::AgentId("agt_shared_read".into()))
        .await
        .unwrap()
        .unwrap();
    agent.conversation.preferences.title = Some("Owner task name".into());
    agent.conversation.observations.metadata.description = Some("Provider task name".into());
    let revision = agent.revision;
    agent.revision += 1;
    fixture
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    let mut parent = fixture
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Paused;
    parent.revision += 1;
    fixture.store.save_sandbox(revision, parent).await.unwrap();
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store.clone(),
        access,
    ));
    let (status, body) =
        agents_tests::json_response(&router, request("?shared=true", "recipient", true)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][0]["title"], "Owner task name");
    assert_eq!(body["items"][0]["task_status"], "failed");
    assert!(body["items"][0].get("runtime_metadata").is_none());
    for kind in [
        AgentLogKind::TurnStarted {
            turn_sequence: 10,
            model: None,
            origin_key: None,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 10 },
    ] {
        fixture
            .store
            .append_agent_log(&agent.id, layer2_core::now_rfc3339(), kind)
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 11;
    fixture.store.save_agent(revision, agent).await.unwrap();
    let (status, body) =
        agents_tests::json_response(&router, request("?shared=true", "recipient", true)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][0]["task_status"], "paused");
}

struct GatedInventory {
    inner: Arc<InMemorySandboxStore>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    corrupt: bool,
}
#[async_trait::async_trait]
impl AgentProvenanceInventoryStore for GatedInventory {
    async fn scan_agent_provenance_inventory(
        &self,
        scope: &ResourceOwner,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
        let mut result = self
            .inner
            .scan_agent_provenance_inventory(scope, query, page)
            .await?;
        if self.corrupt {
            result[0].provenance.creation = Some(
                result[0]
                    .provenance
                    .creation_provenance(
                        &result[0]
                            .provenance
                            .resource
                            .lineage
                            .agent
                            .as_ref()
                            .unwrap()
                            .id,
                    )
                    .unwrap(),
            );
            result[0]
                .provenance
                .creation
                .as_mut()
                .unwrap()
                .agent_created_at = "corrupt".into();
        }
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(result)
    }
    async fn scan_shared_agent_provenance_inventory(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
        let result = self
            .inner
            .scan_shared_agent_provenance_inventory(recipient, organizations, query, page)
            .await?;
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(result)
    }
}

#[tokio::test]
async fn provenance_inventory_rechecks_revoked_rows_even_when_filtered_out_or_corrupt() {
    for corrupt in [false, true] {
        let (fixture, access, _, share) = fixture(Reply::Member).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let store = Arc::new(GatedInventory {
            inner: fixture.store,
            entered: entered.clone(),
            resume: resume.clone(),
            corrupt,
        });
        let service = AgentProvenanceInventoryService::new(store, access.clone());
        let pending = tokio::spawn(async move {
            service
                .list(
                    &context("recipient"),
                    Some("request-credential"),
                    &layer2_core::sandbox::AgentProvenanceInventoryQuery {
                        organization: Some(SubjectId::new("org")),
                        creation_origin: Some(
                            layer2_core::sandbox::AgentCreationCategory::ChannelBridge,
                        ),
                        ..Default::default()
                    },
                    &PageRequest::default(),
                )
                .await
        });
        entered.notified().await;
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "provenance-inventory-revoke".into(),
                },
            )
            .await
            .unwrap();
        resume.notify_one();
        assert!(matches!(
            pending.await.unwrap(),
            Err(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            ))
        ));
    }
}

struct ShortAuthority(OrganizationRole);
#[async_trait::async_trait]
impl OrganizationAuthority for ShortAuthority {
    async fn organization_context(
        &self,
        caller: &layer2_core::Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        current.caller = caller.clone();
        let now = std::time::SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            org.clone(),
            self.0,
            now,
            now + std::time::Duration::from_secs(1),
        )?);
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &layer2_core::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![OrganizationChoice {
            id: SubjectId::new("org"),
            role: OrganizationRole::Admin,
            display_name: None,
        }])
    }
}

#[tokio::test]
async fn provenance_inventory_expired_scope_refuses_empty_filtered_and_corrupt_pages_with_operator_read(
) {
    use layer2_core::sandbox::{AgentCreationCategory, AgentProvenanceInventoryQuery};
    for (empty, corrupt) in [(true, false), (false, false), (false, true)] {
        let (fixture, _, _, _) = fixture(Reply::Member).await;
        let access = Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(ShortAuthority(OrganizationRole::Admin)),
        ));
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let service = AgentProvenanceInventoryService::new(
            Arc::new(GatedInventory {
                inner: fixture.store,
                entered: entered.clone(),
                resume: resume.clone(),
                corrupt,
            }),
            access,
        );
        let mut caller = context("recipient");
        caller.caller.authorization = layer2_core::Authorization::new([
            layer2_core::Capability::UseOwnSandboxes,
            layer2_core::Capability::ReadAllSandboxes,
        ]);
        let pending = tokio::spawn(async move {
            service
                .list(
                    &caller,
                    Some("request-credential"),
                    &AgentProvenanceInventoryQuery {
                        organization: Some(SubjectId::new(if empty { "empty-org" } else { "org" })),
                        creation_origin: Some(AgentCreationCategory::ChannelBridge),
                        ..Default::default()
                    },
                    &PageRequest::default(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        resume.notify_one();
        let error = pending.await.unwrap().unwrap_err();
        tokio::time::resume();
        assert!(matches!(
            error,
            ResourceAccessError::Access(AccessError::RevalidationRequired)
        ));
    }
}

#[tokio::test]
async fn provenance_inventory_org_ownership_is_explicit_for_every_read_role_and_same_subject_personal_scope(
) {
    use layer2_core::sandbox::{store::SandboxStore, AgentProvenanceInventoryQuery};
    for role in [
        OrganizationRole::Viewer,
        OrganizationRole::Member,
        OrganizationRole::Admin,
    ] {
        let fixture = agents_tests::fixture().await;
        for id in ["agt_org_first", "agt_org_second"] {
            agents_tests::insert_agent(
                &fixture.store,
                agents_tests::seeded_agent(id, layer2_core::sandbox::AgentStatus::Ready, 0, None),
                Vec::new(),
            )
            .await;
        }
        let root = fixture
            .store
            .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
            .await
            .unwrap()
            .unwrap();
        fixture
            .store
            .set_resource_policy_for_test(ResourcePolicyRecord {
                sandbox_id: root.id.clone(),
                sandbox_created_at: root.created_at.clone(),
                owner: ResourceOwner::Organization {
                    subject: root.owner.clone(),
                },
                revision: 1,
                organization_evidence: Some(OrganizationOwnershipEvidence {
                    issuer: "https://identity.test".into(),
                    organization: root.owner.clone(),
                    admitted_by: context("manager").actor,
                    verified_at: root.created_at,
                }),
            });
        let access = Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(ShortAuthority(role)),
        ));
        let service = AgentProvenanceInventoryService::new(fixture.store, access);
        let org = service
            .list(
                &context("reader"),
                Some("request-credential"),
                &AgentProvenanceInventoryQuery {
                    organization: Some(root.owner.clone()),
                    agents: AgentListQuery {
                        sandbox_id: Some(root.id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                &PageRequest::default(),
            )
            .await
            .unwrap();
        assert_eq!(org.items.len(), 2);
        assert!(org.next.is_none());
        assert!(service
            .list(
                &context(root.owner.as_str()),
                None,
                &AgentProvenanceInventoryQuery::default(),
                &PageRequest::default()
            )
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn shared_provenance_inventory_unions_current_grants_without_parent_or_personal_leaks() {
    use layer2_core::{AgentId, SandboxId};
    let (fixture, access, authority, original) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access.clone(),
    ));
    for (agent, org, key) in [
        ("agt_shared_read", "org-two", "overlap"),
        ("agt_sibling", "org-two", "second"),
    ] {
        access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: AgentId(agent.into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new(org),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: key.into(),
                },
            )
            .await
            .unwrap();
    }
    let (status, first) =
        agents_tests::json_response(&router, request("?shared=true&limit=1", "recipient", true))
            .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["next"], "agt_shared_read");
    for hidden in [
        "sandbox_id",
        "workspace_id",
        "worktree_id",
        "profile",
        "private",
        "owner",
    ] {
        assert!(!first.to_string().contains(hidden));
    }
    let (status, last) = agents_tests::json_response(
        &router,
        request(
            "?shared=true&limit=1&cursor=agt_shared_read",
            "recipient",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last["items"][0]["agent_id"], "agt_sibling");
    assert!(last.get("next").is_none());
    for filter in ["sandbox=sbx_route", "workspace=wks_route"] {
        assert_eq!(
            agents_tests::json_response(
                &router,
                request(&format!("?shared=true&{filter}"), "recipient", true)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, owner) =
        agents_tests::json_response(&router, request("?shared=true", "owner-a", true)).await;
    assert_eq!(owner["items"], serde_json::json!([]));
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: original.selector,
                change: ShareChange::Revoke {
                    share_id: original.id,
                },
                expected_share_revision: Some(original.revision),
                idempotency_key: "revoke-original".into(),
            },
        )
        .await
        .unwrap();
    let (status, overlap) =
        agents_tests::json_response(&router, request("?shared=true", "recipient", true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overlap["items"].as_array().unwrap().len(), 2);
    // A whole-Sandbox grant independently permits parent filters; it is explicit.
    access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: ResourceSelector::Sandbox {
                    sandbox_id: SandboxId("sbx_route".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "whole".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        agents_tests::json_response(
            &router,
            request("?shared=true&sandbox=sbx_route", "recipient", true)
        )
        .await
        .0,
        StatusCode::OK
    );
    *authority.0.lock().unwrap() = Reply::Missing;
    let (status, removed) =
        agents_tests::json_response(&router, request("?shared=true", "recipient", true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["items"], serde_json::json!([]));
    *authority.0.lock().unwrap() = Reply::Outage;
    assert_eq!(
        agents_tests::json_response(&router, request("?shared=true", "recipient", true))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        agents_tests::json_response(&router, request("", "owner-a", true))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn shared_provenance_inventory_requires_context_and_unambiguous_scope() {
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    for query in [
        "?shared=true&organization=org",
        "?shared=invalid",
        "?shared=true&shared=false",
    ] {
        assert_eq!(
            agents_tests::json_response(&router, request(query, "recipient", true))
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    assert_eq!(
        agents_tests::json_response(&router, request("?shared=true", "recipient", false))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn shared_provenance_inventory_rechecks_filtered_candidates_after_revocation() {
    use layer2_core::sandbox::{AgentCreationCategory, AgentProvenanceInventoryQuery};
    let (fixture, access, _, share) = fixture(Reply::Member).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let service = AgentProvenanceInventoryService::new(
        Arc::new(GatedInventory {
            inner: fixture.store,
            entered: entered.clone(),
            resume: resume.clone(),
            corrupt: false,
        }),
        access.clone(),
    );
    let pending = tokio::spawn(async move {
        service
            .list(
                &context("recipient"),
                Some("request-credential"),
                &AgentProvenanceInventoryQuery {
                    shared: true,
                    creation_origin: Some(AgentCreationCategory::ChannelBridge),
                    ..Default::default()
                },
                &PageRequest::default(),
            )
            .await
    });
    entered.notified().await;
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-aggregate".into(),
            },
        )
        .await
        .unwrap();
    resume.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
}

#[tokio::test]
async fn shared_provenance_inventory_keeps_original_deadline_even_for_empty_pages() {
    use layer2_core::sandbox::AgentProvenanceInventoryQuery;
    for empty in [false, true] {
        let (fixture, old_access, _, share) = fixture(Reply::Member).await;
        if empty {
            old_access
                .change_share(
                    &context("owner-a"),
                    None,
                    ShareCommand {
                        selector: share.selector,
                        change: ShareChange::Revoke { share_id: share.id },
                        expected_share_revision: Some(share.revision),
                        idempotency_key: "empty-aggregate".into(),
                    },
                )
                .await
                .unwrap();
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let access = Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(ShortAuthority(OrganizationRole::Viewer)),
        ));
        let service = AgentProvenanceInventoryService::new(
            Arc::new(GatedInventory {
                inner: fixture.store,
                entered: entered.clone(),
                resume: resume.clone(),
                corrupt: false,
            }),
            access,
        );
        let pending = tokio::spawn(async move {
            service
                .list(
                    &context("recipient"),
                    Some("request-credential"),
                    &AgentProvenanceInventoryQuery {
                        shared: true,
                        ..Default::default()
                    },
                    &PageRequest::default(),
                )
                .await
        });
        entered.notified().await;
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        resume.notify_one();
        let error = pending.await.unwrap().unwrap_err();
        tokio::time::resume();
        assert!(
            matches!(
                error,
                ResourceAccessError::Access(AccessError::RevalidationRequired)
            ),
            "{error}"
        );
    }
}

struct StaleChoices(bool);
#[async_trait::async_trait]
impl OrganizationAuthority for StaleChoices {
    async fn organization_choices(
        &self,
        _: &layer2_core::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![
            OrganizationChoice {
                id: SubjectId::new("org"),
                role: OrganizationRole::Admin,
                display_name: None,
            },
            OrganizationChoice {
                id: SubjectId::new("removed-org"),
                role: OrganizationRole::Admin,
                display_name: None,
            },
        ])
    }
    async fn organization_context(
        &self,
        caller: &layer2_core::Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if org.as_str() == "removed-org" {
            return Err(if self.0 {
                MembershipError::NotMember
            } else {
                MembershipError::Unavailable
            });
        }
        ShortAuthority(OrganizationRole::Viewer)
            .organization_context(caller, credential, org)
            .await
    }
}

#[tokio::test]
async fn shared_provenance_inventory_requires_actual_membership_and_reports_incomplete_discovery() {
    use layer2_core::sandbox::AgentProvenanceInventoryQuery;
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    for removed in [true, false] {
        let service = AgentProvenanceInventoryService::new(
            fixture.store.clone(),
            Arc::new(ResourceAccessService::new(
                fixture.store.clone(),
                Arc::new(StaleChoices(removed)),
            )),
        );
        let result = service
            .list(
                &context("recipient"),
                Some("request-credential"),
                &AgentProvenanceInventoryQuery {
                    shared: true,
                    ..Default::default()
                },
                &PageRequest::default(),
            )
            .await;
        if removed {
            assert_eq!(
                result.unwrap().items[0].agent_id.as_str(),
                "agt_shared_read"
            );
        } else {
            assert!(matches!(
                result,
                Err(ResourceAccessError::Membership(
                    MembershipError::Unavailable
                ))
            ));
        }
    }
}
