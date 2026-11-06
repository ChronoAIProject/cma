use super::tests::{call, Store};
use super::*;
use layer2_core::Capability;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn owner_allocation_context_rechecks_directory_permission_on_each_page() {
    let store = Arc::new(Store::default());
    let base = "/api/v1/admin/usage/metrics/reports";
    let caps = &[Capability::ReadAllSandboxes, Capability::ReadUserDirectory][..];
    call(&store, "POST", base, Some(("operator", caps)), "{}").await;
    let path = format!("{base}/umr_00000000000000000000000000000001/owner-allocations?metric=cpu");
    assert_eq!(
        call(&store, "GET", &path, Some(("operator", caps)), "")
            .await
            .status(),
        StatusCode::OK
    );
    let continuation = format!("{path}&cursor=retained-page");
    assert_eq!(
        call(
            &store,
            "GET",
            &continuation,
            Some(("operator", &[Capability::ReadAllSandboxes])),
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(matches!(
        store.owner_context.lock().unwrap().as_slice(),
        [
            OwnerContextAccess::DirectoryRead,
            OwnerContextAccess::DateOnly
        ]
    ));
    for suffix in ["&directory_read=true", "&owner_context=DirectoryRead"] {
        assert_eq!(
            call(
                &store,
                "GET",
                &format!("{path}{suffix}"),
                Some(("operator", caps)),
                ""
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn owner_allocations_authorize_every_page_validate_and_revoke_cached_reports() {
    let store = Arc::new(Store::default());
    let admin = Some(("operator", &[Capability::ReadAllSandboxes][..]));
    let base = "/api/v1/admin/usage/metrics/reports";
    assert_eq!(
        call(&store, "POST", base, admin, "{}").await.status(),
        StatusCode::OK
    );
    let path = format!("{base}/umr_00000000000000000000000000000001/owner-allocations");
    for suffix in ["?metric=cpu", "?metric=disk&cursor=retained-page"] {
        let path = format!("{path}{suffix}");
        assert_eq!(
            call(&store, "GET", &path, None, "").await.status(),
            StatusCode::UNAUTHORIZED
        );
        for caps in [&[][..], &[Capability::UseOwnSandboxes][..]] {
            assert_eq!(
                call(&store, "GET", &path, Some(("operator", caps)), "")
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            call(
                &store,
                "GET",
                &path,
                Some(("other", &[Capability::ReadAllSandboxes])),
                ""
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&store, "GET", &path, admin, "").await.status(),
            StatusCode::OK
        );
    }
    for suffix in [
        "?metric=cpu&limit=0",
        "?metric=cpu&limit=201",
        "?metric=other",
        "?metric=cpu&cursor=",
        "?metric=cpu&owner=forged",
        "",
    ] {
        assert_eq!(
            call(&store, "GET", &format!("{path}{suffix}"), admin, "")
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let too_long = format!("{path}?metric=cpu&cursor={}", "x".repeat(4097));
    assert_eq!(
        call(&store, "GET", &too_long, admin, "").await.status(),
        StatusCode::BAD_REQUEST
    );
    store.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        call(&store, "GET", &format!("{path}?metric=cpu"), admin, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    store.revoked.store(false, Ordering::SeqCst);
    store.saved.lock().unwrap().as_mut().unwrap().expires_at = 0;
    assert_eq!(
        call(&store, "GET", &format!("{path}?metric=cpu"), admin, "")
            .await
            .status(),
        StatusCode::GONE
    );
}
