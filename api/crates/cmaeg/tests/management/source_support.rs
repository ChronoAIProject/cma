//! Source service fixtures share the real record and provider HTTP backends.
pub(super) use super::{consumer_support::*, *};
pub(super) use cmaeg::channels::{sources, ChannelTarget};
pub(super) use cmaeg_core::{channel::SourceId, ids::ChannelId};
pub(super) use serde_json::Value;

pub(super) async fn start() -> Gateway {
    let mut gateway = Gateway::start(false).await;
    gateway.state.routing.dynamic_sources_enabled = true;
    gateway
}
pub(super) fn relay(platform: &str, route: &str) -> Value {
    let mut source = routing_fixture::relay(platform, route);
    source.as_object_mut().unwrap().remove("jwks_url");
    if matches!(platform, "discord" | "slack" | "whatsapp") {
        source["trust_normalized"] = true.into();
    }
    source
}
pub(super) fn custom(key: &str) -> Value {
    json!({"type":"custom","auth":{"type":"hmac","key_id":key}})
}
pub(super) fn request(
    version: u64,
    source: Value,
    credentials: Value,
    custom_key: Option<&str>,
) -> sources::SourceAttachRequest {
    serde_json::from_value(json!({"expected_version":version,"source":source,"credentials":credentials,"partition_custom_key":custom_key})).unwrap()
}
pub(super) async fn attach(
    gateway: &Gateway,
    id: &ChannelId,
    request: sources::SourceAttachRequest,
) -> Result<
    (cmaeg::channels::management::LoadedChannel, SourceId),
    cmaeg::channels::management::ManagementError,
> {
    sources::attach(
        &gateway.state,
        &creator(),
        &ChannelTarget::Id(id.clone()),
        request,
    )
    .await
}
pub(super) async fn detach(
    gateway: &Gateway,
    id: &ChannelId,
    name: &str,
    version: u64,
) -> Result<cmaeg::channels::management::LoadedChannel, cmaeg::channels::management::ManagementError>
{
    sources::detach_named(
        &gateway.state,
        &creator(),
        &ChannelTarget::Id(id.clone()),
        name,
        version,
    )
    .await
}
pub(super) fn invalid<T: std::fmt::Debug>(
    result: Result<T, cmaeg::channels::management::ManagementError>,
    pointer: &str,
    message: &str,
) {
    let cmaeg::channels::management::ManagementError::Invalid(error) = result.unwrap_err() else {
        panic!("expected invalid source request")
    };
    assert_eq!(error.pointer, pointer);
    assert_eq!(error.message, message);
}
