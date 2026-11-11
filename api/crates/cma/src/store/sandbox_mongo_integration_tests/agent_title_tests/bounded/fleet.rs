use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_selection_keeps_the_title_switch_independent() {
    let mut f = Fixture::new(1_000, 6, false, 64).await;
    f.store.bounded_admission_agents.clear();
    f.store = f
        .store
        .with_fleet_log_percentage(100)
        .with_bounded_title_reads(false);
    for (enabled, rows) in [(false, 1_001), (true, 201)] {
        f.store = f.store.with_bounded_title_reads(enabled);
        f.reset();
        reads::isolated_test(async {
            assert_eq!(
                f.store.agent_title_transcript(&f.agent.id).await.unwrap(),
                f.expected()
            );
            assert_eq!(reads::snapshot(AGENT_LOG)[2], rows);
        })
        .await;
        f.assert_snapshot_read_only();
    }
    f.cleanup().await;
}
