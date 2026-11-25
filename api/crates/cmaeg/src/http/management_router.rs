//! Versioned Channel management route registration.
use axum::{
    routing::{get, post, put},
    Router,
};

use crate::channels::management::ManagementState;

use super::{
    create_channel, delete_channel, get_channel, list_channels, permission_catalog, rekey_channel,
    test_channel, update_channel,
};
use crate::http::{self, problems::protocol_headers};

pub fn management_router(state: ManagementState) -> Router {
    Router::new()
        .merge(crate::github::http::routes())
        .route(
            "/v1/channels/{channel_id}/partition",
            get(http::partitions::inspect),
        )
        .route(
            "/v1/channels/{channel_id}/partition/resets",
            post(http::partition_resets::reset),
        )
        .route(
            "/v1/channels/{channel_id}/activity",
            get(http::activity::latest),
        )
        .route(
            "/v1/channels/{channel_id}/activity/stream",
            get(http::activity::stream),
        )
        .route(
            "/v1/channels/{channel_id}/activity/history",
            get(http::activity_history::history),
        )
        .route(
            "/v1/channels/{channel_id}/activity/history/events/{event_id}/details",
            get(http::activity_history::details),
        )
        .merge(http::outbound::routes())
        .route(
            "/v1/channels/{channel_id}/events/{event_id}/media-responses",
            get(http::media_responses::list),
        )
        .route("/v1/permissions", get(permission_catalog))
        .route("/v1/channels", get(list_channels).post(create_channel))
        .route(
            "/v1/channel-records/{record_id}",
            get(http::creation::recover_channel),
        )
        .route(
            "/v1/channel-records/{record_id}/cancel-create",
            post(http::creation::cancel_create),
        )
        .route(
            "/v1/channel-records/cancel-create-capability",
            get(http::creation::cancel_create_capability),
        )
        .route(
            "/v1/channel-records/{record_id}/management",
            get(http::record_management::read).delete(http::record_management::retire),
        )
        .route(
            "/v1/channels/{channel_id}",
            get(get_channel).put(update_channel).delete(delete_channel),
        )
        .merge(http::profile_revisions::routes())
        .route("/v1/channels/{channel_id}/test", post(test_channel))
        .route(
            "/v1/channels/{channel_id}/sources/{source_index}/test",
            post(http::channel_nodes::test_source),
        )
        .route(
            "/v1/channels/{channel_id}/consumers/{consumer_id}/test",
            post(http::channel_nodes::test_consumer),
        )
        .route(
            "/v1/channels/{channel_id}/routing",
            put(http::channel_nodes::update_routing),
        )
        .route("/v1/channels/{channel_id}/rekey", post(rekey_channel))
        .with_state(state)
        .layer(axum::middleware::from_fn(protocol_headers))
}
