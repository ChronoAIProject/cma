use super::tests::Fixture;
use super::*;
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot, WorktreeRef,
    WorktreeRequest,
};
use serde_json::{json, Value};

fn lifecycle(config: &RuntimeConfig, name: &str) -> Vec<(Operation, Value)> {
    let agent = AgentSlot::new(name);
    let cwd = crate::worktree::expected_path(&config.home, "workspace", name)
        .to_string_lossy()
        .into_owned();
    let home = config.agent_home(name).to_string_lossy().into_owned();
    let add = Operation::add_worktree(&WorktreeRequest::new(
        format!("worktree:workspace:{name}:add:v1"),
        "workspace",
        name,
        "a".repeat(40),
    ));
    let create = Operation::create_agent_home(&AgentHomePlan {
        operation_id: format!("agent:{name}:home:create:v1"),
        request_sha256: "a".repeat(64),
        agent_id: agent.clone(),
        files: Vec::new(),
        symlinks: Vec::new(),
        skills: Vec::new(),
    });
    let open = Operation::open_agent(&AgentLaunch {
        operation_id: format!("agent:{name}:open:v1"),
        request_sha256: "b".repeat(64),
        agent_id: agent.clone(),
        cwd: cwd.clone(),
        home: home.clone(),
        program: "synthetic".into(),
        args: Vec::new(),
        env: Default::default(),
    });
    let close = Operation::close_agent(&AgentCloseRequest::new(
        format!("agent:{name}:close:v1"),
        agent.clone(),
    ));
    let remove_home = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        format!("agent:{name}:home:remove:v1"),
        agent,
    ));
    let remove = Operation::remove_worktree(&WorktreeRef::new(
        format!("worktree:workspace:{name}:remove:v1"),
        "workspace",
        name,
    ));
    [add, create, open, close, remove_home, remove]
        .into_iter()
        .enumerate()
        .map(|(i, op)| {
            let mut value =
                json!({"operation_id":op.operation_id,"request_sha256":op.request_sha256});
            if i == 0 || i == 5 {
                value["workspace"] = json!("workspace");
                value["name"] = json!(name);
                if i == 0 {
                    value["path"] = json!(cwd);
                    value["branch"] = json!(format!("cma/{name}"));
                    value["base_commit"] = json!("a".repeat(40));
                }
            } else {
                value["agent_id"] = json!(name);
                if i == 1 {
                    value["path"] = json!(home);
                }
                if i == 2 {
                    value["cwd"] = json!(cwd);
                    value["home"] = json!(home);
                }
            }
            (op, value)
        })
        .collect()
}

#[tokio::test]
async fn thousand_complete_agent_lifecycles_preserve_receipts_and_tombstones() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for i in 0..1000 {
        let name = format!("agent-{i}");
        let cycle = lifecycle(&fixture.config, &name);
        // Both preparation admissions precede either effect.
        ledger
            .admit(1, vec![cycle[0].0.clone(), cycle[1].0.clone()])
            .await
            .unwrap();
        for (index, (operation, receipt)) in cycle.iter().enumerate() {
            if index >= 2 {
                ledger
                    .admit(index as u64, vec![operation.clone()])
                    .await
                    .unwrap();
            }
            let started = ledger.begin(operation.clone()).await.unwrap();
            assert!(started.new);
            ledger
                .finish(started.record.completed(receipt).unwrap())
                .await
                .unwrap();
            if index == 2 {
                assert!(ledger.active(ProcessFilter::Any).await.unwrap());
                assert!(ledger
                    .active(ProcessFilter::Agent(name.clone()))
                    .await
                    .unwrap());
                let cwd = crate::worktree::expected_path(&fixture.config.home, "workspace", &name)
                    .to_string_lossy()
                    .into_owned();
                assert!(ledger.active(ProcessFilter::Worktree(cwd)).await.unwrap());
                assert!(ledger
                    .active(ProcessFilter::Workspace(
                        fixture
                            .config
                            .home
                            .join("workspace")
                            .to_string_lossy()
                            .into()
                    ))
                    .await
                    .unwrap());
                assert!(!ledger
                    .active(ProcessFilter::Workspace(
                        fixture.config.home.join("work").to_string_lossy().into()
                    ))
                    .await
                    .unwrap());
            }
        }
        assert!(!ledger.active(ProcessFilter::Any).await.unwrap());
    }
    let snapshot = ledger.snapshot().await.unwrap();
    assert_eq!(
        (
            snapshot.operations,
            snapshot.terminal,
            snapshot.pending,
            snapshot.processes,
            snapshot.reserved_bytes
        ),
        (6000, 6000, 0, 0, 0)
    );
    assert!(ledger
        .started_page(0, String::new())
        .await
        .unwrap()
        .is_empty());
    let first = lifecycle(&fixture.config, "agent-0");
    for (op, expected) in first.iter().skip(3) {
        ledger.admit(u64::MAX, vec![op.clone()]).await.unwrap();
        let replay = ledger.begin(op.clone()).await.unwrap();
        assert!(!replay.new);
        assert_eq!(replay.record.receipt::<Value>().unwrap(), *expected);
    }
    for (op, _) in first.iter().take(3) {
        assert_eq!(
            ledger
                .admit(u64::MAX, vec![op.clone()])
                .await
                .unwrap_err()
                .code,
            "mutation_authority_retired"
        );
    }
    assert!(ledger
        .active(ProcessFilter::Agent("a".repeat(65537)))
        .await
        .is_err());
    ledger.close().await.unwrap();
}
