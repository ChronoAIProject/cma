use super::model::DispatchProgress;
use super::*;

use crate::records::RecordError;
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    cloudevent::CloudEvent,
    ids::{compact_partition_key, ConversationId, EventId, IdempotencyKey},
    seal::{event_ref::activity_digest, open_blob, EventRef, EventRefPayload, ReplyAddress},
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

impl Gateway {
    pub(super) async fn recover_tick(&self) {
        let Ok(owners) = self.store.integration_owners(64).await else {
            return;
        };
        for owner in owners.into_iter().take(4) {
            let Ok(deliveries) = self
                .store
                .list_owner::<Delivery>(&owner, "delivery", None, 64)
                .await
            else {
                continue;
            };
            for (id, _, delivery) in deliveries.into_iter().take(16) {
                if delivery.dispatch.as_ref().is_some_and(|dispatch| {
                    dispatch
                        .channels
                        .iter()
                        .any(|channel| channel.status == "pending")
                }) {
                    let _ = self.dispatch_pending(&owner, &id).await;
                }
            }
        }
    }

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
            let projection = compact_projection(
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

    async fn matching_channels(
        &self,
        owner: &str,
        integration: &AppIntegration,
        installation_id: u64,
        repository_id: u64,
        event: &str,
        action: Option<&str>,
    ) -> Result<Vec<DispatchChannel>, Error> {
        let Some(ingress) = self.ingress.as_ref() else {
            return Err(Error::DispatchUnavailable);
        };
        let records = ingress.resolver.record_client();
        let heads = records
            .list(Some(owner), None, 1000)
            .await
            .map_err(|_| Error::Unavailable)?;
        let mut matches = Vec::new();
        for head in heads {
            let url = records
                .record_url(&head.record_id)
                .map_err(|_| Error::Unavailable)?;
            let record = match records.get(url.as_str()).await {
                Ok(record) => record,
                Err(RecordError::NotFound) => continue,
                Err(_) => return Err(Error::Unavailable),
            };
            let opened =
                open_blob(&ingress.master_keys, &record.blob).map_err(|_| Error::Unavailable)?;
            let definition: cmaeg_core::channel::ChannelDefinition =
                serde_json::from_slice(&opened.payload).map_err(|_| Error::Unavailable)?;
            let Some(creation) = definition
                .creation
                .as_ref()
                .filter(|creation| creation.ready)
            else {
                continue;
            };
            if definition.owner.subject != owner || definition.record_id.as_str() != head.record_id
            {
                continue;
            }
            let source = SourceKind::Github {
                app_ref: integration.app_ref.clone(),
                installation_id,
                repository_id,
            };
            if definition
                .source_of(&source)
                .is_none_or(|source| !source.github_accepts(event, action))
            {
                continue;
            }
            let channel = match ingress
                .resolver
                .resolve_current(creation.channel_id.as_str())
                .await
            {
                Ok(channel) => channel,
                Err(_) => continue,
            };
            // resolve_current re-reads the current version and definition. The
            // snapshot is frozen into the receipt; guarded admission rechecks it.
            if channel.definition.owner.subject != owner
                || channel
                    .definition
                    .source_of(&source)
                    .is_none_or(|source| !source.github_accepts(event, action))
            {
                continue;
            }
            matches.push(DispatchChannel {
                channel_id: channel.id.to_string(),
                record_id: channel.pointer.record_id.to_string(),
                channel_version: channel.version,
                status: "pending".into(),
                reason: None,
                admission_id: None,
                attempts: 0,
                next_attempt_at: self.store.current_time(),
            });
        }
        matches.sort_by(|left, right| left.channel_id.cmp(&right.channel_id));
        Ok(matches)
    }

    async fn dispatch_pending(&self, owner: &str, receipt_id: &str) -> Result<Receipt, Error> {
        let Some(ingress) = self.ingress.as_ref() else {
            return Err(Error::DispatchUnavailable);
        };
        let now = self.store.current_time();
        let Some((mut guard, mut delivery)) =
            self.store.claim_delivery(owner, receipt_id, now).await?
        else {
            return self.receipt(owner, receipt_id).await;
        };
        let projection_bytes = serde_json::to_vec(
            &delivery
                .dispatch
                .as_ref()
                .ok_or(Error::Unavailable)?
                .projection,
        )
        .map_err(|_| Error::Unavailable)?;
        let mut task_ids = delivery.receipt.task_ids.clone();
        let dispatch = delivery.dispatch.as_mut().ok_or(Error::Unavailable)?;
        let eligible = match self.read_app(owner, &delivery.receipt.app_ref).await {
            Ok(app) if app.status == "verified" => self
                .installation_binding(
                    owner,
                    &app,
                    delivery.receipt.installation_id.ok_or(Error::Unavailable)?,
                )
                .await
                .is_ok_and(|binding| {
                    binding.status == "active"
                        && delivery.receipt.repository_id.is_some_and(|id| {
                            binding.repository_ids.contains(&id)
                                && !binding.excluded_repository_ids.contains(&id)
                        })
                }),
            _ => false,
        };
        if !eligible {
            for channel_state in &mut dispatch.channels {
                if channel_state.status == "pending" {
                    channel_state.status = "refused".into();
                    channel_state.reason =
                        Some("github_authority_tightened_before_admission".into());
                }
            }
        }
        // One bounded tick handles at most 16 channels. The owner/kind/_id
        // index scopes recovery; no process scans another owner's deliveries.
        for channel_state in dispatch.channels.iter_mut().take(16) {
            if !eligible {
                continue;
            }
            if channel_state.status != "pending" || channel_state.next_attempt_at > now {
                continue;
            }
            channel_state.attempts = channel_state.attempts.saturating_add(1);
            let channel = match ingress
                .resolver
                .resolve_current(&channel_state.channel_id)
                .await
            {
                Ok(channel) => channel,
                Err(_) => {
                    channel_state.status = "refused".into();
                    channel_state.reason = Some("channel_unavailable_or_removed".into());
                    continue;
                }
            };
            if channel.version != channel_state.channel_version
                || channel.definition.owner.subject != owner
            {
                channel_state.status = "refused".into();
                channel_state.reason = Some("channel_changed_before_admission".into());
                continue;
            }
            let request =
                github_launch_request(channel, &dispatch.projection, &ingress.master_keys, now)?;
            match ingress
                .launch_github(request, projection_bytes.clone())
                .await
            {
                crate::ingress::launch::LaunchOutcome::Admitted { admission_id, .. } => {
                    channel_state.status = "dispatched".into();
                    channel_state.admission_id = Some(admission_id.clone());
                    if !task_ids.contains(&admission_id) {
                        task_ids.push(admission_id);
                    }
                }
                crate::ingress::launch::LaunchOutcome::Refused(problem) => {
                    channel_state.status = "refused".into();
                    channel_state.reason = Some(
                        problem
                            .detail
                            .clone()
                            .unwrap_or_else(|| "routed_admission_refused".into()),
                    );
                }
                _ => {
                    channel_state.status = "pending".into();
                    channel_state.reason = Some("routed_admission_uncertain".into());
                    channel_state.next_attempt_at =
                        now.saturating_add(backoff(channel_state.attempts));
                }
            }
        }
        dispatch.lease = None;
        let has_tasks = !task_ids.is_empty();
        delivery.receipt.task_ids = task_ids;
        let terminal = dispatch
            .channels
            .iter()
            .all(|channel| matches!(channel.status.as_str(), "dispatched" | "refused"));
        delivery.receipt.status = if has_tasks {
            "accepted"
        } else if terminal {
            "failed"
        } else {
            "pending"
        }
        .into();
        delivery.receipt.dispatch = Some(
            dispatch
                .channels
                .iter()
                .map(DispatchProgress::from)
                .collect(),
        );
        guard = self
            .store
            .update_delivery(owner, receipt_id, guard, &delivery)
            .await?;
        self.store
            .release_delivery(owner, receipt_id, guard)
            .await?;
        Ok(delivery.receipt)
    }

    pub(super) async fn receipt(&self, owner: &str, id: &str) -> Result<Receipt, Error> {
        let (_, delivery): (u64, Delivery) = self.store.read_owned(owner, id, "delivery").await?;
        if delivery.receipt.receipt_id != id {
            return Err(Error::Unavailable);
        }
        Ok(delivery.receipt)
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

fn compact_projection(
    payload: &serde_json::Value,
    _app_ref: &str,
    delivery_id: &str,
    event: &str,
    action: Option<&str>,
    installation_id: u64,
) -> Result<GithubProjection, Error> {
    let object = match event {
        "issues" => payload.get("issue"),
        "pull_request" => payload.get("pull_request"),
        _ => None,
    }
    .and_then(serde_json::Value::as_object)
    .ok_or(Error::Invalid)?;
    let repository = payload
        .get("repository")
        .and_then(serde_json::Value::as_object)
        .ok_or(Error::Invalid)?;
    let sender = payload
        .get("sender")
        .and_then(serde_json::Value::as_object)
        .ok_or(Error::Invalid)?;
    let string = |value: Option<&serde_json::Value>| {
        value.and_then(serde_json::Value::as_str).map(str::to_owned)
    };
    Ok(GithubProjection {
        delivery_id: delivery_id.into(),
        event: event.into(),
        action: action.map(str::to_owned),
        installation_id,
        repository: GithubRepository {
            id: repository
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or(Error::Invalid)?,
            full_name: string(repository.get("full_name")).ok_or(Error::Invalid)?,
        },
        number: object
            .get("number")
            .and_then(serde_json::Value::as_u64)
            .ok_or(Error::Invalid)?,
        html_url: string(object.get("html_url")).ok_or(Error::Invalid)?,
        sender: GithubSender {
            login: string(sender.get("login")).ok_or(Error::Invalid)?,
            r#type: string(sender.get("type")),
        },
        state: string(object.get("state")),
        merged: object.get("merged").and_then(serde_json::Value::as_bool),
        draft: object.get("draft").and_then(serde_json::Value::as_bool),
    })
}

fn github_launch_request(
    channel: Arc<crate::channels::resolver::ResolvedChannel>,
    projection: &GithubProjection,
    keys: &cmaeg_core::seal::MasterKeyRing,
    now: i64,
) -> Result<crate::ingress::launch::LaunchRequest, Error> {
    let source = SourceKind::Github {
        app_ref: channel
            .definition
            .sources
            .iter()
            .find_map(|source| match source {
                cmaeg_core::channel::Source::Github {
                    app_ref,
                    installation_ids,
                    repository_ids,
                    deny_repository_ids,
                    ..
                } if installation_ids.contains(&projection.installation_id)
                    && repository_ids.contains(&projection.repository.id)
                    && !deny_repository_ids.contains(&projection.repository.id) =>
                {
                    Some(app_ref.clone())
                }
                _ => None,
            })
            .ok_or(Error::Unavailable)?,
        installation_id: projection.installation_id,
        repository_id: projection.repository.id,
    };
    let event_id = EventId::new(projection.delivery_id.clone()).map_err(|_| Error::Invalid)?;
    let channel_key = channel.id.to_string();
    let conversation_id = ConversationId::derive(
        keys,
        &channel.id,
        channel.definition.partition.version,
        "github-channel",
    );
    let data = serde_json::to_value(projection).map_err(|_| Error::Unavailable)?;
    let ce = CloudEvent {
        specversion: "1.0".into(),
        id: projection.delivery_id.clone(),
        source: format!(
            "github://app/{}",
            match &source {
                SourceKind::Github { app_ref, .. } => app_ref,
                _ => unreachable!(),
            }
        ),
        r#type: format!("com.github.{}", projection.event),
        time: Some(
            time::OffsetDateTime::from_unix_timestamp(now)
                .map_err(|_| Error::Unavailable)?
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| Error::Unavailable)?,
        ),
        subject: Some(format!(
            "repository:{}/item:{}",
            projection.repository.id, projection.number
        )),
        datacontenttype: Some("application/json".into()),
        dataschema: None,
        data: Some(data),
        extensions: BTreeMap::new(),
    };
    let activity = Activity {
        thread: None,
        schema_version: 1,
        event_id: event_id.clone(),
        kind: ActivityKind::Event {
            ce,
            attachments: Vec::new(),
        },
        actor: Actor {
            id: format!("github:{}", projection.sender.login),
            display_name: None,
            kind: ActorKind::System,
        },
        conversation: ActivityConversation {
            id: channel_key,
            kind: ConversationKind::Channel,
            thread_id: None,
        },
        occurred_at: time::OffsetDateTime::from_unix_timestamp(now)
            .map_err(|_| Error::Unavailable)?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Error::Unavailable)?,
        source,
        routing: None,
    };
    activity.validate().map_err(|_| Error::Invalid)?;
    let event_ref = EventRef::seal(
        &EventRefPayload {
            telegram_admission: None,
            v: 1,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: event_id.clone(),
            partition_key: compact_partition_key("github-channel"),
            conversation_id: conversation_id.clone(),
            expires_at: now.saturating_add(7 * 86400),
            reply: ReplyAddress::None,
            consumer: None,
            lineage: None,
        },
        keys,
    )
    .map_err(|_| Error::Unavailable)?;
    Ok(crate::ingress::launch::LaunchRequest {
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at: Instant::now(),
        trace: tracing::Span::current(),
        timing: None,
        consumer: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{backoff, compact_projection};
    use serde_json::json;

    #[test]
    fn projection_drops_issue_title_and_body_before_template_rendering() {
        let payload = json!({
            "issue": {"id": 9, "number": 4, "html_url": "https://github.test/i/4", "state": "open", "title": "title-sentinel", "body": "body-sentinel"},
            "repository": {"id": 42, "full_name": "org/repo"},
            "sender": {"login": "octocat", "type": "User"}
        });
        let projection =
            compact_projection(&payload, "app", "delivery", "issues", Some("opened"), 7).unwrap();
        let encoded = serde_json::to_string(&projection).unwrap();
        assert!(!encoded.contains("title-sentinel"));
        assert!(!encoded.contains("body-sentinel"));
        assert!(encoded.contains("org/repo"));
    }

    #[test]
    fn dispatch_backoff_is_bounded() {
        assert_eq!(backoff(0), 5);
        assert_eq!(backoff(8), 1280);
        assert_eq!(backoff(99), 1280);
    }
}
