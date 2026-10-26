//! Explicit owner prepare/commit/cancel over durable, narrowly bound authority.
mod store;
pub mod types;
use crate::{
    auth::Creator,
    channels::{
        management::{self, ManagementState},
        state::LoadedChannel,
    },
    delivery::{intents::DeliveryOutcome, relay_client::RelayClient},
};
use cmaeg_core::{
    channel::{Secret, Source},
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};
use store::{hash, not_found, Operation, Store};
pub use types::*;

fn records(state: &ManagementState) -> Store {
    Store::new((*state.records).clone(), state.master_keys.clone())
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub(super) fn unavailable() -> Problem {
    Problem::new(ProblemCode::Internal, 503, "outbound_store_unavailable")
}
fn invalid(detail: &str) -> Problem {
    Problem::new(ProblemCode::InvalidRequest, 400, detail)
}
async fn channel(
    state: &ManagementState,
    creator: &Creator,
    id: &str,
) -> Result<LoadedChannel, Problem> {
    let id = cmaeg_core::ids::ChannelId::new(id).map_err(|_| not_found())?;
    management::load(state, creator, &id)
        .await
        .map_err(|e| match e {
            management::ManagementError::NotFound => not_found(),
            _ => unavailable(),
        })
}
fn enabled(state: &ManagementState) -> Result<&RelayClient, Problem> {
    if !state.outbound_messages_writes_enabled {
        return Err(Problem::new(
            ProblemCode::Forbidden,
            403,
            "outbound_messages_writes_disabled",
        ));
    }
    state.outbound_relay.as_ref().ok_or_else(unavailable)
}
fn authority<'a>(
    channel: &'a LoadedChannel,
    source_id: &cmaeg_core::channel::SourceId,
    route_id: &str,
) -> Result<(&'a Source, &'a Secret, String), Problem> {
    if uuid::Uuid::parse_str(route_id).is_err() {
        return Err(invalid("outbound_destination_requires_saved_route_uuid"));
    }
    let source = channel
        .definition
        .sources
        .iter()
        .find(|source| source.source_id() == *source_id)
        .ok_or_else(|| invalid("outbound_destination_not_configured"))?;
    let Source::NyxidRelay { route_ids, .. } = source else {
        return Err(invalid("outbound_destination_requires_nyxid_relay"));
    };
    if !route_ids.iter().any(|id| id == route_id) {
        return Err(invalid("outbound_destination_not_configured"));
    }
    let secret = channel
        .definition
        .secrets
        .channel_key
        .as_ref()
        .ok_or_else(|| invalid("outbound_destination_has_no_channel_key"))?;
    if secret.as_str().is_empty() {
        return Err(invalid("outbound_destination_has_no_channel_key"));
    }
    let digest = hash(&serde_json::json!({"source":source,"key":hash(&secret.as_str())}));
    Ok((source, secret, digest))
}
pub async fn prepare(
    state: &ManagementState,
    creator: &Creator,
    id: &str,
    key: &str,
    request: PrepareOutboundMessage,
) -> Result<OutboundReceipt, Problem> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| (33..=126).contains(&b)) {
        return Err(invalid(
            "a_1_to_128_byte_printable_idempotency_key_is_required",
        ));
    }
    let channel = channel(state, creator, id).await?;
    let store = records(state);
    // An exact replay only recovers a receipt. It must survive disabled writers
    // and removed source settings so a lost prepare response remains cancellable.
    let existing_id =
        Operation::identifier(&creator.subject, channel.definition.record_id.as_str(), key);
    match store.load(&existing_id).await {
        Ok(existing) => {
            if existing.owner != creator.subject
                || existing.record_id != channel.definition.record_id.as_str()
                || existing.request_hash != hash(&request)
            {
                return Err(Problem::new(
                    ProblemCode::Conflict,
                    409,
                    "outbound_idempotency_conflict",
                ));
            }
            return Ok(existing.public());
        }
        Err(error) if error.status == 404 => {}
        Err(error) => return Err(error),
    }
    let relay = enabled(state)?;
    let (source, _, digest) = authority(&channel, &request.source_id, &request.route_id)?;
    let Source::NyxidRelay { platform, .. } = source else {
        unreachable!()
    };
    let (_, rendering) =
        relay.prepare_initiated(*platform, &request.route_id, "validation", &request.message)?;
    let mut op = Operation::new(
        &creator.subject,
        channel.definition.record_id.as_str(),
        digest,
        key,
        &request,
        rendering,
        now(),
    );
    match store.save(&op, None).await {
        Ok(version) => {
            op.version = version;
            Ok(op.public())
        }
        Err(error) if error.status == 409 => {
            let existing = store.load(&op.id).await?;
            if existing.owner != op.owner
                || existing.record_id != op.record_id
                || existing.request_hash != op.request_hash
            {
                return Err(Problem::new(
                    ProblemCode::Conflict,
                    409,
                    "outbound_idempotency_conflict",
                ));
            }
            Ok(existing.public())
        }
        Err(error) => Err(error),
    }
}
async fn owned(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &str,
    delivery_id: &str,
) -> Result<(LoadedChannel, Operation), Problem> {
    let channel = channel(state, creator, channel_id).await?;
    let op = records(state).load(delivery_id).await?;
    if op.owner != creator.subject || op.record_id != channel.definition.record_id.as_str() {
        return Err(not_found());
    }
    Ok((channel, op))
}
pub async fn read(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &str,
    delivery_id: &str,
) -> Result<OutboundReceipt, Problem> {
    Ok(owned(state, creator, channel_id, delivery_id)
        .await?
        .1
        .public())
}
pub async fn cancel(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &str,
    delivery_id: &str,
) -> Result<OutboundReceipt, Problem> {
    let (_, mut op) = owned(state, creator, channel_id, delivery_id).await?;
    match op.receipt.status {
        OutboundStatus::Cancelled => return Ok(op.public()),
        OutboundStatus::Committed => {
            return Err(Problem::new(
                ProblemCode::Conflict,
                409,
                "outbound_already_committed",
            ))
        }
        OutboundStatus::Prepared => {}
    }
    op.receipt.status = OutboundStatus::Cancelled;
    records(state).save(&op, Some(op.version)).await?;
    Ok(op.public())
}
pub async fn commit(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &str,
    delivery_id: &str,
    message: OutboundMessage,
) -> Result<OutboundReceipt, Problem> {
    let (channel, mut op) = owned(state, creator, channel_id, delivery_id).await?;
    if hash(&message) != op.message_hash {
        return Err(Problem::new(
            ProblemCode::Conflict,
            409,
            "outbound_message_changed",
        ));
    }
    if op.receipt.status != OutboundStatus::Prepared {
        return Ok(op.public());
    }
    let relay = enabled(state)?;
    if now() >= op.receipt.expires_at {
        return Err(Problem::new(
            ProblemCode::InvalidRequest,
            410,
            "outbound_preparation_expired",
        ));
    }
    let (source, secret, digest) =
        authority(&channel, &op.receipt.source_id, &op.receipt.route_id)?;
    if digest != op.authority_hash {
        return Err(Problem::new(
            ProblemCode::Forbidden,
            403,
            "outbound_authority_changed",
        ));
    }
    let Source::NyxidRelay { platform, .. } = source else {
        unreachable!()
    };
    let (body, _) = relay.prepare_initiated(*platform, &op.receipt.route_id, &op.id, &message)?;
    let store = records(state);
    op.receipt.status = OutboundStatus::Committed;
    op.version = match store.save(&op, Some(op.version)).await {
        Ok(version) => version,
        Err(error) if error.status == 409 => return Ok(store.load(&op.id).await?.public()),
        Err(error) => return Err(error),
    };
    // Re-read after committing: concurrent configuration changes cannot grant
    // a new credential or destination to this admitted operation.
    let current = self::channel(state, creator, channel_id).await;
    let still_authorized = current.as_ref().is_ok_and(|current| {
        current.definition.record_id.as_str() == op.record_id
            && authority(current, &op.receipt.source_id, &op.receipt.route_id)
                .is_ok_and(|(_, _, hash)| hash == op.authority_hash)
    });
    let outcome = if still_authorized {
        relay.send_initiated(secret, body).await
    } else {
        DeliveryOutcome::Rejected {
            reason: "outbound_authority_changed".into(),
        }
    };
    op.receipt.delivery = Some(outcome);
    // Failed receipt persistence leaves committed/unknown durable; never resend.
    store.save(&op, Some(op.version)).await?;
    Ok(op.public())
}
