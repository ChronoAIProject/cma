//! A refused create leaves no channel.
use super::{
    management::{self, ManagementError},
    mutation_fixture::{owner, Fixture},
};
use crate::providers::ProviderError;

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
