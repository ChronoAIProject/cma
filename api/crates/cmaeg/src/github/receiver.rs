use super::model::DispatchProgress;
use super::*;
use crate::records::RecordError;
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    cloudevent::CloudEvent,
    ids::{compact_partition_key, ConversationId, EventId, IdempotencyKey, RecordId},
    seal::{
        event_ref::activity_digest, open_blob, ChannelPointer, EventRef, EventRefPayload,
        ReplyAddress,
    },
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
impl Gateway {
    pub(super) async fn receive(
        &self,
        app_ref: &str,
        delivery_id: &str,
        event: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<(Receipt, bool), Error> {
        retry::run(|| self.receive_once(app_ref, delivery_id, event, signature, body)).await
    }
    async fn receive_once(
        &self,
        app_ref: &str,
        delivery_id: &str,
        event: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<(Receipt, bool), Error> {
        let (integration_owner, app, webhook_secrets) = self.webhook_credentials(app_ref).await?;
        let hex = signature
            .strip_prefix("sha256=")
            .filter(|value| value.len() == 64)
            .ok_or(Error::Signature)?;
        let tag: Vec<u8> = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).map_err(|_| Error::Signature)?;
                u8::from_str_radix(pair, 16).map_err(|_| Error::Signature)
            })
            .collect::<Result<_, _>>()?;
        if !webhook_secrets.iter().any(|secret| {
            ring::hmac::verify(
                &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_str().as_bytes()),
                body,
                &tag,
            )
            .is_ok()
        }) {
            return Err(Error::Signature);
        }
        let receipt_id = identity("delivery", &(app_ref, delivery_id));
        let fingerprint = format!("{:x}", Sha256::digest(body));
        match self.store.read::<Delivery>(&receipt_id, "delivery").await {
            Ok((_, delivery)) => {
                if delivery.receipt.event != event || delivery.receipt.body_sha256 != fingerprint {
                    return Err(Error::Conflict);
                }
                if delivery.dispatch.as_ref().is_some_and(|dispatch| {
                    dispatch
                        .channels
                        .iter()
                        .any(|channel| channel.status == "pending")
                }) {
                    return Ok((
                        self.dispatch_pending(&integration_owner, &receipt_id)
                            .await?,
                        false,
                    ));
                }
                return Ok((delivery.receipt, false));
            }
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let payload: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| Error::Invalid)?;
        let received_at = self.store.current_time();
        if event == "ping" {
            if !payload.get("zen").is_some_and(serde_json::Value::is_string)
                || payload
                    .get("hook_id")
                    .and_then(serde_json::Value::as_u64)
                    .is_none()
                || !payload
                    .get("hook")
                    .is_some_and(serde_json::Value::is_object)
                || payload.get("action").is_some()
            {
                return Err(Error::Invalid);
            }
            let receipt = Receipt {
                receipt_id,
                app_ref: app_ref.into(),
                installation_id: None,
                delivery_id: delivery_id.into(),
                repository_id: None,
                event: event.into(),
                action: None,
                status: "ignored".into(),
                reason: None,
                received_at,
                raw_body_expires_at: received_at + 86400,
                retained_until: Some(received_at + 7 * 86400),
                body_sha256: fingerprint,
                task_ids: Vec::new(),
                authority_revision: 0,
                dispatch: None,
            };
            return self
                .persist(&integration_owner, &app, receipt, body, None)
                .await;
        }
        let installation_id = payload
            .pointer("/installation/id")
            .and_then(serde_json::Value::as_u64)
            .ok_or(Error::Invalid)?;
        let ignored = |reason: &str, authority_revision: u64| Receipt {
            receipt_id: receipt_id.clone(),
            app_ref: app_ref.into(),
            installation_id: Some(installation_id),
            delivery_id: delivery_id.into(),
            repository_id: payload
                .pointer("/repository/id")
                .and_then(serde_json::Value::as_u64),
            event: event.into(),
            action: payload
                .get("action")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            status: "ignored".into(),
            reason: Some(reason.into()),
            received_at,
            raw_body_expires_at: received_at,
            retained_until: Some(received_at + 7 * 86400),
            body_sha256: fingerprint.clone(),
            task_ids: Vec::new(),
            authority_revision,
            dispatch: None,
        };
        let owner = integration_owner.clone();
        let mut binding = match self
            .installation_binding(&owner, &app, installation_id)
            .await
        {
            Ok(value) => value,
            Err(Error::NotFound) => {
                return self
                    .persist(
                        &integration_owner,
                        &app,
                        ignored("installation_unbound", 0),
                        &[],
                        None,
                    )
                    .await
            }
            Err(error) => return Err(error),
        };
        if matches!(
            event,
            "installation" | "installation_repositories" | "repository"
        ) {
            return self
                .installation(
                    &owner,
                    binding,
                    LifecycleDelivery {
                        integration: &app,
                        received_at,
                        receipt_id,
                        delivery_id,
                        fingerprint: &fingerprint,
                        event,
                        repository_id: if event == "repository" {
                            payload
                                .pointer("/repository/id")
                                .and_then(serde_json::Value::as_u64)
                        } else {
                            None
                        },
                        payload: &payload,
                        body,
                    },
                )
                .await;
        }
        let repository_id = payload
            .pointer("/repository/id")
            .and_then(serde_json::Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or(Error::Invalid)?;
        let repository_owner = payload
            .pointer("/repository/owner/id")
            .and_then(serde_json::Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or(Error::Invalid)?;
        if repository_owner != binding.account_id {
            let expected = binding.version;
            let eligible =
                binding.status == "active" && binding.repository_ids.contains(&repository_id);
            binding.exclude_repository(repository_id);
            binding.version = binding.version.checked_add(1).ok_or(Error::Unavailable)?;
            if eligible {
                binding.authority_revision = binding
                    .authority_revision
                    .checked_add(1)
                    .ok_or(Error::Unavailable)?;
            }
            return self
                .persist(
                    &owner,
                    &app,
                    ignored("repository_owner_changed", binding.authority_revision),
                    &[],
                    Some((&binding, expected)),
                )
                .await;
        }
        if binding.status != "active" {
            let reason = match binding.status.as_str() {
                "pending_verification" => "installation_pending_verification",
                "suspended" => "installation_suspended",
                "uninstalled" => "installation_uninstalled",
                _ => return Err(Error::Unavailable),
            };
            return self
                .persist(
                    &owner,
                    &app,
                    ignored(reason, binding.authority_revision),
                    &[],
                    None,
                )
                .await;
        }
        if !binding.repository_ids.contains(&repository_id)
            || binding.excluded_repository_ids.contains(&repository_id)
        {
            return self
                .persist(
                    &owner,
                    &app,
                    ignored("repository_not_approved", binding.authority_revision),
                    &[],
                    None,
                )
                .await;
        }
        let action = payload.get("action").and_then(serde_json::Value::as_str);
        if (payload.get("issue").is_some() && payload.get("comment").is_none() && event != "issues")
            || (payload.get("pull_request").is_some()
                && payload.get("review").is_none()
                && payload.get("comment").is_none()
                && payload.get("thread").is_none()
                && event != "pull_request")
        {
            return Err(Error::Invalid);
        }
        let mut supported = false;
        if matches!(event, "issues" | "pull_request") {
            let action = action
                .filter(|action| valid_ref(action))
                .ok_or(Error::Invalid)?;
            if payload.get("comment").is_some()
                || payload.get("review").is_some()
                || payload.get("thread").is_some()
            {
                return Err(Error::Invalid);
            }
            let object = match event {
                "issues" if payload.get("pull_request").is_none() => payload.get("issue"),
                "pull_request" if payload.get("issue").is_none() => payload.get("pull_request"),
                _ => None,
            }
            .ok_or(Error::Invalid)?;
            if object
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .is_none_or(|id| id == 0)
                || object
                    .get("number")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|number| number == 0)
            {
                return Err(Error::Invalid);
            }
            supported = matches!(
                action,
                "opened"
                    | "edited"
                    | "closed"
                    | "reopened"
                    | "labeled"
                    | "unlabeled"
                    | "assigned"
                    | "unassigned"
                    | "milestoned"
                    | "demilestoned"
                    | "locked"
                    | "unlocked"
            ) || (event == "issues"
                && matches!(
                    action,
                    "deleted" | "transferred" | "pinned" | "unpinned" | "typed" | "untyped"
                ))
                || (event == "pull_request"
                    && matches!(
                        action,
                        "synchronize"
                            | "ready_for_review"
                            | "converted_to_draft"
                            | "review_requested"
                            | "review_request_removed"
                            | "auto_merge_enabled"
                            | "auto_merge_disabled"
                            | "enqueued"
                            | "dequeued"
                    ));
        }
        if supported {
            let projection = projection::compact_projection(
                &payload,
                app_ref,
                delivery_id,
                event,
                action,
                installation_id,
            )?;
            let channels = self
                .matching_channels(
                    &integration_owner,
                    &app,
                    installation_id,
                    repository_id,
                    event,
                    action,
                )
                .await?;
            if channels.is_empty() {
                let mut receipt = ignored("no_matching_channel", binding.authority_revision);
                receipt.raw_body_expires_at = received_at + 86400;
                receipt.retained_until = Some(received_at + 7 * 86400);
                return self.persist(&owner, &app, receipt, body, None).await;
            }
            let dispatch = DispatchState {
                schema_version: 1,
                projection,
                channels,
                lease: None,
            };
            let receipt = Receipt {
                receipt_id,
                app_ref: app_ref.into(),
                installation_id: Some(installation_id),
                delivery_id: delivery_id.into(),
                repository_id: Some(repository_id),
                event: event.into(),
                action: action.map(str::to_owned),
                status: "pending".into(),
                reason: None,
                received_at,
                raw_body_expires_at: received_at + 86400,
                // A pending receipt may commit an admission on the last day of
                // this window; retain the receipt for that admission's full
                // seven-day execution horizon.
                retained_until: Some(received_at + 14 * 86400),
                body_sha256: fingerprint.clone(),
                task_ids: Vec::new(),
                authority_revision: binding.authority_revision,
                dispatch: Some(
                    dispatch
                        .channels
                        .iter()
                        .map(DispatchProgress::from)
                        .collect(),
                ),
            };
            let delivery = Delivery {
                receipt: receipt.clone(),
                raw_body_records: self.store.source(&owner, &receipt, body).await?,
                dispatch: Some(dispatch),
            };
            if let Err(error) = self.store.create_delivery(&owner, &delivery).await {
                if !matches!(error, Error::Conflict) {
                    return Err(error);
                }
                let (_, committed): (u64, Delivery) = self
                    .store
                    .read_owned(&owner, &receipt.receipt_id, "delivery")
                    .await?;
                return Ok((committed.receipt, false));
            }
            let receipt = self.dispatch_pending(&owner, &receipt.receipt_id).await?;
            return Ok((receipt, true));
        }
        let (status, task_ids) = ("ignored", Vec::new());
        let receipt = Receipt {
            receipt_id,
            app_ref: app_ref.into(),
            installation_id: Some(installation_id),
            delivery_id: delivery_id.into(),
            repository_id: Some(repository_id),
            event: event.into(),
            action: action.map(str::to_owned),
            status: status.into(),
            reason: Some("event_not_supported".into()),
            received_at,
            raw_body_expires_at: received_at + 86400,
            retained_until: Some(received_at + 7 * 86400),
            body_sha256: fingerprint.clone(),
            task_ids,
            authority_revision: binding.authority_revision,
            dispatch: None,
        };
        self.persist(&owner, &app, receipt, body, None).await
    }
    pub(super) async fn persist(
        &self,
        owner: &str,
        integration: &AppIntegration,
        receipt: Receipt,
        body: &[u8],
        authority: Option<(&Binding, u64)>,
    ) -> Result<(Receipt, bool), Error> {
        let delivery = Delivery {
            receipt: receipt.clone(),
            raw_body_records: self.store.source(owner, &receipt, body).await?,
            dispatch: None,
        };
        let committed = match authority {
            Some((binding, expected_version)) => {
                self.store
                    .commit_with_authority(owner, binding, integration, expected_version, &delivery)
                    .await
            }
            None => {
                // This receipt cannot authorize work. A read observes retirement;
                // it must not serialize unrelated inert events on authority rows.
                if self.read_app(owner, &integration.app_ref).await?.status == "retired" {
                    return Err(Error::Retired);
                }
                self.store
                    .create(&receipt.receipt_id, owner, "delivery", &delivery)
                    .await
            }
        };
        if let Err(error) = committed {
            return match self
                .store
                .read::<Delivery>(&receipt.receipt_id, "delivery")
                .await
            {
                Ok((_, committed)) => duplicate(committed, &receipt.event, &receipt.body_sha256),
                Err(Error::NotFound) if matches!(error, Error::Retry) => Err(Error::Retry),
                Err(_) => Err(Error::Unavailable),
            };
        }
        Ok((receipt, true))
    }
}
impl From<&DispatchChannel> for DispatchProgress {
    fn from(value: &DispatchChannel) -> Self {
        Self {
            channel_id: value.channel_id.clone(),
            status: value.status.clone(),
            reason: value.reason.clone(),
            admission_id: value.admission_id.clone(),
        }
    }
}
fn backoff(attempts: u32) -> i64 {
    5_i64.saturating_mul(1_i64 << attempts.min(8))
}

fn retryable_problem(problem: &cmaeg_core::problem::Problem) -> bool {
    problem.status >= 500
        || problem.status == 429
        || matches!(
            problem.code,
            cmaeg_core::problem::ProblemCode::ProviderUnavailable
                | cmaeg_core::problem::ProblemCode::ProviderBusy
                | cmaeg_core::problem::ProblemCode::Draining
                | cmaeg_core::problem::ProblemCode::RateLimited
        )
}

#[path = "receiver_dispatch.rs"]
mod dispatch;
#[path = "receiver_projection.rs"]
mod projection;
