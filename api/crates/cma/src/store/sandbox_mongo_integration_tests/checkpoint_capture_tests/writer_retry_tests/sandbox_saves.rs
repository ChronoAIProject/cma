//! A known-aborted lifecycle save must yield time to competing parent writers.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn sandbox_saves_pace_known_aborts_without_partial_usage_or_extra_attempts() {
    for exhausted in [false, true] {
        let f = Fixture::new().await;
        let (parent, _, _) = f.due("sandbox_save").await;
        let mut next = parent.clone();
        next.name = "renamed-sandbox".into();
        next.revision += 1;
        next.updated_at = MINUTE.into();
        f.conflicts(if exhausted { 25 } else { 4 }).await;
        let result = f.store.save_sandbox(parent.revision, next.clone()).await;
        let boundaries = f.disable().await;
        let saved = f.store.get_sandbox(&parent.id).await.unwrap();
        let usage_rows = f
            .store
            .sandbox_usage
            .count_documents(doc! {})
            .await
            .unwrap();
        let usage_events = f
            .store
            .sandbox_usage_events
            .count_documents(doc! {})
            .await
            .unwrap();
        // Dispose the fixture even if a timing assertion exposes the regression.
        f.client.database(&f.name).drop().await.unwrap();
        if exhausted {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(saved, Some(parent));
            assert_eq!((usage_rows, usage_events), (0, 0));
        } else {
            result.unwrap();
            assert_eq!(saved, Some(next));
            assert_eq!((usage_rows, usage_events), (1, 1));
        }
        assert_paced(&boundaries, exhausted);
    }
}
