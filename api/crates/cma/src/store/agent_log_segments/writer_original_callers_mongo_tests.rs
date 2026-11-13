//! Real worker/control roots after adoption, including later domain aborts.
use super::*;
use layer2_core::sandbox::{store::*, *};

async fn hierarchy(f: &Fixture) {
    lifecycle_caller::parent(f).await;
    f.store
        .create_worktree(Worktree {
            id: f.agent.worktree_id.clone(),
            workspace_id: f.agent.workspace_id.clone(),
            sandbox_id: f.agent.sandbox_id.clone(),
            owner: f.agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/receipt/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(f.agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
}

async fn claim_fixture(f: &mut Fixture) {
    hierarchy(f).await;
    f.agent
        .work
        .schedule(AgentWorkStep::CloseProcess, NOW.into());
    f.agent.requested_stop_reason = Some(AgentStopReason::Owner);
    f.agent
        .work
        .claim("worker".into(), NOW, "2026-11-11T00:10:00Z".into())
        .unwrap();
    f.agent.revision += 1;
    f.store
        .agents
        .update_one(
            doc! {"_id":f.agent.id.as_str()},
            doc! {"$set":agent_replacement_fields(&f.agent).unwrap()},
        )
        .await
        .unwrap();
}

fn retry(agent: &Agent) -> AgentWorkMutation {
    let fence = agent.work.lease.as_ref().unwrap().fence;
    let mut next = agent.clone();
    next.work
        .retry("worker", fence, "2026-11-11T00:02:00Z".into())
        .unwrap();
    next.updated_at = NOW.into();
    next.revision += 1;
    next.conversation.last_seq += 1;
    AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: "worker".into(),
        fence,
        agent: next,
        worktree: None,
        appended_at: NOW.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::CloseProcess,
            failure: TypedFailure {
                class: "runtime_unavailable".into(),
                detail: "retry".into(),
            },
        }],
    }
}

async fn unknown(admin: &mongodb::Client, app: &str) {
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":app,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected uncertainty"}}
    }).await.unwrap();
}

async fn clear(admin: &mongodb::Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn worker_root_settles_segmented_publication_and_aborts_later_domain_refusal() {
    let mut f = fixture(2).await;
    claim_fixture(&mut f).await;
    let mutation = retry(&f.agent);
    let mut invalid = mutation.clone();
    invalid.agent.title = Some("foreign domain change".into());
    assert!(f.store.commit_agent_work(invalid).await.is_err());
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let (admin, app) = keyed_restart::isolated(&mut f).await;
    unknown(&admin, &app).await;
    let result = f.store.commit_agent_work(mutation.clone()).await;
    clear(&admin).await;
    result.unwrap();
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(mutation.agent)
    );
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn control_root_updates_segmented_head_and_exact_original_receipt() {
    let mut f = fixture(2).await;
    hierarchy(&f).await;
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&f.agent),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let mut foreign = request.clone();
    foreign.target.owner = layer2_core::SubjectId::new("foreign-owner");
    assert!(f.store.schedule_agent_stop(foreign).await.is_err());
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let (admin, app) = keyed_restart::isolated(&mut f).await;
    unknown(&admin, &app).await;
    let result = f.store.schedule_agent_stop(request.clone()).await;
    clear(&admin).await;
    let AgentControlResult::Scheduled(next) = result.unwrap() else {
        panic!("scheduled control")
    };
    assert_eq!(next.requested_stop_reason, Some(AgentStopReason::Owner));
    assert!(matches!(
        f.store.schedule_agent_stop(request).await.unwrap(),
        AgentControlResult::Existing(_)
    ));
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        collection(&f.store, HEADS)
            .find_one(doc! {"_id":f.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get_i64("revision")
            .unwrap(),
        2
    );
    f.destroy().await;
}
