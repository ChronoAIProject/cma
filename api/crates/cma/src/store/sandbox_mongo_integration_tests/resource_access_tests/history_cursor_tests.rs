use super::fixture::context;
use super::*;

#[path = "history_cursor_adoption_tests.rs"]
mod adoption;

#[path = "history_page_budget_tests.rs"]
mod page_budget;

#[tokio::test]
#[ignore = "requires the disposable replica-set runner"]
async fn cursor_pages_keep_semantics_and_idle_authority_has_no_history_growth() {
    let mut idle = None;
    for rows in [1_000, 10_000] {
        let mut f = Fixture::semantic(rows).await;
        f.store = f
            .store
            .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
            .await
            .unwrap();
        let reads = f.reads(false);
        let context = context(f.agent.owner.clone());
        let authorization = reads
            .authorize_event_stream(&context, None, &f.agent.id)
            .await
            .unwrap();
        let mut cursor = AgentHistoryCursor::default();
        let before = scans();
        f.meter.clear();
        let mut pages = 0;
        while cursor.sequence() < rows {
            let page = reads
                .event_stream_page(&authorization, &mut cursor, None)
                .await
                .unwrap();
            assert!(!page.history.is_empty() && page.history.len() <= 200);
            assert_eq!(page.history.last().unwrap().seq, cursor.sequence());
            pages += 1;
        }
        let after = scans();
        assert_eq!(
            &before[..3],
            &after[..3],
            "prepared cursor pages must not scan complete history"
        );
        let bootstrap = f.meter.stats.lock().unwrap().clone();
        eprintln!("cursor_bootstrap rows={rows} pages={pages} commands={} returned_docs={} returned_bytes={}", bootstrap.commands.values().sum::<u64>(), bootstrap.docs, bootstrap.bytes);
        f.meter.clear();
        // Exactly the unchanged-tail operation used by the stream. This is 100
        // independent fresh evaluations, not a simulated or cached authority.
        for _ in 0..100 {
            let header = reads
                .current_event_stream_header(&authorization)
                .await
                .unwrap();
            assert_eq!(header.head, rows);
        }
        let actual = f.meter.stats.lock().unwrap().clone();
        let commands = actual.commands.values().sum::<u64>();
        assert_eq!(commands, actual.checkouts);
        assert_eq!(actual.snapshots, 100);
        assert_eq!(
            commands, 300,
            "shared cohort batching belongs to the next slice"
        );
        assert!(actual
            .commands
            .keys()
            .all(|key| key.starts_with("authorization/")));
        if let Some(prior) = idle {
            assert_eq!(commands, prior);
        }
        idle = Some(commands);
        eprintln!("cursor_idle rows={rows} observers=100 refreshes=100 commands={commands} returned_docs={} returned_bytes={} log_commands=0", actual.docs, actual.bytes);
        // A deleted Agent cannot reuse either cursor or held stream authority.
        f.store
            .agents
            .delete_one(doc! {"_id":f.agent.id.as_str()})
            .await
            .unwrap();
        assert!(reads
            .current_event_stream_header(&authorization)
            .await
            .is_err());
        assert!(reads
            .event_stream_page(&authorization, &mut cursor, None)
            .await
            .is_err());
        assert_eq!(cursor.sequence(), rows);
        f.client.database(&f.name).drop().await.unwrap();
    }
}
