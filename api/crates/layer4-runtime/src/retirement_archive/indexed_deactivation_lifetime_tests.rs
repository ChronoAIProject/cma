use super::*;

async fn select(fixture: &Fixture, key: String, worktree: bool) -> (GroupProof, Vec<u8>) {
    let source = fixture.active(false).await;
    let mut cursor = None;
    let mut context = None;
    let mut rows = Vec::new();
    loop {
        let page = source.retirement_inventory(cursor).await.unwrap();
        context.get_or_insert(page.context);
        rows.extend(
            page.rows
                .into_iter()
                .filter(|row| row.group().unwrap().as_deref() == Some(key.as_str())),
        );
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    source.close().await.unwrap();
    let context = context.unwrap();
    let mut proof = proof(&context, &rows, key);
    let mut observation: RuntimeLedgerRetirementSnapshot =
        serde_json::from_slice(&fixture.observation(&proof.key)).unwrap();
    if worktree {
        observation.worktrees_observed = 1;
        observation.removed_worktrees = std::mem::take(&mut observation.deleted_agents);
        observation.agents_observed = 0;
    }
    let observation = serde_json::to_vec(&observation).unwrap();
    proof.observation_sha256 = format!("{:x}", Sha256::digest(&observation));
    let archive = fixture.indexed(false).await;
    archive
        .prepare_indexed(proof.clone(), context, move |index| {
            Ok(rows[index as usize].clone())
        })
        .await
        .unwrap();
    archive.close().await.unwrap();
    (proof, observation)
}

#[tokio::test]
async fn indexed_deactivation_reuses_active_pages_across_both_legacy_lifetime_bounds() {
    let fixture = Fixture::new();
    fixture.active(true).await.close().await.unwrap();
    fixture.indexed(true).await.close().await.unwrap();
    let mut maximum_pages = 0;
    let mut first_receipt = None;
    for i in 0..1400 {
        let name = format!("cycle-{i}");
        let source = fixture.active(false).await;
        fixture.cycle(&source, &name, 6).await;
        let snapshot = source.snapshot().await.unwrap();
        assert_eq!(
            (
                snapshot.operations,
                snapshot.authority,
                snapshot.terminal,
                snapshot.reserved_bytes
            ),
            (6, 3, 6, 0)
        );
        maximum_pages = maximum_pages.max(snapshot.allocated_bytes);
        source.close().await.unwrap();
        let operations = lifecycle(&fixture.config, &name);
        for (index, worktree) in [(1, false), (0, true)] {
            let (proof, observation) =
                select(&fixture, group_key(&operations[index].0), worktree).await;
            let receipt = fixture
                .apply(proof.clone(), observation.clone())
                .await
                .unwrap();
            if i == 0 && !worktree {
                first_receipt = Some((proof, observation, receipt));
            }
        }
        let source = fixture.active(false).await;
        let snapshot = source.snapshot().await.unwrap();
        assert_eq!(
            (
                snapshot.operations,
                snapshot.authority,
                snapshot.terminal,
                snapshot.reserved_bytes,
                snapshot.processes,
                snapshot.pending
            ),
            (0, 0, 0, 0, 0, 0)
        );
        source.close().await.unwrap();
    }
    // 8,400 operations and 4,200 authority lifetimes exceed both V1 bounds.
    // Active capacity follows one live cohort; retained archives still grow.
    assert!(maximum_pages < 256 * 4096);
    let archive = fixture.indexed(false).await;
    for name in ["cycle-0", "cycle-1399"] {
        for (operation, _) in lifecycle(&fixture.config, name) {
            assert_eq!(
                archive.authorize(&[operation]).await.unwrap_err().code,
                "mutation_resource_retired"
            );
        }
    }
    archive.close().await.unwrap();
    let (old_proof, old_observation, _) = first_receipt.unwrap();
    // An old transition cannot certify its old after-state after later work.
    assert!(fixture.apply(old_proof, old_observation).await.is_err());
}

#[tokio::test]
async fn indexed_deactivation_retry_refuses_effects_after_the_recorded_transition() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    let source = fixture.active(false).await;
    fixture.cycle(&source, "new", 3).await;
    let before = fingerprint(&source, "").await;
    source.close().await.unwrap();
    assert!(fixture.apply(proof, observation).await.is_err());
    let source = fixture.active(false).await;
    assert_eq!(fingerprint(&source, "").await, before);
    source.close().await.unwrap();
}

