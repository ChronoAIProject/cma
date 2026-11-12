use super::*;

pub(super) async fn page(
    fixture: &Fixture,
    after: u64,
    through: u64,
    limit: u16,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut rows = Vec::new();
    fixture
        .store
        .visit_agent_log_page_with_tail(&fixture.id, after, through, limit, &mut |entry| {
            rows.push(entry);
            Ok(())
        })
        .await?;
    Ok(rows)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn tail_proof_shares_snapshot_with_predecessor_and_frozen_page() {
    for prepared in [false, true] {
        let fixture = Fixture::new(260).await;
        let fixture = if prepared {
            fixture.prepare().await
        } else {
            fixture
        };
        let entries = fixture
            .store
            .agent_log_prefix(&fixture.id, 260)
            .await
            .unwrap();
        for (after, through, limit) in [(0_u64, 0, 1), (0, 100, 100), (99, 200, 100), (200, 200, 1)]
        {
            let start = after.saturating_sub(1);
            let end = through.min(after + u64::from(limit));
            fixture.reset_commands();
            let rows = page(&fixture, after, through, limit).await.unwrap();
            assert_eq!(
                rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
                ((start + 1)..=end).collect::<Vec<_>>()
            );
            fixture.assert_read_only();
            let commands = fixture.commands.lock().unwrap();
            let first = commands[0].get_document("lsid").unwrap();
            for command in commands.iter() {
                assert_eq!(first, command.get_document("lsid").unwrap());
                if command.contains_key("find") {
                    assert!(command_uses_snapshot_read(command));
                }
            }
        }
        assert_eq!(
            page(&fixture, 0, 261, 1).await,
            Err(SandboxStoreError::SequenceConflict)
        );
        for (after, through, limit) in [(0, 1, 0), (0, 1, 201), (2, 1, 1)] {
            assert_eq!(
                page(&fixture, after, through, limit).await,
                Err(SandboxStoreError::InvalidPage)
            );
        }
        assert_eq!(
            fixture
                .store
                .visit_agent_log_page_with_tail(&fixture.id, 0, 1, 1, &mut |_| Err(
                    SandboxStoreError::InvalidPage
                ))
                .await,
            Err(SandboxStoreError::InvalidPage)
        );
        // The visitor-failure contract drops a partially consumed cursor;
        // its asynchronous killCursors cleanup is outside the read session.
        // Run it after measuring the successful page commands, not before.
        layer2_core::sandbox::store::agent_log_contract::assert_tail_page_contract(
            &fixture.store,
            &entries,
        )
        .await;
        fixture.cleanup().await;
    }
}
