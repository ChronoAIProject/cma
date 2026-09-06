use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_writer_claims_preserve_concurrent_log_and_title_updates() {
    use layer2_core::sandbox::{AgentTitle, AgentTitleStore};

    use crate::store::child_writer_claims::{persist, ChildWriterClaim};

    let (client, name) = database("capture_claim_agent_cas").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for kind in ["log", "title"] {
        let (parent, _, mut agent) = capture_fixture(&store, &format!("claim_cas_{kind}")).await;
        let revision = agent.revision;
        agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
        agent
            .work
            .claim("writer".into(), NOW, MINUTE.into())
            .unwrap();
        agent.revision += 1;
        if kind == "log" {
            store
                .append_agent_log(
                    &agent.id,
                    NOW.into(),
                    AgentLogKind::ConversationOpened {
                        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                        runtime_generation: 1,
                    },
                )
                .await
                .unwrap();
        } else {
            store
                .set_user_agent_title(
                    &agent.id,
                    &AgentTitle::new("Updated title").unwrap(),
                    NOW.into(),
                )
                .await
                .unwrap();
        }
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(current.revision, revision + u64::from(kind == "title"));
        assert_eq!(
            persist(&store, revision, ChildWriterClaim::Agent(Box::new(agent))).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(current));
        let stored_parent = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert!(!stored_parent.contains_key("runtime_writer_claim_fence"));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_begin_and_sandbox_writer_claim_have_one_winner() {
    let (client, name) = database("capture_claim_race").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for index in 0..16 {
        let (mut parent, _, _) = capture_fixture(&store, &format!("claim_race_{index}")).await;
        let revision = parent.revision;
        parent.work =
            SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, MINUTE.into());
        parent.revision += 1;
        store.save_sandbox(revision, parent.clone()).await.unwrap();
        let request = begin(&parent);
        let (capture, writer) = tokio::join!(
            store.commit_sandbox_lifecycle(request),
            store.claim_sandbox_work(&parent.id, "writer", MINUTE.into(), TWO_MINUTES.into()),
        );
        match (capture, writer) {
            (Ok(()), Ok(None)) | (Err(SandboxStoreError::Conflict), Ok(Some(_))) => {}
            result => panic!("capture and runtime writer were not exclusive: {result:?}"),
        }
        let stored = store.get_sandbox(&parent.id).await.unwrap().unwrap();
        assert_ne!(
            stored.lifecycle.checkpoint_capture.is_some(),
            stored.work.lease.is_some()
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn child_writer_claims_fence_a_capture_started_from_an_older_parent_snapshot() {
    let (client, name) = database("capture_parent_fence").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for kind in ["agent", "workspace"] {
        let (parent, mut workspace, mut agent) =
            capture_fixture(&store, &format!("fence_{kind}")).await;
        let request = begin(&parent);
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let old = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.get_i64("revision").unwrap(), parent.revision as i64);

        if kind == "agent" {
            let revision = agent.revision;
            agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
            agent.revision += 1;
            store.save_agent(revision, agent.clone()).await.unwrap();
            assert!(store
                .claim_agent_work(&agent.id, "writer", NOW.into(), MINUTE.into())
                .await
                .unwrap()
                .is_some());
        } else {
            let revision = workspace.revision;
            workspace.status = WorkspaceStatus::Removing;
            workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
            workspace.revision += 1;
            store
                .save_workspace(revision, workspace.clone())
                .await
                .unwrap();
            assert!(store
                .claim_workspace_work(&workspace.id, "writer", NOW.into(), MINUTE.into())
                .await
                .unwrap()
                .is_some());
        }
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        let fields = crate::store::sandboxes::sandbox_replacement_fields(&request.sandbox).unwrap();
        let error = store
            .sandboxes
            .update_one(
                doc! { "_id": parent.id.as_str(), "revision": parent.revision as i64 },
                doc! { "$set": fields },
            )
            .session(&mut session)
            .await
            .unwrap_err();
        assert!(is_transient(&error));
        let _ = session.abort_transaction().await;
        assert!(store
            .get_sandbox(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .checkpoint_capture
            .is_none());
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn capture_and_child_writer_claims_reconcile_ambiguous_commits_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-capture-claim-{suffix}");
    let name = format!("cma_capture_claim_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, _, _) = capture_fixture(&store, "ambiguous_begin").await;
    let request = begin(&parent);
    ambiguous_commits(&admin, &app_name).await;
    let result = store.commit_sandbox_lifecycle(request.clone()).await;
    disable_failpoint(&admin).await;
    result.unwrap();
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(request.sandbox)
    );
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        1
    );

    for kind in ["agent", "workspace"] {
        let (parent, mut workspace, mut agent) =
            capture_fixture(&store, &format!("ambiguous_{kind}")).await;
        let claimed = if kind == "agent" {
            let revision = agent.revision;
            agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
            agent.revision += 1;
            store.save_agent(revision, agent.clone()).await.unwrap();
            ambiguous_commits(&admin, &app_name).await;
            store
                .claim_agent_work(&agent.id, "writer", NOW.into(), MINUTE.into())
                .await
                .map(|row| row.map(|row| row.revision))
        } else {
            let revision = workspace.revision;
            workspace.status = WorkspaceStatus::Removing;
            workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
            workspace.revision += 1;
            store
                .save_workspace(revision, workspace.clone())
                .await
                .unwrap();
            ambiguous_commits(&admin, &app_name).await;
            store
                .claim_workspace_work(&workspace.id, "writer", NOW.into(), MINUTE.into())
                .await
                .map(|row| row.map(|row| row.revision))
        };
        disable_failpoint(&admin).await;
        assert_eq!(claimed.unwrap(), Some(3));
        let parent = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parent.get_i64("runtime_writer_claim_fence").unwrap(), 1);
    }
    client.database(&name).drop().await.unwrap();
}

async fn ambiguous_commits(admin: &Client, app_name: &str) {
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous checkpoint writer authority",
                },
            },
        })
        .await
        .unwrap();
}

async fn disable_failpoint(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable failpoint before assertions");
}
