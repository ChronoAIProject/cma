use super::*;
use base64::Engine as _;
use layer3_adapter::hosted::HostedAccessToolSource;
use layer3_adapter::{AgentOpenRequest, RuntimeHandle};
use std::sync::Arc;

fn open(agent: &Agent, parent: &Sandbox, tree: &Worktree) -> AgentOpenRequest {
    AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(agent, parent, tree),
        None,
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn ordinary_agent_in_prepared_sandbox_enrolls_own_access_tools_without_borrowing_session() {
    for same_workspace in [false, true] {
        let (client, name, store) = database().await;
        let (mut original, parent, prepared_workspace, prepared_tree) =
            open_ready_fixture::hierarchy(&store).await;
        let expiry = layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        );
        original.work.lease.as_mut().unwrap().expires_at = expiry.clone();
        original.revision += 1;
        store
            .save_agent(original.revision - 1, original.clone())
            .await
            .unwrap();
        let workspace = if same_workspace {
            prepared_workspace
        } else {
            let workspace = crate::store::agent_access_fixtures::workspace(&parent, "additional");
            store.create_workspace(workspace.clone()).await.unwrap();
            workspace
        };
        let mut input = crate::store::agent_access_fixtures::admission(&workspace, "additional");
        input.origin = AgentOrigin::Lifecycle {
            operation_id: "additional-lifecycle".into(),
        };
        input.creation_key = format!("lifecycle:{}", content_digest(&input.origin).unwrap());
        let mut child = store
            .admit_agent(input)
            .await
            .unwrap()
            .creation()
            .agent
            .clone();
        assert_ne!(child.id, original.id);
        assert_ne!(child.worktree_id, original.worktree_id);
        assert!(child.work.warm_conversation_id.is_none());
        child.work.step = AgentWorkStep::OpenConversation;
        child.work.next_fence = 2;
        child.work.lease = Some(AgentWorkLease {
            holder: "additional-worker".into(),
            fence: 1,
            expires_at: expiry,
        });
        child.revision += 1;
        store
            .save_agent(child.revision - 1, child.clone())
            .await
            .unwrap();
        let snapshot = store.agent_provenance_snapshot(&child.id).await.unwrap();
        let child_open = open(
            &child,
            &parent,
            snapshot.resource.lineage.worktree.as_ref().unwrap(),
        );
        let original_open = open(&original, &parent, &prepared_tree);
        let runtime = RuntimeHandle(parent.runtime.as_ref().unwrap().runtime_id.clone());
        let encryption = crate::store::AuthEncryptionKey::from_base64(
            &base64::engine::general_purpose::STANDARD.encode([9; 32]),
        )
        .unwrap();
        let store = Arc::new(store);
        let source = crate::store::agent_access_tools::MongoAgentAccessToolSource::new(
            store.clone(),
            "https://control.example.test",
            &encryption,
        )
        .unwrap();
        let original_binding = source
            .resolve(
                &runtime,
                &original_open,
                original.work.lease.as_ref().unwrap().fence,
            )
            .await
            .unwrap()
            .unwrap();
        source
            .confirm_open(&original_binding, "prepared-session")
            .await
            .unwrap();
        let before = store.get_agent(&original.id).await.unwrap().unwrap();
        let before_parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
        let original_events = store
            .agent_log
            .count_documents(doc! {"agent_id": original.id.as_str()})
            .await
            .unwrap();

        // This fails before the fix: the parent's preparation receipt is wrongly
        // required to identify this additional, ordinarily admitted Agent.
        let binding = source
            .resolve(&runtime, &child_open, 1)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(binding, original_binding);
        assert_eq!(binding.scope().1 .0, child.id.as_str());
        let forged_prepared = binding
            .clone()
            .with_prepared_conversation("prepared-session")
            .unwrap();
        assert_ne!(
            binding, forged_prepared,
            "ordinary enrollment carries no prepared-session authority"
        );
        assert!(source
            .confirm_open(&forged_prepared, "prepared-session")
            .await
            .is_err());
        source
            .confirm_open(&binding, "additional-session")
            .await
            .unwrap();
        source
            .confirm_open(&binding, "additional-session")
            .await
            .unwrap();
        assert!(source
            .confirm_open(&binding, "prepared-session")
            .await
            .is_err());
        assert_eq!(
            store.get_agent(&original.id).await.unwrap().unwrap(),
            before
        );
        assert_eq!(
            store
                .agent_log
                .count_documents(doc! {"agent_id": original.id.as_str()})
                .await
                .unwrap(),
            original_events
        );
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap().unwrap(),
            before_parent
        );

        for borrowed in ["session", "worktree", "origin"] {
            let mut changed = snapshot.clone();
            let agent = changed.resource.lineage.agent.as_mut().unwrap();
            match borrowed {
                "session" => agent.work.warm_conversation_id = Some("prepared-session".into()),
                "worktree" => agent.worktree_id = original.worktree_id.clone(),
                _ => agent.origin = original.origin.clone(),
            }
            let mut session = client.start_session().await.unwrap();
            assert!(
                super::super::prepared_conversation(&store, &changed, &child_open, &mut session)
                    .await
                    .is_err(),
                "borrowed {borrowed}"
            );
        }
        let mut changed = store.agent_provenance_snapshot(&original.id).await.unwrap();
        changed
            .resource
            .lineage
            .agent
            .as_mut()
            .unwrap()
            .work
            .warm_conversation_id = None;
        let mut session = client.start_session().await.unwrap();
        assert!(
            super::super::prepared_conversation(&store, &changed, &original_open, &mut session)
                .await
                .is_err(),
            "original prepared Agent cannot lose its warm receipt"
        );
        client.database(&name).drop().await.unwrap();
    }
}
