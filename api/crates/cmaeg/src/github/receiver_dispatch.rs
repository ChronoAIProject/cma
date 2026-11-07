use super::*;

impl Gateway {
    pub(crate) async fn recover_tick(&self) {
        let now = self.store.current_time();
        let mut cursor = self.recovery_cursor.lock().await;
        let after = cursor.clone();
        let mut pending = match self
            .store
            .list_pending(
                after
                    .as_ref()
                    .map(|(owner, id)| (owner.as_str(), id.as_str())),
                now,
                16,
            )
            .await
        {
            Ok(pending) => pending,
            Err(_) if after.is_some() => {
                *cursor = None;
                match self.store.list_pending(None, now, 16).await {
                    Ok(pending) => pending,
                    Err(_) => return,
                }
            }
            Err(_) => return,
        };
        if pending.is_empty() && after.is_some() {
            *cursor = None;
            pending = match self.store.list_pending(None, now, 16).await {
                Ok(pending) => pending,
                Err(_) => return,
            };
        }
        for (owner, id, _delivery) in pending {
            *cursor = Some((owner.clone(), id.clone()));
            let _ = self.dispatch_pending(&owner, &id).await;
        }
    }

    pub(super) async fn matching_channels(
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
        let mut matches = Vec::new();
        let mut after = None;
        loop {
            let heads = records
                .list(Some(owner), after.as_deref(), 128)
                .await
                .map_err(|_| Error::Unavailable)?;
            if heads.is_empty() {
                break;
            }
            after = heads.last().map(|head| head.record_id.clone());
            for head in heads {
                let url = records
                    .record_url(&head.record_id)
                    .map_err(|_| Error::Unavailable)?;
                let record = match records.get(url.as_str()).await {
                    Ok(record) => record,
                    Err(RecordError::NotFound) => continue,
                    Err(_) => {
                        // A malformed or unavailable unrelated channel cannot grant
                        // authority to this delivery; skip it and continue the
                        // owner-scoped page.
                        tracing::warn!(owner, record_id = %head.record_id, "skipping unreadable channel while matching GitHub delivery");
                        continue;
                    }
                };
                let opened = match open_blob(&ingress.master_keys, &record.blob) {
                    Ok(opened) => opened,
                    Err(_) => {
                        tracing::warn!(owner, record_id = %head.record_id, "skipping malformed channel while matching GitHub delivery");
                        continue;
                    }
                };
                let definition: cmaeg_core::channel::ChannelDefinition =
                    match serde_json::from_slice(&opened.payload) {
                        Ok(definition) => definition,
                        Err(_) => continue,
                    };
                if definition
                    .creation
                    .as_ref()
                    .is_some_and(|creation| !creation.ready)
                {
                    continue;
                }
                if definition.owner.subject != owner
                    || definition.record_id.as_str() != head.record_id
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
                let channel_id = match definition.creation.as_ref() {
                    Some(creation) => creation.channel_id.clone(),
                    None => {
                        let record_id = RecordId::new(head.record_id.clone())
                            .map_err(|_| Error::Unavailable)?;
                        ChannelPointer {
                            v: 1,
                            record_id,
                            record_url: url.to_string(),
                        }
                        .seal_id(&ingress.master_keys)
                        .map_err(|_| Error::Unavailable)?
                    }
                };
                let channel = match ingress.resolver.resolve_current(channel_id.as_str()).await {
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
        }
        matches.sort_by(|left, right| left.channel_id.cmp(&right.channel_id));
        Ok(matches)
    }

    pub(crate) async fn dispatch_pending(
        &self,
        owner: &str,
        receipt_id: &str,
    ) -> Result<Receipt, Error> {
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
        let mut authority_retry = false;
        let authority = match self.read_app(owner, &delivery.receipt.app_ref).await {
            Ok(app) if app.status == "verified" => match self
                .installation_binding(
                    owner,
                    &app,
                    delivery.receipt.installation_id.ok_or(Error::Unavailable)?,
                )
                .await
            {
                Ok(binding)
                    if binding.status == "active"
                        && delivery.receipt.repository_id.is_some_and(|id| {
                            binding.repository_ids.contains(&id)
                                && !binding.excluded_repository_ids.contains(&id)
                        }) =>
                {
                    Some((app, binding))
                }
                Err(crate::github::Error::NotFound) => None,
                Err(_) => {
                    authority_retry = true;
                    None
                }
                Ok(_) => None,
            },
            Err(crate::github::Error::NotFound | crate::github::Error::Retired) => None,
            Err(_) => {
                authority_retry = true;
                None
            }
            Ok(_) => None,
        };
        if authority_retry || authority.is_none() {
            for channel_state in &mut dispatch.channels {
                if channel_state.status == "pending" {
                    if authority_retry {
                        channel_state.attempts = channel_state.attempts.saturating_add(1);
                        channel_state.next_attempt_at =
                            now.saturating_add(backoff(channel_state.attempts));
                        channel_state.reason = Some("github_authority_unavailable".into());
                    } else {
                        channel_state.status = "refused".into();
                        channel_state.reason =
                            Some("github_authority_tightened_before_admission".into());
                    }
                }
            }
        }
        // One bounded tick handles at most 16 channels. The owner/kind/_id
        // index scopes recovery; no process scans another owner's deliveries.
        for channel_state in dispatch.channels.iter_mut().take(16) {
            if authority_retry || authority.is_none() {
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
                Err(
                    crate::channels::resolver::ResolveError::NotFound
                    | crate::channels::resolver::ResolveError::Tampered,
                ) => {
                    channel_state.status = "refused".into();
                    channel_state.reason = Some("channel_unavailable_or_removed".into());
                    continue;
                }
                Err(_) => {
                    channel_state.next_attempt_at =
                        now.saturating_add(backoff(channel_state.attempts));
                    channel_state.reason = Some("channel_restore_unavailable".into());
                    continue;
                }
            };
            let source = SourceKind::Github {
                app_ref: delivery.receipt.app_ref.clone(),
                installation_id: dispatch.projection.installation_id,
                repository_id: dispatch.projection.repository.id,
            };
            if channel.definition.owner.subject != owner
                || channel.definition.source_of(&source).is_none_or(|source| {
                    !source.github_accepts(
                        &dispatch.projection.event,
                        dispatch.projection.action.as_deref(),
                    )
                })
            {
                channel_state.status = "refused".into();
                channel_state.reason = Some("channel_changed_before_admission".into());
                continue;
            }
            channel_state.channel_version = channel.version;
            let request = projection::github_launch_request(
                channel,
                &delivery.receipt.app_ref,
                &dispatch.projection,
                &ingress.master_keys,
                delivery.receipt.received_at,
            )?;
            let Some((app, binding)) = authority.as_ref() else {
                continue;
            };
            let fence = crate::ingress::routed::github::GithubAuthorityFence {
                owner: owner.to_owned(),
                integration_id: app.app_ref.clone(),
                integration_version: app.version,
                binding_id: binding.binding_id.clone(),
                binding_version: binding.version,
            };
            match ingress
                .launch_github(request, projection_bytes.clone(), fence)
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
                    channel_state.reason = Some(
                        problem
                            .detail
                            .clone()
                            .unwrap_or_else(|| "routed_admission_refused".into()),
                    );
                    if retryable_problem(&problem) {
                        channel_state.status = "pending".into();
                        channel_state.next_attempt_at =
                            now.saturating_add(backoff(channel_state.attempts));
                    } else {
                        channel_state.status = "refused".into();
                    }
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

    pub(crate) async fn receipt(&self, owner: &str, id: &str) -> Result<Receipt, Error> {
        let (_, delivery): (u64, Delivery) = self.store.read_owned(owner, id, "delivery").await?;
        if delivery.receipt.receipt_id != id {
            return Err(Error::Unavailable);
        }
        Ok(delivery.receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::super::retryable_problem;
    use cmaeg_core::problem::{Problem, ProblemCode};

    #[test]
    fn provider_refusals_retry_but_definitive_route_refusals_settle() {
        assert!(retryable_problem(&Problem::new(
            ProblemCode::ProviderUnavailable,
            503,
            "provider unavailable",
        )));
        assert!(retryable_problem(&Problem::new(
            ProblemCode::RateLimited,
            429,
            "busy",
        )));
        assert!(!retryable_problem(&Problem::new(
            ProblemCode::AdmissionRefused,
            422,
            "route removed",
        )));
    }
}
