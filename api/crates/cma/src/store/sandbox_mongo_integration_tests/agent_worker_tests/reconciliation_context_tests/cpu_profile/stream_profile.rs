use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::{
    AgentReadService, SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading,
};

pub(super) struct NoExternalCalls;
#[async_trait::async_trait]
impl SandboxResourceReader for NoExternalCalls {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("history reads must not ask the provider for resources")
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for NoExternalCalls {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal authority does not require organization I/O")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal authority does not require organization I/O")
    }
}

pub(super) async fn measure(store: &Arc<MongoSandboxStore>, original: &Agent, meter: &Meter) {
    let mut agent = original.clone();
    store
        .agent_log
        .delete_many(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap();
    let rows: Vec<_> = (1..=5_000)
        .map(|seq| {
            let kind = match seq {
                1 => AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
                2 => AgentLogKind::ConversationOpened {
                    conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                    runtime_generation: 1,
                },
                _ => AgentLogKind::WorkStepFailed {
                    step: AgentWorkStep::RemoveWorktree,
                    failure: TypedFailure {
                        class: "fixture_failure".into(),
                        detail: "synthetic retained history ".repeat(20),
                    },
                },
            };
            crate::store::agent_log::log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq,
                appended_at: NOW.into(),
                kind,
            })
            .unwrap()
        })
        .collect();
    store.agent_log.insert_many(rows).await.unwrap();
    agent.conversation.last_seq = 5_000;
    store
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str()},
            // This synthetic replacement is a legacy/unprepared history; the
            // previous fixture's tail hash cannot describe these new rows.
            doc! {"$set":{"last_seq":5_000_i64,"record.conversation.last_seq":5_000_i64},
            "$unset": {crate::store::agent_log_integrity::INTEGRITY_FIELD: ""}},
        )
        .await
        .unwrap();
    let authority = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(NoExternalCalls),
    ));
    let reads = AgentReadService::new(store.clone(), Arc::new(NoExternalCalls))
        .with_resource_access(authority);
    let context = CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            agent.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://fixture.test".into(),
            subject: agent.owner.clone(),
            credential_id: "cpu-fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: agent.owner.clone(),
        },
        memberships: vec![],
    };
    let authorization = reads
        .authorize_event_stream(&context, None, &agent.id)
        .await
        .unwrap();
    meter
        .phase("open_stream_snapshot_5000_entries", 20, || async {
            assert_eq!(
                reads
                    .event_stream_snapshot(&authorization)
                    .await
                    .unwrap()
                    .head,
                5_000
            );
        })
        .await;
}
