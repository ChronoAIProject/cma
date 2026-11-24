use super::*;

async fn usage_save_retries_known_aborts_without_refreshing_revision_or_replaying_unknown_case(
    scoped: bool,
    failures: i32,
    unknown: bool,
    stale: bool,
) {
    let f = Fixture::new().await;
    let (claimed, agents, _) = f.phase().await;
    let before = f.observations(&claimed, &agents).await;
    let mut next = claimed.clone();
    next.revision += if stale { 2 } else { 1 };
    next.updated_at = now();
    let expected = claimed.revision + u64::from(stale);
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.fail(
                if unknown {
                    "commitTransaction"
                } else {
                    "update"
                },
                failures,
                unknown,
            )
            .await;
            let save =
                crate::store::sandbox_usage::save::save(&f.store, expected, &next, None, None);
            if scoped {
                crate::store::work_publication::scope(&f.store, save).await
            } else {
                save.await
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
    let after = f.observations(&claimed, &agents).await;
    let usage = f
        .independent
        .sandbox_usage
        .find_one(doc! {"_id":claimed.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    f.drop_database().await;
    if unknown {
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_fresh_transactions(&starts, 1);
        // An unacknowledged commit can leave either complete outcome.
        // Reconcile exact content, permitting only lease renewal; never
        // infer application or accept a partial successor from revision.
        let original =
            layer2_core::sandbox::store::sandbox_content_matches(after.0.as_ref(), &claimed);
        let successor =
            layer2_core::sandbox::store::sandbox_content_matches(after.0.as_ref(), &next);
        assert!(
            original ^ successor,
            "unknown commit must retain exactly the original or one successor"
        );
    } else if stale {
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert_fresh_transactions(&starts, 2);
        assert_eq!(after.0.as_ref().unwrap().revision, claimed.revision);
    } else if failures == 5 {
        // Known-abort exhaustion retains the established usage-save result.
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert_fresh_transactions(&starts, 5);
        assert_eq!(after.0.as_ref().unwrap().revision, claimed.revision);
    } else {
        result.expect("usage save discarded known-aborted authority fence");
        assert_fresh_transactions(&starts, 2);
        assert_eq!(after.0.as_ref().unwrap().revision, next.revision);
    }
    assert_eq!((after.1, after.2, after.3), (before.1, before.2, before.3));
    assert_eq!(
        crate::store::sandbox_usage::read_state(&usage)
            .unwrap()
            .source_revision,
        after.0.as_ref().unwrap().revision
    );
}

#[path = "usage_cases.rs"]
mod cases;