#[tokio::test]
async fn indexed_deactivation_keeps_a_worktree_used_by_another_live_group() {
    for spelling in ["canonical", "trailing", "double", "dot"] {
        shared_worktree_case(spelling, true).await;
    }
}

#[tokio::test]
async fn indexed_deactivation_preserves_an_unrelated_live_worktree() {
    for spelling in ["canonical", "trailing", "double", "dot"] {
        shared_worktree_case(spelling, false).await;
    }
}

async fn shared_worktree_case(spelling: &str, shared: bool) {
    let fixture = Fixture::new();
    let source = fixture.active(true).await;
    fixture.cycle(&source, "retired", 6).await;
    fixture.cycle(&source, "live", 2).await;
    let name = if shared { "retired" } else { "live" };
    let canonical = crate::worktree::expected_path(&fixture.config.home, "workspace", name);
    // No worktree exists on disk: identity comparison must not require fs IO.
    assert!(!canonical.exists());
    let cwd = canonical.to_string_lossy().into_owned();
    let cwd = match spelling {
        "canonical" => cwd,
        "trailing" => format!("{cwd}/"),
        "double" => cwd.replace("/.worktrees/", "//.worktrees/"),
        "dot" => cwd.replace("/.worktrees/", "/./.worktrees/"),
        _ => unreachable!(),
    };
    let home = fixture
        .config
        .agent_home("live")
        .to_string_lossy()
        .into_owned();
    let operation = Operation::open_agent(&layer3_adapter::AgentLaunch {
        operation_id: "agent:live:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: layer3_adapter::AgentSlot::new("live"),
        cwd: cwd.clone(),
        home: home.clone(),
        program: "synthetic".into(),
        args: Vec::new(),
        env: Default::default(),
    });
    crate::mutation_journal::validate_operation(&fixture.config, &operation).unwrap();
    source.admit(3, vec![operation.clone()]).await.unwrap();
    let receipt = serde_json::json!({
        "operation_id": operation.operation_id,
        "request_sha256": operation.request_sha256,
        "agent_id": "live", "cwd": cwd, "home": home,
    });
    let begun = source.begin(operation).await.unwrap();
    source
        .finish(begun.record.completed(&receipt).unwrap())
        .await
        .unwrap();
    let before = source.snapshot().await.unwrap();
    let all_rows = fingerprint(&source, "").await;
    let retired_operation = lifecycle(&fixture.config, "retired")[0].0.clone();
    let key = group_key(&retired_operation);
    let retained = fingerprint(&source, &key).await;
    source.close().await.unwrap();
    fixture.indexed(true).await.close().await.unwrap();
    let (proof, observation) = select(&fixture, key.clone(), true).await;
    let database = fixture.root.join("active/ledger.sqlite3");
    let original_database = std::fs::read(&database).unwrap();
    let reserves = [
        fixture.root.join("active/emergency.reserve"),
        fixture.path.join("emergency.reserve"),
    ];
    let reserve_bytes = reserves.each_ref().map(|path| std::fs::read(path).unwrap());
    let result = fixture.apply(proof.clone(), observation).await;
    let source = fixture.active(false).await;
    if shared {
        assert_eq!(
            result.unwrap_err().code,
            "mutation_ledger_invalid",
            "{spelling}"
        );
        assert_eq!(source.snapshot().await.unwrap(), before);
        assert_eq!(fingerprint(&source, "").await, all_rows);
        assert_eq!(std::fs::read(&database).unwrap(), original_database);
    } else {
        let receipt = result.unwrap();
        assert_eq!((receipt.operations, receipt.authority), (2, 1));
        assert_eq!(receipt.after_revision, receipt.before_revision + 1);
        assert_eq!(source.snapshot().await.unwrap().processes, 1);
        assert_eq!(fingerprint(&source, &key).await, retained);
    }
    source.close().await.unwrap();
    for (path, bytes) in reserves.iter().zip(reserve_bytes) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    let archive = fixture.indexed(false).await;
    assert!(!archive.verify_indexed(proof).await.unwrap().complete);
    assert!(archive.require_ready().await.is_err());
    assert_eq!(
        archive
            .authorize(&[retired_operation])
            .await
            .unwrap_err()
            .code,
        "mutation_resource_retired"
    );
    archive.close().await.unwrap();
}
