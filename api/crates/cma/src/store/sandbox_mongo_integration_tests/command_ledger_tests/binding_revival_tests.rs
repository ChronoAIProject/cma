use super::*;
use layer2_core::sandbox::store::SandboxTransaction;
use layer3_adapter::{CheckpointReceipt, WriterState};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_command_revives_only_its_parent_and_retirement_fences_the_transaction() {
    binding_revival_with_shares(0).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_revival_with_one_agent_view_share() {
    binding_revival_with_shares(1).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_revival_with_agent_and_sandbox_view_shares() {
    binding_revival_with_shares(2).await;
}

async fn grant_view_share(
    store: &MongoSandboxStore,
    agent: &Agent,
    selector: ResourceSelector,
    id: &str,
) -> ResourceShare {
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    store
        .mutate_share(ShareMutation {
            selector,
            owner: snapshot.scope(&target).unwrap().owner,
            actor: ResourceActor::Human {
                subject: agent.owner.clone(),
            },
            key_sha256: command_key_digest(id).unwrap(),
            change: ShareChange::Create {
                target_org_id: SubjectId::new("viewers"),
                mode: ShareMode::View,
            },
            expected_share_revision: None,
            policy_fence: snapshot.fence(),
            proposed_id: id.into(),
            at: layer2_core::now_rfc3339(),
        })
        .await
        .unwrap()
        .share
}

async fn binding_revival_with_shares(share_count: usize) {
    for retire in [false, true] {
        let (client, name) = database("binding_command_revival").await;
        let (store, agent, mut parent, execution, _, sends) = composed(&client, &name).await;
        let (binding, mut conversation, _) = binding_for(&store, &agent).await;
        let target = ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        };
        let mut shares = Vec::new();
        for (index, selector) in [
            ResourceSelector::Agent {
                agent_id: agent.id.clone(),
            },
            ResourceSelector::Sandbox {
                sandbox_id: parent.id.clone(),
            },
        ]
        .into_iter()
        .take(share_count)
        .enumerate()
        {
            shares
                .push(grant_view_share(&store, &agent, selector, &format!("share-{index}")).await);
        }
        let snapshot = store.resource_access_snapshot(&target).await.unwrap();
        snapshot.scope(&target).unwrap();
        assert_eq!(snapshot.shares, shares);
        let mut request = reservation(&store, &agent, &parent, &binding).await;
        request.policy = snapshot.fence();
        let command = record(store.reserve_command(request).await.unwrap());
        let revision = parent.revision;
        parent.status = SandboxStatus::Paused;
        parent.runtime = None;
        parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
        parent.lifecycle.checkpoints.latest = Some(
            SandboxCheckpointRef::seal(
                &CheckpointReceipt {
                    epoch: 1,
                    object_reference: "checkpoints/binding-revival".into(),
                    sha256: "a".repeat(64),
                    writers: WriterState::Resumed,
                },
                &[],
                NOW,
            )
            .unwrap(),
        );
        parent.revision += 1;
        store.save_sandbox(revision, parent.clone()).await.unwrap();
        let auth = auth(store.clone());
        let access = ResourceAccessService::new(store.clone(), auth.clone())
            .with_bindings(BindingAccessService::new(store.clone()));
        let execution_proof = auth
            .authorize_resource_command(&access, &command.receipt.envelope)
            .await
            .unwrap();
        execution_proof.current_snapshot().await.unwrap();
        let proof = auth
            .authorize_command_revival(&access, &command)
            .await
            .unwrap();
        let audits = proof.authorization_audits().unwrap();
        assert_eq!(audits.len(), 2);
        assert_eq!(audits[0].target, target);
        assert_eq!(
            audits[1].target,
            ResourceTarget::Sandbox {
                sandbox_id: parent.id.clone()
            }
        );
        for audit in audits {
            audit.validate().unwrap();
            assert_eq!(audit.policy_revision, snapshot.fence().policy_revision);
        }
        if retire {
            conversation.state = BindingConversationState::Retiring;
            conversation.revision += 1;
            store
                .save_binding_conversation(binding.revision, Some(2), conversation)
                .await
                .unwrap();
            let mut next = parent.clone();
            let at = layer2_core::now_rfc3339();
            next.begin_revive(SandboxReviveCause::AgentMessage, at.clone())
                .unwrap();
            let transaction = SandboxTransaction::SaveSandboxAndAppend {
                command_revival: Some(proof),
                released_runtime: None,
                expected_revision: parent.revision,
                sandbox: Box::new(next),
                event_at: at,
                event: SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Paused,
                    to: SandboxStatus::Reviving,
                },
            };
            assert!(store.commit(transaction).await.is_err());
            assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
        } else {
            execution
                .context
                .prepare_command_input(proof)
                .await
                .unwrap();
            assert_eq!(
                store.get_sandbox(&parent.id).await.unwrap().unwrap().status,
                SandboxStatus::Reviving
            );
        }
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command)
        );
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        let after = store.resource_access_snapshot(&target).await.unwrap();
        assert_eq!(after.shares, shares);
        assert_eq!(after.policy, snapshot.policy);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_revival_refuses_foreign_or_malformed_original_shares() {
    let (client, name) = database("binding_revival_invalid_share").await;
    let (store, agent, parent, _, _, sends) = composed(&client, &name).await;
    let (binding, _, _) = binding_for(&store, &agent).await;
    let share = grant_view_share(
        &store,
        &agent,
        ResourceSelector::Agent {
            agent_id: agent.id.clone(),
        },
        "agent-view",
    )
    .await;
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let mut request = reservation(&store, &agent, &parent, &binding).await;
    request.policy = snapshot.fence();
    let command = record(store.reserve_command(request).await.unwrap());
    let auth = auth(store.clone());
    let bindings = BindingAccessService::new(store.clone());
    let principal = bindings
        .authenticate(&binding.key_sha256)
        .await
        .unwrap()
        .unwrap();
    let context = context(&principal);
    let access = ResourceAccessService::new(store.clone(), auth).with_bindings(bindings);
    let original = store
        .resource_shares
        .find_one(doc! {"_id": &share.id})
        .await
        .unwrap()
        .unwrap();
    // Retain the query selector so malformed source records must be validated
    // before the binding parent projection can discard any Agent shares.
    for update in [
        doc! {"record.selector": {"kind": "sandbox", "sandbox_id": "foreign-sandbox"}},
        doc! {"record.selector": {"kind": "agent", "agent_id": "foreign-agent"}},
        doc! {"record.revision": 0, "revision": 0},
    ] {
        store
            .resource_shares
            .update_one(doc! {"_id": &share.id}, doc! {"$set": update})
            .await
            .unwrap();
        assert!(matches!(
            access
                .authorize_command_revival(&context, "", &command)
                .await,
            Err(ResourceAccessError::Store(
                ResourceAccessStoreError::Corrupt
            ))
        ));
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command.clone())
        );
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        store
            .resource_shares
            .replace_one(doc! {"_id": &share.id}, original.clone())
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}
