use super::*;

impl MongoBridgeReplyToolStore {
    /// One bounded scan/attempt. All IO is behind the supplied NyxID-only port;
    /// callers can test crash/commit behavior without sending platform traffic.
    pub(crate) async fn deliver_one(
        &self,
        holder: &str,
        port: &dyn RelayDeliveryPort,
    ) -> Result<bool, OutboxError> {
        let documents = self.scan(holder).await?;
        for document in documents {
            // A corrupt retained item is reported but cannot pin the scan on
            // itself forever and starve unrelated original responses.
            let id = document.get_str("_id").map_err(|_| OutboxError::Corrupt)?;
            self.advance_scan(holder, id).await?;
            let candidate = records::read_item(&document)?;
            let item = match self.claim_item(&candidate.id, holder).await {
                Ok(Some(item)) => item,
                Ok(None) => continue,
                Err(
                    OutboxError::Conflict
                    | OutboxError::PredecessorPending
                    | OutboxError::Expired
                    | OutboxError::Suspended,
                ) => {
                    self.delay_candidate(&candidate, 1000).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let validation = match self
                .validator
                .validate_connection(item.source.owner(), item.source.config())
                .await
            {
                Ok(value) => value,
                // Positive no-send evidence while checking authority. Backoff
                // is durable and bounded; a provider outage cannot busy-loop.
                Err(_) => {
                    self.defer_claim(&item, holder, DeliveryFailure::ConnectUnavailable)
                        .await?;
                    return Ok(true);
                }
            };
            let sending = match self.begin_delivery(&item, holder, &validation).await {
                Ok(sending) => sending,
                Err(error) => {
                    let reason = if matches!(error, OutboxError::Unavailable) {
                        DeliveryFailure::ConnectUnavailable
                    } else {
                        DeliveryFailure::RouteUnavailable
                    };
                    // Exact Claimed CAS plus rate release share a transaction.
                    // An uncertain Sending commit cannot match this revision,
                    // so its unknown-send boundary and lease remain intact.
                    match self.defer_claim(&item, holder, reason).await {
                        Ok(()) => return Ok(true),
                        Err(OutboxError::Conflict) => return Err(error),
                        Err(failure) => return Err(failure),
                    }
                }
            };
            let bridge = match self.read_current_bridge(&sending.source).await {
                Ok(bridge) => bridge,
                Err(_) => {
                    self.complete_delivery(
                        &sending,
                        DeliveryOutcome::DefinitiveRejection(DeliveryFailure::RouteUnavailable),
                    )
                    .await?;
                    return Ok(true);
                }
            };
            let fingerprint = sending.source.fingerprint()?;
            let request = RelayDeliveryRequest::after_sending_commit(
                sending.clone(),
                &bridge,
                validation,
                DeliveryCapabilityObservation {
                    source_fingerprint: fingerprint,
                    original_route_supported: true,
                    edit_supported: matches!(
                        sending.source.config().connector.platform,
                        layer2_core::bridge::BridgePlatform::Lark
                            | layer2_core::bridge::BridgePlatform::Feishu
                    ),
                    deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(5),
                },
                now_ms(),
            );
            let outcome = match request {
                Ok(request) => port.deliver(request).await,
                // Construction failed before a transport request existed.
                Err(OutboxError::Expired) => {
                    DeliveryOutcome::DefinitiveRejection(DeliveryFailure::CredentialExpired)
                }
                Err(_) => DeliveryOutcome::DefinitiveRejection(DeliveryFailure::RouteUnavailable),
            };
            self.complete_delivery(&sending, outcome).await?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) async fn read_current_bridge(
        &self,
        source: &OutboxSource,
    ) -> Result<BridgeRecord, OutboxError> {
        let document = self
            .invokes
            .bridges
            .definitions
            .find_one(doc! {"_id":source.pin().bridge_id.as_str()})
            .await
            .map_err(|_| OutboxError::Unavailable)?
            .ok_or(OutboxError::SourceMismatch)?;
        let bridge = crate::store::bridge_definitions::decode_bridge(&document)
            .map_err(|_| OutboxError::SourceMismatch)?;
        source.require_bridge(&bridge)?;
        Ok(bridge)
    }

    pub(super) async fn claim_item(
        &self,
        id: &str,
        holder: &str,
    ) -> Result<Option<OutboxItem>, OutboxError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result=async {
                let Some(document)=self.items.find_one(doc!{"_id":id}).session(&mut session).await? else {return Ok(None);};
                let mut item=records::read_item(&document)?;
                let previous=item.revision;
                let now=now_ms();
                if matches!(item.state,DeliveryState::Claimed {expires_at_ms,..}|DeliveryState::Sending{expires_at_ms,..} if expires_at_ms<=now) {
                    item.recover_expired(now)?;
                    self.replace_item(previous,&item,&mut session).await?;
                    return Ok(None);
                }
                match item.state {
                    DeliveryState::Pending=>{},
                    DeliveryState::RetryWait{retry_at_ms,..} if retry_at_ms<=now=>{},
                    _=>return Ok(None),
                }
                match item.reject_expired(now) {
                    Ok(()) => {
                        self.replace_item(previous, &item, &mut session).await?;
                        return Ok(None);
                    }
                    Err(OutboxError::Conflict) => {},
                    Err(error) => return Err(error.into()),
                }
                self.validator.check_feature(
                    layer2_core::bridge::BridgeFeature::Outbound,
                    &item.source.config().connector.platform,
                ).map_err(|_| OutboxError::Suspended)?;
                let predecessor=match &item.predecessor {
                    Some(id)=>Some(records::read_item(&self.items.find_one(doc!{"_id":id}).session(&mut session).await?.ok_or(OutboxError::Corrupt)?)?),
                    None=>None,
                };
                let bridge_doc=self.invokes.bridges.definitions.find_one(doc!{"_id":item.source.pin().bridge_id.as_str()}).session(&mut session).await?.ok_or(OutboxError::SourceMismatch)?;
                let bridge=crate::store::bridge_definitions::decode_bridge(&bridge_doc).map_err(|_|OutboxError::Corrupt)?;
                item.source.require_bridge(&bridge)?;
                match item.claim(holder.into(),now,now+30_000,predecessor.as_ref()) {
                    Ok(_)=>{},
                    Err(OutboxError::Expired)=>{
                        item.reject_expired(now)?;
                        self.replace_item(previous,&item,&mut session).await?;
                        return Ok(None);
                    }
                    Err(error)=>return Err(error.into()),
                }
                if let Some(ready_at) = self.reserve_rate(&item, now, &mut session).await? {
                    let delayed = self.items.update_one(
                        doc! {"_id":&item.id,"revision":previous as i64},
                        doc! {"$set":{"next_check_at_ms":ready_at.min(now.saturating_add(60_000))}},
                    ).session(&mut session).await?;
                    if delayed.matched_count != 1 { return Err(OutboxError::Conflict.into()); }
                    return Ok(None);
                }
                self.replace_item(previous,&item,&mut session).await?;
                Ok::<_,MutationError>(Some(item))
            }.await;
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable);
                }
            }
        }
        Err(OutboxError::Unavailable)
    }

    async fn begin_delivery(
        &self,
        claimed: &OutboxItem,
        holder: &str,
        validation: &BridgeValidation,
    ) -> Result<OutboxItem, OutboxError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = async {
                let document = self
                    .items
                    .find_one(doc! {"_id":&claimed.id})
                    .session(&mut session)
                    .await?
                    .ok_or(OutboxError::Conflict)?;
                let mut item = records::read_item(&document)?;
                if item != *claimed {
                    return Err(OutboxError::Conflict.into());
                }
                let bridge_doc = self
                    .invokes
                    .bridges
                    .definitions
                    .find_one(doc! {"_id":item.source.pin().bridge_id.as_str()})
                    .session(&mut session)
                    .await?
                    .ok_or(OutboxError::SourceMismatch)?;
                let bridge = crate::store::bridge_definitions::decode_bridge(&bridge_doc)
                    .map_err(|_| OutboxError::Corrupt)?;
                self.fence_retained_delivery_source(&item.source, &mut session)
                    .await?;
                self.fence_delivery(&bridge, &item.source, validation, &mut session)
                    .await?;
                let attempt = match &item.state {
                    DeliveryState::Claimed { attempt, .. } => attempt.clone(),
                    _ => return Err(OutboxError::Conflict.into()),
                };
                item.begin_send(holder, &attempt, now_ms())?;
                self.replace_item(claimed.revision, &item, &mut session)
                    .await?;
                Ok::<_, MutationError>(item)
            }
            .await;
            let item = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(item),
                CommitOutcome::Retry(_) => continue,
                // Sending might have committed; never issue IO after uncertainty.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable);
                }
            }
        }
        Err(OutboxError::Unavailable)
    }

    pub(super) async fn complete_delivery(
        &self,
        original: &OutboxItem,
        outcome: DeliveryOutcome,
    ) -> Result<(), OutboxError> {
        original.validate()?;
        let DeliveryState::Sending { attempt, .. } = &original.state else {
            return Err(OutboxError::Conflict);
        };
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = async {
                let document = self
                    .items
                    .find_one(doc! {"_id":&original.id})
                    .session(&mut session)
                    .await?
                    .ok_or(OutboxError::Conflict)?;
                let mut item = records::read_item(&document)?;
                item.require_same_output(original)?;
                let revision = item.revision;
                let now = now_ms();
                let retry_at = match outcome {
                    DeliveryOutcome::RateLimitedUnknown { not_before_ms } => Some(not_before_ms),
                    _ => None,
                };
                if item != *original {
                    // Recovery or an explicit manager decision may have
                    // advanced this item while its response was in flight.
                    // Keep that disposition, but retain a real late cooldown
                    // without replacing a newer bot reservation.
                    let observed = original.attempts.last().ok_or(OutboxError::Corrupt)?;
                    if retry_at.is_none()
                        || !item.attempts.iter().any(|retained| {
                            retained.id == observed.id
                                && retained.number == observed.number
                                && retained.started_at_ms == observed.started_at_ms
                                && retained.credential == observed.credential
                        })
                    {
                        return Err(OutboxError::Conflict.into());
                    }
                    return self
                        .finish_rate(original, now, retry_at, &mut session)
                        .await;
                }
                self.finish_rate(original, now, retry_at, &mut session)
                    .await?;
                item.complete(attempt, outcome.clone(), now)?;
                self.replace_item(revision, &item, &mut session).await
            }
            .await;
            match result {
                Ok(()) => {}
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable);
                }
            }
        }
        Err(OutboxError::Unavailable)
    }

    /// Final local transport gate after actual custody has been re-resolved.
    /// The sender must make no request unless this exact attempt still exists.
    pub(crate) async fn require_sending(
        &self,
        request: &RelayDeliveryRequest,
    ) -> Result<(), OutboxError> {
        request.require_current()?;
        let document = self
            .items
            .find_one(doc! {"_id":&request.item().id})
            .await
            .map_err(|_| OutboxError::Unavailable)?
            .ok_or(OutboxError::Conflict)?;
        let current = records::read_item(&document)?;
        if current != *request.item() || !matches!(current.state, DeliveryState::Sending { .. }) {
            return Err(OutboxError::Conflict);
        }
        self.read_current_bridge(&current.source).await?;
        self.require_rate(&current).await?;
        request.require_current()
    }
}
