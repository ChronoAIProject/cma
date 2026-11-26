use super::*;
use crate::store::sandbox_mongo_integration_tests::sandbox;
use layer2_core::sandbox::{SandboxStatus, SandboxWorkState};

async fn terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once_case(
    scoped: bool,
    fault: bool,
) {
    let f = Fixture::new().await;
    let mut parent = sandbox("sbx_purge_fence");
    parent.status = SandboxStatus::Retired;
    parent.credential.delivered = false;
    parent.credential.current = None;
    parent.work = SandboxWorkState::new(SandboxWorkStep::Purge, MINUTE.into());
    parent.retired_at = Some(MINUTE.into());
    parent.purge_after = Some(MINUTE.into());
    f.independent.create_sandbox(parent.clone()).await.unwrap();
    f.independent
        .workspaces
        .insert_one(doc! {"_id":"wks_purge", "sandbox_id":parent.id.as_str()})
        .await
        .unwrap();
    f.independent
        .checkpoints
        .insert_one(doc! {"_id":"checkpoint_purge", "sandbox_id":parent.id.as_str()})
        .await
        .unwrap();
    let claimed = f
        .independent
        .claim_sandbox_work(
            &parent.id,
            "purge-worker",
            MINUTE.into(),
            work_lease_deadline(),
        )
        .await
        .unwrap()
        .unwrap();
    let before = hierarchy(&f).await;
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.reset_trace();
            if fault {
                f.fail("update", 1, false).await;
            }
            let work = f.store.purge_sandbox(&claimed.id, claimed.revision, MINUTE);
            if scoped {
                crate::store::work_publication::scope(&f.store, work).await
            } else {
                work.await
            }
        },
        || async {
            f.independent
                .renew_sandbox_authority(&claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await;
    f.clear().await;
    let starts = f.starts();
    let events = f.events.lock().unwrap().clone();
    let failures = events
        .iter()
        .filter_map(|event| match event {
            mongodb::event::command::CommandEvent::Failed(event) => Some(event),
            _ => None,
        })
        .collect::<Vec<_>>();
    let known_aborts = failures.iter().filter(|event| {
        event.command_name == "update"
            && matches!(event.failure.kind.as_ref(), mongodb::error::ErrorKind::Command(e) if e.code == 112)
            && event.failure.contains_label(mongodb::error::TRANSIENT_TRANSACTION_ERROR)
            && !event.failure.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
    }).count();
    let commits = events.iter().filter(|event| matches!(event,
        mongodb::event::command::CommandEvent::Started(event) if event.command_name == "commitTransaction"
    )).count();
    let after = hierarchy(&f).await;
    let hierarchy_unchanged = before == after;
    eprintln!("purge_evidence scoped={scoped} fault={fault} known_abort_hits={known_aborts} starts={} commits={commits} conflict={} hierarchy_unchanged={hierarchy_unchanged}",
        starts.len(), matches!(result, Err(SandboxStoreError::Conflict)));
    let stored = f.independent.get_sandbox(&parent.id).await.unwrap();
    let workspaces = f
        .independent
        .workspaces
        .count_documents(doc! {"sandbox_id":parent.id.as_str()})
        .await
        .unwrap();
    let checkpoints = f
        .independent
        .checkpoints
        .count_documents(doc! {"sandbox_id":parent.id.as_str()})
        .await
        .unwrap();
    f.drop_database().await;
    if fault {
        assert_eq!(
            known_aborts, 1,
            "the actual work fence must return one typed known abort"
        );
    } else {
        assert!(failures.is_empty());
    }
    result.expect("purge must retry known-aborted authority writes, not report lost custody");
    assert_fresh_transactions(&starts, if fault { 2 } else { 1 });
    assert!(stored.is_none());
    assert_eq!((workspaces, checkpoints), (0, 0));
}

async fn hierarchy(f: &Fixture) -> Vec<Vec<Document>> {
    use futures_util::TryStreamExt;
    let mut state = Vec::new();
    for rows in [
        &f.independent.sandboxes,
        &f.independent.workspaces,
        &f.independent.checkpoints,
        &f.independent.agents,
        &f.independent.worktrees,
    ] {
        state.push(
            rows.find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    state
}

#[path = "purge_cases.rs"]
mod cases;

#[path = "purge/contract.rs"]
mod contract;
