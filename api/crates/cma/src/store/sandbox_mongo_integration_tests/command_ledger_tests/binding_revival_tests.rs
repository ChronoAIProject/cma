use super::*;
use layer2_core::sandbox::store::SandboxTransaction;
use layer3_adapter::{CheckpointReceipt, WriterState};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_command_revives_only_its_parent_and_retirement_fences_the_transaction() {
    for retire in [false, true] {
        let (client, name) = database("binding_command_revival").await;
        let (store, agent, mut parent, execution, _, sends) = composed(&client, &name).await;
        let (binding, mut conversation, _) = binding_for(&store, &agent).await;
        let request = reservation(&store, &agent, &parent, &binding).await;
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
        let proof = auth
            .authorize_command_revival(&access, &command)
            .await
            .unwrap();
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
        client.database(&name).drop().await.unwrap();
    }
}
