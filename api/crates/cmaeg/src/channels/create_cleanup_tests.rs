//! A refused create leaves no channel; a refused release is typed and retained.
use super::{
    management::{self, ManagementError},
    mutation_fixture::{owner, Fixture},
};
use crate::providers::ProviderError;
use axum::http::StatusCode;

#[tokio::test]
async fn an_unlisted_provider_is_refused_before_any_channel_is_reserved() {
    for custody in [false, true] {
        let fixture = Fixture::new(false, custody).await;
        let state = fixture.state().await;
        let mut body = fixture.body();
        body["provider"]["slug"] = "unlisted".into();
        body["profile"]["metadata"]["provider"] = "unlisted".into();
        let result = management::create(
            &state,
            &owner("alice", false),
            serde_json::from_value(body).unwrap(),
        )
        .await;
        assert!(
            matches!(
                result,
                Err(ManagementError::Provider(ProviderError::UnknownProvider(_)))
            ),
            "custody={custody}: {result:?}"
        );
        assert!(fixture.peer.rows.lock().unwrap().is_empty(), "no record");
        assert!(fixture.peer.calls.lock().unwrap().is_empty(), "no send");
    }
}

#[tokio::test]
async fn a_refused_release_is_typed_and_retains_the_channel() {
    for custody in [false, true] {
        let fixture = Fixture::new(false, custody).await;
        let state = fixture.state().await;
        let creator = owner("alice", false);
        let created = management::create(
            &state,
            &creator,
            serde_json::from_value(fixture.body()).unwrap(),
        )
        .await
        .unwrap();
        *fixture.peer.refuse_delete.lock().unwrap() = Some((StatusCode::FORBIDDEN, "forbidden"));
        let refused =
            management::delete(&state, &creator, &created.channel_id, created.version).await;
        assert!(
            matches!(
                &refused,
                Err(ManagementError::ReleaseRefused { status: 403, code }) if code == "forbidden"
            ),
            "custody={custody}: {refused:?}"
        );
        assert!(management::load(&state, &creator, &created.channel_id)
            .await
            .is_ok());
        assert_eq!(fixture.peer.bound.lock().unwrap().len(), 1);
        if !custody {
            // Once the provider releases it, the same request completes.
            *fixture.peer.refuse_delete.lock().unwrap() = None;
            management::delete(&state, &creator, &created.channel_id, created.version)
                .await
                .unwrap();
            assert!(matches!(
                management::load(&state, &creator, &created.channel_id).await,
                Err(ManagementError::NotFound)
            ));
        }
    }
}

#[tokio::test]
async fn a_release_reported_absent_or_unavailable_keeps_its_existing_outcome() {
    let fixture = Fixture::new(false, false).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    for (status, deleted) in [
        (StatusCode::SERVICE_UNAVAILABLE, false),
        (StatusCode::NOT_FOUND, true),
    ] {
        let created = management::create(
            &state,
            &creator,
            serde_json::from_value(fixture.body()).unwrap(),
        )
        .await
        .unwrap();
        *fixture.peer.refuse_delete.lock().unwrap() = Some((status, "binding_not_found"));
        let result =
            management::delete(&state, &creator, &created.channel_id, created.version).await;
        let retained = management::load(&state, &creator, &created.channel_id)
            .await
            .is_ok();
        if deleted {
            assert!(result.is_ok(), "{result:?}");
            assert!(!retained);
        } else {
            assert!(
                matches!(
                    result,
                    Err(ManagementError::Provider(ProviderError::Unavailable(_)))
                ),
                "{result:?}"
            );
            assert!(retained);
        }
    }
}
