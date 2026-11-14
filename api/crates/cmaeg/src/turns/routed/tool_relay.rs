//! A single authenticated hop to the current front-target owner. A transport
//! timeout is uncertainty, never permission to issue another external send.
pub(super) mod codec;
#[cfg(test)]
mod codec_tests;
#[cfg(test)]
mod mongo_tests;

use super::{storage, ExecutionError, RoutedToolReplies};
use crate::{
    auth::AgentCaller,
    delivery::intents::IntentReceipt,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::partition::PartitionFence,
};
use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRef, EventRefPayload},
};
use codec::{Direction, Envelope};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tower_http::limit::RequestBodyLimitLayer;

const PATH: &str = "/internal/replicas/tool-reply";
const CONTEXT: &str = "POST /internal/replicas/tool-reply";
const REQUEST_WINDOW: i64 = 24;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    caller_key_hash: [u8; 32],
    event: EventRef,
    message: OutboundMessage,
    idempotency_key: Option<String>,
    expected: PartitionFence,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
enum Outcome {
    Receipt(Box<IntentReceipt>),
    Refused(Refusal),
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Refusal {
    Invalid,
    Revoked,
    Busy,
    Expired,
    Conflict,
    Limit,
    LostLease,
    Unavailable,
}
impl From<ExecutionError> for Refusal {
    fn from(error: ExecutionError) -> Self {
        match error {
            ExecutionError::Invalid | ExecutionError::Unsupported => Self::Invalid,
            ExecutionError::AuthorityChanged => Self::Revoked,
            ExecutionError::Busy => Self::Busy,
            ExecutionError::Expired => Self::Expired,
            ExecutionError::IdempotencyConflict => Self::Conflict,
            ExecutionError::OperationLimit => Self::Limit,
            ExecutionError::LostLease => Self::LostLease,
            _ => Self::Unavailable,
        }
    }
}
impl From<Refusal> for ExecutionError {
    fn from(error: Refusal) -> Self {
        match error {
            Refusal::Invalid => Self::Invalid,
            Refusal::Revoked => Self::AuthorityChanged,
            Refusal::Busy => Self::Busy,
            Refusal::Expired => Self::Expired,
            Refusal::Conflict => Self::IdempotencyConflict,
            Refusal::Limit => Self::OperationLimit,
            Refusal::LostLease => Self::LostLease,
            Refusal::Unavailable => Self::Uncertain,
        }
    }
}

impl RoutedToolReplies {
    pub fn with_replica_relay(mut self) -> Result<Self, ExecutionError> {
        if self.journal.partitions.is_none() {
            return Err(ExecutionError::Invalid);
        }
        self.replica_relay = Some(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(24))
                .build()
                .map_err(|_| ExecutionError::Unavailable)?,
        );
        Ok(self)
    }
    pub fn replica_relay_router(self: Arc<Self>) -> Router {
        Router::new()
            .route(
                PATH,
                post(receive).layer(RequestBodyLimitLayer::new(codec::BODY_LIMIT)),
            )
            .route(
                super::stream_relay::PATH,
                post(super::stream_relay::receive)
                    .layer(RequestBodyLimitLayer::new(codec::STREAM_BODY_LIMIT)),
            )
            .with_state(self)
    }
    pub(super) async fn forward_reply(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        message: OutboundMessage,
        idempotency_key: Option<&str>,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<IntentReceipt, ExecutionError> {
        let client = self
            .replica_relay
            .as_ref()
            .ok_or(ExecutionError::Unavailable)?;
        // The origin is obtained from authenticated membership only. Revalidate
        // its URL shape defensively and never follow a redirect or environment proxy.
        let owner = self
            .journal
            .partition_owner(plan, target)
            .await?
            .ok_or(ExecutionError::LostLease)?;
        let origin = crate::replicas::config::validate_address(&owner.origin)
            .map_err(|_| ExecutionError::Corrupt)?;
        let now = (self.journal.clock)();
        let id = uuid::Uuid::new_v4().to_string();
        let envelope = codec::seal(
            &self.journal.keys,
            Direction::Request,
            &id,
            CONTEXT,
            now.saturating_add(REQUEST_WINDOW),
            now,
            &Request {
                caller_key_hash: caller.key_hash,
                event: EventRef::seal(event, &self.journal.keys)
                    .map_err(|_| ExecutionError::Invalid)?,
                message,
                idempotency_key: idempotency_key.map(str::to_owned),
                expected: owner.fence,
            },
        )?;
        let bytes = storage::bytes(&envelope)?;
        let request_hash = storage::hash(&bytes);
        let mut response = client
            .post(format!("{}{PATH}", origin.trim_end_matches('/')))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| ExecutionError::Uncertain)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > codec::BODY_LIMIT as u64)
        {
            return Err(ExecutionError::Uncertain);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ExecutionError::Uncertain)?
        {
            if bytes.len().saturating_add(chunk.len()) > codec::BODY_LIMIT {
                return Err(ExecutionError::Uncertain);
            }
            bytes.extend_from_slice(&chunk);
        }
        let envelope: Envelope =
            serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Uncertain)?;
        if envelope.id != id {
            return Err(ExecutionError::Uncertain);
        }
        let outcome = codec::open(
            &self.journal.keys,
            Direction::Response,
            &request_hash,
            (self.journal.clock)(),
            &envelope,
        )
        .map_err(|_| ExecutionError::Uncertain)?;
        metrics::counter!("cmaeg_replica_tool_relay_total", "outcome" => match &outcome {
            Outcome::Receipt(_) => "receipt", Outcome::Refused(_) => "refused",
        })
        .increment(1);
        match outcome {
            Outcome::Receipt(receipt) => Ok(*receipt),
            Outcome::Refused(error) => Err(error.into()),
        }
    }
}

async fn receive(
    State(replies): State<Arc<RoutedToolReplies>>,
    bytes: Bytes,
) -> Result<Bytes, StatusCode> {
    if replies.replica_relay.is_none() || replies.journal.partitions.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    let now = (replies.journal.clock)();
    let request: Request = codec::open(
        &replies.journal.keys,
        Direction::Request,
        CONTEXT,
        now,
        &envelope,
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let event = request
        .event
        .open(&replies.journal.keys, now)
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .payload;
    let result = replies
        .reply_inner(
            AgentCaller {
                key_hash: request.caller_key_hash,
            },
            &event,
            request.message,
            false,
            request.idempotency_key.as_deref(),
            Some(request.expected),
        )
        .await;
    let outcome = match result {
        Ok(Some(receipt)) => Outcome::Receipt(Box::new(receipt)),
        Ok(None) => Outcome::Refused(Refusal::Invalid),
        Err(error) => {
            tracing::warn!(
                ?error,
                "replica tool relay refused; no relay retry authorized"
            );
            Outcome::Refused(error.into())
        }
    };
    let now = (replies.journal.clock)();
    let response = codec::seal(
        &replies.journal.keys,
        Direction::Response,
        &envelope.id,
        &storage::hash(&bytes),
        now.saturating_add(REQUEST_WINDOW),
        now,
        &outcome,
    )
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    storage::bytes(&response)
        .map(Bytes::from)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
