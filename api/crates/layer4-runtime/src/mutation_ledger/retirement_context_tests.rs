use super::tests::{finish, inventory};
use super::*;
use crate::mutation_ledger::{budget, lifecycle_tests::lifecycle, tests::Fixture};

async fn live(ledger: &Ledger, config: &RuntimeConfig) {
    for (index, (operation, receipt)) in lifecycle(config, "context-agent")
        .into_iter()
        .take(3)
        .enumerate()
    {
        ledger
            .admit(index as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        let entry = ledger.begin(operation).await.unwrap();
        ledger
            .finish(entry.record.completed(&receipt).unwrap())
            .await
            .unwrap();
    }
}

async fn refuses(ledger: &Ledger) {
    let mut cursor = None;
    for _ in 0..20 {
        match ledger.retirement_inventory(cursor).await {
            Err(_) => return,
            Ok(page) => {
                assert!(page.next.is_some(), "damaged inventory was accepted");
                cursor = page.next;
            }
        }
    }
    panic!("inventory did not finish within the fixture bound");
}

#[tokio::test]
async fn retirement_inventory_retains_historical_processes_reservations_and_context() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("context-home".into());
    let generation = fixture.config.generation;
    let ledger = fixture.open(true).await;
    live(&ledger, &fixture.config).await;
    let context = ledger.retirement_inventory(None).await.unwrap().context;
    assert_eq!(context.accounting, [9, 3 * budget::OUTCOME, 3, 3, 3]);
    ledger.close().await.unwrap();
    fixture.config.generation += 1;
    let ledger = fixture.open(false).await;
    let before = ledger.snapshot().await.unwrap();
    let (rows, _, digest) = inventory(&ledger).await;
    let mut cleanup = 0;
    let mut scopes = Vec::new();
    let mut processes = 0;
    for row in &rows {
        match row {
            SourceRow::Auxiliary(Auxiliary::Cleanup { available, .. }) => {
                assert!(*available);
                cleanup += 1;
            }
            SourceRow::Auxiliary(Auxiliary::Scope {
                scope,
                pending,
                processes,
            }) => {
                scopes.push((
                    u64::from_be_bytes(scope.as_slice().try_into().unwrap()),
                    *pending,
                    *processes,
                ));
            }
            SourceRow::Auxiliary(Auxiliary::Process {
                scope,
                agent,
                cwd,
                home,
            }) => {
                assert_eq!(
                    u64::from_be_bytes(scope.as_slice().try_into().unwrap()),
                    generation
                );
                assert_eq!(agent, "context-agent");
                assert_eq!(std::path::Path::new(home), fixture.config.agent_home(agent));
                assert_eq!(
                    std::path::Path::new(cwd),
                    crate::worktree::expected_path(&fixture.config.home, "workspace", agent)
                );
                processes += 1;
            }
            _ => {}
        }
    }
    assert_eq!((cleanup, processes), (3, 1));
    assert_eq!(scopes, [(generation, 0, 1)]);
    assert_eq!(rows.len(), 11);
    assert_eq!(ledger.snapshot().await.unwrap(), before);
    assert_eq!(
        before.processes, 0,
        "current-generation observation omits old processes"
    );
    assert_eq!(inventory(&ledger).await.2, digest);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_pages_zero_and_historical_scope_rows_with_index_ranges() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("scope-home".into());
    fixture.config.generation = 130;
    let ledger = fixture.open(true).await;
    ledger
        .storage
        .call(|db| {
            for generation in 0_u64..130 {
                db.execute(
                    "INSERT INTO scope_budget VALUES (?,0,0)",
                    [generation.to_be_bytes().as_slice()],
                )
                .unwrap();
            }
            let plan = db
                .prepare(&format!("EXPLAIN QUERY PLAN {}", auxiliary::SCOPES))
                .unwrap()
                .query_map([Vec::<u8>::new()], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join("\n");
            assert!(
                plan.contains("SEARCH") && plan.contains("PRIMARY KEY"),
                "{plan}"
            );
            assert!(
                !plan.contains("SCAN") && !plan.contains("TEMP B-TREE"),
                "{plan}"
            );
            Ok(())
        })
        .await
        .unwrap();
    let (rows, lengths, _) = inventory(&ledger).await;
    assert_eq!(lengths, [0, 0, 0, 64, 64, 2, 0]);
    for (expected, row) in rows.iter().enumerate() {
        let SourceRow::Auxiliary(Auxiliary::Scope {
            scope,
            pending,
            processes,
        }) = row
        else {
            panic!("expected scope row");
        };
        assert_eq!(
            u64::from_be_bytes(scope.as_slice().try_into().unwrap()),
            expected as u64
        );
        assert_eq!((*pending, *processes), (0, 0));
    }
    assert_eq!(rows.len(), 130);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_refuses_changed_accounting_or_geometry_without_revision() {
    for sql in [
        "UPDATE accounting SET reserved=reserved+1",
        "UPDATE accounting SET authority=authority+1",
        "PRAGMA max_page_count=65536",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        finish(&ledger, 0).await;
        let cursor = ledger
            .retirement_inventory(None)
            .await
            .unwrap()
            .next
            .unwrap();
        ledger
            .storage
            .call(move |db| {
                db.execute_batch(sql).unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            ledger.retirement_inventory(Some(cursor)).await.is_err(),
            "{sql}"
        );
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_refuses_damaged_context_and_auxiliary_columns() {
    for sql in [
        "UPDATE cleanup_budget SET available=2",
        "UPDATE cleanup_budget SET resource='{}' WHERE resource=(SELECT resource FROM cleanup_budget ORDER BY scope,resource LIMIT 1)",
        "UPDATE scope_budget SET pending=-1",
        "UPDATE scope_budget SET processes=-1",
        "UPDATE scope_budget SET scope=x''",
        "UPDATE logical_processes SET scope=x''",
        "UPDATE logical_processes SET home='/another/home'",
        "UPDATE logical_processes SET cwd='/another/worktree'",
        "UPDATE logical_processes SET agent='../escape'",
        "UPDATE accounting SET terminal=operations+1",
        "UPDATE accounting SET reserved=-1",
        "UPDATE metadata SET sandbox='other-sandbox'",
        "UPDATE metadata SET admission='import_held'",
        "UPDATE metadata SET home_root='/another/home'",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        live(&ledger, &fixture.config).await;
        ledger
            .storage
            .call(move |db| {
                db.pragma_update(None, "ignore_check_constraints", true)
                    .unwrap();
                db.execute_batch(sql).unwrap();
                db.pragma_update(None, "ignore_check_constraints", false)
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        refuses(&ledger).await;
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_preserves_consumed_cleanup_and_empty_scope_projections() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for (index, (operation, receipt)) in lifecycle(&fixture.config, "context-agent")
        .into_iter()
        .enumerate()
    {
        ledger
            .admit(index as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        let entry = ledger.begin(operation).await.unwrap();
        ledger
            .finish(entry.record.completed(&receipt).unwrap())
            .await
            .unwrap();
    }
    // Historical writers retain empty scope rows. New completions prune them,
    // but inventory and archival must still preserve an older exact source.
    ledger
        .storage
        .call(|db| {
            db.execute("INSERT INTO scope_budget VALUES (zeroblob(8),0,0)", [])
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (rows, _, _) = inventory(&ledger).await;
    let cleanup = rows
        .iter()
        .filter(|row| {
            matches!(
                row,
                SourceRow::Auxiliary(Auxiliary::Cleanup {
                    available: false,
                    ..
                })
            )
        })
        .count();
    let scopes = rows
        .iter()
        .filter(|row| {
            matches!(
                row,
                SourceRow::Auxiliary(Auxiliary::Scope {
                    pending: 0,
                    processes: 0,
                    ..
                })
            )
        })
        .count();
    assert_eq!((cleanup, scopes), (3, 1));
    assert!(!rows
        .iter()
        .any(|row| matches!(row, SourceRow::Auxiliary(Auxiliary::Process { .. }))));
    let context = ledger.retirement_inventory(None).await.unwrap().context;
    assert_eq!(context.accounting, [18, 0, 6, 3, 6]);
    ledger.close().await.unwrap();
}
