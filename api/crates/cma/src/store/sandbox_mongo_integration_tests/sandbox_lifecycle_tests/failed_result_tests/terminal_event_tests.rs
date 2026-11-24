use super::*;

pub(super) async fn assert_public_terminal(app: &Router, failed: &Sandbox) {
    let detail = call(app, &failed.id, Method::GET, "owner", StatusCode::OK).await;
    assert_eq!(detail["status"], "failed");
    let path = format!("/api/v1/sandboxes/{}/events", failed.id);
    let full = call_path(app, &path, Method::GET, "owner", StatusCode::OK).await;
    let events = full.as_array().unwrap();
    let mut replayed = None;
    let mut terminal_count = 0;
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["seq"], index as u64 + 1);
        match event["kind"]["type"].as_str().unwrap() {
            "status_changed" => replayed = Some(event["kind"]["to"].clone()),
            "failed" => {
                replayed = Some(serde_json::json!("failed"));
                terminal_count += 1;
                assert_eq!(
                    event["kind"]["failure"],
                    serde_json::to_value(&failed.failure).unwrap()
                );
                for field in ["class", "detail"] {
                    assert_eq!(event["kind"]["failure"][field], detail["failure"][field]);
                }
                assert_eq!(event["appended_at"], failed.updated_at);
            }
            _ => {}
        }
    }
    assert_eq!(terminal_count, 1);
    assert_eq!(replayed.as_ref(), Some(&detail["status"]));
    let mut pages = Vec::new();
    let mut after = 0;
    loop {
        let page = call_path(
            app,
            &format!("{path}?after={after}&limit=1"),
            Method::GET,
            "owner",
            StatusCode::OK,
        )
        .await;
        let page = page.as_array().unwrap();
        if page.is_empty() {
            break;
        }
        assert_eq!(page.len(), 1);
        after = page[0]["seq"].as_u64().unwrap();
        pages.extend(page.clone());
    }
    assert_eq!(&pages, events);
    call_path(app, &path, Method::GET, "other", StatusCode::NOT_FOUND).await;
}

fn failed_successor(claimed: &Sandbox) -> Sandbox {
    let mut failed = claimed.clone();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    if claimed.lifecycle.operation.is_some() {
        failed
            .fail_lifecycle(
                "fixture terminal failure".into(),
                "worker",
                fence,
                TWO_MINUTES.into(),
            )
            .unwrap();
    } else {
        failed
            .work
            .advance("worker", fence, SandboxWorkStep::None, TWO_MINUTES.into())
            .unwrap();
        failed
            .transition(
                SandboxStatus::Failed,
                TWO_MINUTES.into(),
                Some(TypedFailure {
                    class: "terminal_validation".into(),
                    detail: "fixture terminal failure".into(),
                }),
            )
            .unwrap();
    }
    failed
}

async fn commit_failure(
    store: &MongoSandboxStore,
    claimed: &Sandbox,
    failed: &Sandbox,
) -> Result<(), SandboxStoreError> {
    let event = SandboxEventKind::Failed {
        failure: failed.failure.clone().unwrap(),
    };
    if claimed.lifecycle.operation.is_some() {
        store
            .commit_sandbox_lifecycle(SandboxLifecycleMutation {
                guard: SandboxMutationGuard::capture(claimed),
                sandbox: failed.clone(),
                agents: vec![],
                agent_scope: SandboxLifecycleAgentScope::Touched,
                appended_at: TWO_MINUTES.into(),
                events: vec![event],
            })
            .await
    } else {
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: claimed.revision,
                sandbox: Box::new(failed.clone()),
                event_at: TWO_MINUTES.into(),
                event,
            })
            .await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn terminal_failure_is_atomic_replayable_and_not_duplicated_after_ambiguous_commit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    for lifecycle in [false, true] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app_name = format!("cma-terminal-event-{suffix}");
        let name = format!("cma_2536_terminal_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut original = sandbox("sbx_terminal_event");
        original.status = SandboxStatus::Installing;
        original.work = SandboxWorkState::new(SandboxWorkStep::RunInstall, MINUTE.into());
        if lifecycle {
            original.status = SandboxStatus::Active;
            original.work.step = SandboxWorkStep::None;
        }
        store.create_sandbox(original.clone()).await.unwrap();
        if lifecycle {
            store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                    original.id.clone(),
                    original.owner.clone(),
                    MINUTE.into(),
                    None,
                ))
                .await
                .unwrap();
        }
        let claimed = store
            .claim_sandbox_work(
                &original.id,
                "worker",
                TWO_MINUTES.into(),
                work_lease_deadline(),
            )
            .await
            .unwrap()
            .unwrap();
        let failed = failed_successor(&claimed);
        let before_events = store.sandbox_events(&original.id, 0, 200).await.unwrap();
        let mut stale = claimed.clone();
        stale.revision -= 1;
        assert!(commit_failure(&store, &stale, &failed_successor(&stale))
            .await
            .is_err());
        assert_eq!(
            store.get_sandbox(&original.id).await.unwrap(),
            Some(claimed.clone())
        );
        assert_eq!(
            store.sandbox_events(&original.id, 0, 200).await.unwrap(),
            before_events
        );

        admin.database("admin").run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
            "data": { "failCommands": ["commitTransaction"], "appName": &app_name,
                "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous terminal commit" } },
        }).await.unwrap();
        let result = commit_failure(&store, &claimed, &failed).await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .unwrap();
        result.unwrap();
        let events = store.sandbox_events(&original.id, 0, 200).await.unwrap();
        assert_eq!(events.len(), before_events.len() + 1);
        let retry = commit_failure(&store, &claimed, &failed).await;
        assert!(matches!(retry, Ok(()) | Err(SandboxStoreError::Conflict)));
        assert_eq!(
            store.sandbox_events(&original.id, 0, 200).await.unwrap(),
            events
        );
        let fresh = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        assert_eq!(
            fresh.get_sandbox(&failed.id).await.unwrap(),
            Some(failed.clone())
        );
        assert_public_terminal(&router(fresh), &failed).await;
        client.database(&name).drop().await.unwrap();
    }
}
