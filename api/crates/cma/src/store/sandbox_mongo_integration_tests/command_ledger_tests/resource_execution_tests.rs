use super::super::work_tests::message_mutation;
use super::*;

#[path = "command_revival_tests.rs"]
pub(in crate::store::sandbox_mongo_integration_tests) mod revival;

struct Membership(Duration);

fn context() -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new("actual-admin"),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        actor: ResourceActor::Api {
            subject: SubjectId::new("actual-admin"),
            delegation_id: None,
        },
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new("actual-admin"),
            credential_id: "verified-context".into(),
        },
        memberships: vec![],
    }
}

#[async_trait::async_trait]
impl OrganizationAuthority for Membership {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(caller.subject, context().caller.subject);
        assert_eq!(credential, "fixture-credential");
        let mut context = context();
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            org.clone(),
            OrganizationRole::Admin,
            now,
            now + self.0,
        )?);
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("inventory is not execution authority")
    }
}

async fn grant(store: &MongoSandboxStore, parent: &Sandbox) -> ResourceShare {
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Sandbox {
            sandbox_id: parent.id.clone(),
        })
        .await
        .unwrap();
    store
        .mutate_share(super::super::super::resource_access_tests::mutation(
            &snapshot,
            "execute",
            ShareMode::OrgRoles,
        ))
        .await
        .unwrap()
        .share
}

async fn admit_shared(
    store: &MongoSandboxStore,
    agent: &Agent,
    parent: &Sandbox,
) -> AdmittedAgentCommand {
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })
        .await
        .unwrap();
    let mut request = live_request(agent, parent, "shared");
    request.envelope.actor = context().actor;
    request.envelope.owner = snapshot
        .scope(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })
        .unwrap()
        .owner;
    request.policy = snapshot.fence();
    store.reserve_command(request).await.unwrap();
    store
        .claim_next_command(claim_request(agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap()
}

async fn proof(
    store: Arc<MongoSandboxStore>,
    command: &AdmittedAgentCommand,
    lifetime: Duration,
) -> CommandExecutionAuthorization {
    ResourceAccessService::new(store, Arc::new(Membership(lifetime)))
        .authorize_command_execution(&context(), "fixture-credential", &command.receipt.envelope)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resource_execution_shared_admin_preserves_owner_receipt_and_conversation() {
    let (client, name) = database("command_shared_execution").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    grant(&store, &parent).await;
    let command = admit_shared(&store, &agent, &parent).await;
    let proof = proof(store.clone(), &command, MAX_MEMBERSHIP_AGE).await;
    execution
        .operations
        .execute_resource_claim(&proof, &command)
        .await
        .unwrap();
    let retained = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.receipt, command.receipt);
    assert!(retained.work.dispatch.is_some());
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(current.owner, agent.owner);
    assert_eq!(current.profile, agent.profile);
    assert_eq!(
        current.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert!(execution
        .operations
        .execute_resource_claim(&proof, &command)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let audits = store
        .command_authorization_audits(
            &command.receipt.envelope.command_id,
            &PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    let audit = &audits.items[0];
    assert_eq!(audit.actor, context().actor);
    assert_eq!(audit.owner, command.receipt.envelope.owner);
    assert!(
        matches!(&audit.effect, CommandAuthorizationEffect::Dispatch { dispatch } if Some(dispatch) == retained.work.dispatch.as_ref())
    );
    assert!(matches!(
        &audit.resources[0].observations[0].basis,
        ResourceAuthorizationBasis::Organization {
            role: OrganizationRole::Admin,
            share: Some(_),
            ..
        }
    ));
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .command_authorization_audits(
                &command.receipt.envelope.command_id,
                &PageRequest::new(20, None).unwrap()
            )
            .await
            .unwrap(),
        audits
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resource_execution_transaction_fences_policy_ancestry_and_exact_command() {
    for change in ["policy", "ancestry", "command"] {
        let (client, name) = database("command_shared_fence").await;
        let (store, agent, parent, _, _, sends) = composed(&client, &name).await;
        grant(&store, &parent).await;
        let command = admit_shared(&store, &agent, &parent).await;
        let proof = proof(store.clone(), &command, MAX_MEMBERSHIP_AGE).await;
        let mut mutation =
            message_mutation(&agent, &parent, &command.work.lease.as_ref().unwrap().claim);
        mutation.execution_fence = Some(proof.fence());
        match change {
            "policy" => {
                store
                    .resource_policies
                    .update_one(
                        doc! { "_id": parent.id.as_str() },
                        doc! { "$inc": { "record.revision": 1_i64, "revision": 1_i64 } },
                    )
                    .await
                    .unwrap();
            }
            "ancestry" => {
                store
                    .workspaces
                    .update_one(
                        doc! { "_id": agent.workspace_id.as_str() },
                        doc! { "$set": { "record.created_at": "2026-09-06T00:00:00Z" } },
                    )
                    .await
                    .unwrap();
            }
            "command" => {
                let mut altered = command.receipt.envelope.clone();
                altered.correlation_id = "different-execution".into();
                // Only the independently issued proof changes. The mutation and
                // durable command remain valid, proving the atomic identity fence.
                let altered_proof = ResourceAccessService::new(
                    store.clone(),
                    Arc::new(Membership(MAX_MEMBERSHIP_AGE)),
                )
                .authorize_command_execution(&context(), "fixture-credential", &altered)
                .await
                .unwrap();
                mutation.execution_fence = Some(altered_proof.fence());
            }
            _ => unreachable!(),
        }
        assert!(
            store.commit_agent_conversation(mutation).await.is_err(),
            "{change}"
        );
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command)
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .command_authorization_audits
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn resource_execution_expired_proof_cannot_bind_a_claim() {
    let (client, name) = database("command_shared_expired").await;
    let (store, agent, parent, _, _, sends) = composed(&client, &name).await;
    grant(&store, &parent).await;
    let command = admit_shared(&store, &agent, &parent).await;
    let proof = proof(store.clone(), &command, Duration::from_millis(300)).await;
    let mut mutation =
        message_mutation(&agent, &parent, &command.work.lease.as_ref().unwrap().claim);
    mutation.execution_fence = Some(proof.fence());
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(store.commit_agent_conversation(mutation).await.is_err());
    assert_eq!(
        store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(command)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn resource_execution_expiry_before_commit_rolls_back_parent_command_and_logs() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_execution_expiry_{suffix}");
    let app_name = format!("cma-execution-expiry-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let (store, agent, parent, _, _, sends) = composed(&client, &name).await;
    grant(&store, &parent).await;
    let command = admit_shared(&store, &agent, &parent).await;
    let before_parent = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap();
    let before_log = store.agent_log(&agent.id, 0, 50).await.unwrap();
    let proof = proof(store.clone(), &command, Duration::from_millis(500)).await;
    let mut mutation =
        message_mutation(&agent, &parent, &command.work.lease.as_ref().unwrap().claim);
    mutation.execution_fence = Some(proof.fence());
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
        "data": { "failCommands": ["insert"], "appName": &app_name, "blockConnection": true, "blockTimeMS": 1000_i32 },
    }).await.unwrap();
    let result = store.commit_agent_conversation(mutation).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable fixture failpoint before assertions");
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(command)
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap(),
        before_parent
    );
    assert_eq!(store.agent_log(&agent.id, 0, 50).await.unwrap(), before_log);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
