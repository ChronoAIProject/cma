use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn usage_save_retries_known_aborts_without_refreshing_revision_or_replaying_unknown() {
    for scoped in [false, true] {
        for (failures, unknown, stale) in [
            (1, false, false),
            (5, false, false),
            (5, true, false),
            (1, false, true),
        ] {
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
                        crate::store::sandbox_usage::save::save(&f.store, expected, &next, None);
                    if scoped {
                        super::super::scope(&f.store, save).await
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
                assert_eq!(after.0.as_ref().unwrap().revision, next.revision);
            } else if stale {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_fresh_transactions(&starts, 2);
                assert_eq!(after.0.as_ref().unwrap().revision, claimed.revision);
            } else if failures == 5 {
                assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
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
    }
}
