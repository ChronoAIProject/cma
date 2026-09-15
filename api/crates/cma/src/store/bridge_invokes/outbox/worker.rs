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
                    self.defer_claim(&item, holder).await?;
                    return Ok(true);
                }
            };
            let sending = self.begin_delivery(&item, holder, &validation).await?;
            let attempt = match &sending.state {
                DeliveryState::Sending { attempt, .. } => attempt.clone(),
                _ => return Err(OutboxError::Corrupt),
            };
            let bridge = match self.read_current_bridge(&sending.source).await {
                Ok(bridge) => bridge,
                Err(_) => {
                    self.complete_delivery(
                        &sending.id,
                        &attempt,
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
            self.complete_delivery(&sending.id, &attempt, outcome)
                .await?;
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

    async fn complete_delivery(
        &self,
        id: &str,
        attempt: &str,
        outcome: DeliveryOutcome,
    ) -> Result<(), OutboxError> {
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
                    .find_one(doc! {"_id":id})
                    .session(&mut session)
                    .await?
                    .ok_or(OutboxError::Conflict)?;
                let mut item = records::read_item(&document)?;
                let revision = item.revision;
                item.complete(attempt, outcome.clone(), now_ms())?;
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
        request.require_current()
    }
}
