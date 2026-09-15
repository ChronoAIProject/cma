use super::*;
use crate::bridge::outbox_rate::{BotRateState, RateError, RateLease};
use layer2_core::bridge::BridgePlatform;
use sha2::{Digest, Sha256};

fn key(source: &OutboxSource) -> Result<String, OutboxError> {
    let connector = &source.config().connector;
    let bytes = serde_json::to_vec(&(
        "cma.bridge.outbox.bot-rate.v1",
        connector.base_url.trim_end_matches('/'),
        connector.issuer.trim_end_matches('/'),
        &connector.platform,
        &connector.bot_id,
    ))
    .map_err(|_| OutboxError::Corrupt)?;
    Ok(format!("brr_{:x}", Sha256::digest(bytes)))
}

fn interval(source: &OutboxSource) -> i64 {
    // Conservative CMA ceilings include Telegram's group pacing. These are
    // shared by every Bridge/key using this actual bot, including old pins.
    match source.config().connector.platform {
        BridgePlatform::Telegram | BridgePlatform::TelegramNew => 3_000,
        BridgePlatform::Discord | BridgePlatform::Lark | BridgePlatform::Feishu => 1_000,
    }
}

fn lease(item: &OutboxItem) -> Result<RateLease, OutboxError> {
    match &item.state {
        DeliveryState::Claimed {
            attempt,
            expires_at_ms,
            ..
        }
        | DeliveryState::Sending {
            attempt,
            expires_at_ms,
        } => Ok(RateLease {
            item: item.id.clone(),
            attempt: attempt.clone(),
            expires_at_ms: *expires_at_ms,
        }),
        _ => Err(OutboxError::Conflict),
    }
}

fn read(document: Option<Document>) -> Result<(i64, BotRateState), OutboxError> {
    let Some(document) = document else {
        return Ok((0, BotRateState::default()));
    };
    let revision = document
        .get_i64("revision")
        .map_err(|_| OutboxError::Corrupt)?;
    if revision <= 0 || document.get_i32("schema_version").ok() != Some(1) {
        return Err(OutboxError::Corrupt);
    }
    let state: BotRateState = mongodb::bson::deserialize_from_bson(
        document.get("record").ok_or(OutboxError::Corrupt)?.clone(),
    )
    .map_err(|_| OutboxError::Corrupt)?;
    state.ready_at(1_000).map_err(|_| OutboxError::Corrupt)?;
    Ok((revision, state))
}

impl MongoBridgeReplyToolStore {
    async fn write_rate(
        &self,
        key: &str,
        previous: i64,
        state: &BotRateState,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let revision = previous.checked_add(1).ok_or(OutboxError::Corrupt)?;
        let document = doc! {"_id":key,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"schema_version":1_i32,"revision":revision,
        "record":record_bson(state).map_err(|_|OutboxError::Corrupt)?};
        if previous == 0 {
            self.send_rates
                .insert_one(document)
                .session(session)
                .await?;
        } else {
            let changed = self
                .send_rates
                .replace_one(doc! {"_id":key,"revision":previous}, document)
                .session(session)
                .await?;
            if changed.matched_count != 1 {
                return Err(OutboxError::Conflict.into());
            }
        }
        Ok(())
    }

    /// Atomically reserve pacing with the claim. Returning a deadline leaves
    /// the original outbox item and its attempt count unchanged.
    pub(super) async fn reserve_rate(
        &self,
        item: &OutboxItem,
        now: i64,
        session: &mut ClientSession,
    ) -> Result<Option<i64>, MutationError> {
        let key = key(&item.source)?;
        let (revision, mut state) = read(
            self.send_rates
                .find_one(doc! {"_id":&key})
                .session(&mut *session)
                .await?,
        )?;
        match state.reserve(lease(item)?, now, interval(&item.source)) {
            Ok(()) => self.write_rate(&key, revision, &state, session).await?,
            Err(RateError::BusyUntil(at)) => return Ok(Some(at)),
            Err(_) => return Err(OutboxError::Corrupt.into()),
        }
        Ok(None)
    }

    pub(super) async fn finish_rate(
        &self,
        original: &OutboxItem,
        now: i64,
        retry_at: Option<i64>,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let key = key(&original.source)?;
        let (revision, mut state) = read(
            self.send_rates
                .find_one(doc! {"_id":&key})
                .session(&mut *session)
                .await?,
        )?;
        let expected = lease(original)?;
        let cooldown = retry_at.filter(|at| *at > now);
        if state.lease.as_ref() == Some(&expected) {
            state
                .finish(&expected, now, interval(&original.source), cooldown)
                .map_err(|_| OutboxError::Corrupt)?;
        } else {
            // A late persisted observation or an old-version attempt may lack
            // this slot. Keep any newer lease and still honor provider backoff.
            state.next_start_ms = state.next_start_ms.max(
                now.checked_add(interval(&original.source))
                    .ok_or(OutboxError::Corrupt)?,
            );
            state.cooldown_until_ms = state.cooldown_until_ms.max(cooldown.unwrap_or(0));
        }
        self.write_rate(&key, revision, &state, session).await
    }

    pub(super) async fn require_rate(&self, item: &OutboxItem) -> Result<(), OutboxError> {
        let key = key(&item.source)?;
        let (_, state) = read(
            self.send_rates
                .find_one(doc! {"_id":key})
                .await
                .map_err(|_| OutboxError::Unavailable)?,
        )?;
        let expected = lease(item)?;
        let now = now_ms();
        if state.lease.as_ref() == Some(&expected)
            && now < expected.expires_at_ms
            && (now < state.next_start_ms || now < state.cooldown_until_ms)
        {
            return Err(OutboxError::RateLimited);
        }
        state
            .require_lease(&expected, now)
            .map_err(|_| OutboxError::Suspended)
    }
}

#[cfg(test)]
mod tests;
