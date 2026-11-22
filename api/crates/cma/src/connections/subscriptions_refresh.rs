//! One refresh authority algorithm, with allocation and source work at ports.

use std::{
    borrow::Borrow,
    future::Future,
    time::{Duration, SystemTime},
};

use super::{
    CodexSubscriptions, SecretValue, SubjectId, SubscriptionError, SubscriptionRecord,
    SubscriptionTokens, CAS_ATTEMPTS, REFRESH_LEASE,
};

#[path = "subscriptions_refresh_admitted.rs"]
pub(super) mod admitted;
#[path = "subscriptions_refresh_legacy.rs"]
mod legacy;
#[path = "subscriptions_refresh_records.rs"]
mod records;
pub(super) struct Legacy<'a>(pub(super) &'a CodexSubscriptions);

pub(in crate::connections) enum CacheUpdate<'a, T> {
    Replace(&'a T),
    Clear,
    Keep,
}

/// Implementations own allocation and I/O, while the engine retains the same
/// account, lease, incarnation and retry decisions for every representation.
pub(super) trait RefreshPort: Sync {
    type Record: Borrow<SubscriptionRecord> + Clone + Send + Sync;
    type Cache: Borrow<SecretValue> + Send + Sync;
    type Tokens: Borrow<SubscriptionTokens> + Send;
    type Error: From<SubscriptionError> + Send;

    fn is_sign_in_required(error: &Self::Error) -> bool;
    fn get(
        &self,
        owner: &SubjectId,
    ) -> impl Future<Output = Result<Option<Self::Record>, Self::Error>> + Send;
    fn decode_record(&self, record: &Self::Record) -> Result<Self::Tokens, Self::Error>;
    fn retain_cache(&self, record: &Self::Record) -> Result<Self::Cache, Self::Error>;
    fn decode_cache(&self, cache: &Self::Cache) -> Result<Self::Tokens, Self::Error>;
    fn claim(
        &self,
        record: &Self::Record,
        lease_lifetime: Duration,
    ) -> Result<Self::Record, Self::Error>;
    fn finish(
        &self,
        record: &Self::Record,
        cache: CacheUpdate<'_, Self::Cache>,
    ) -> Result<Self::Record, Self::Error>;
    fn replace(
        &self,
        expected: &Self::Record,
        next: Self::Record,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;
    fn refresh(
        &self,
        record: &Self::Record,
    ) -> impl Future<Output = Result<Self::Cache, Self::Error>> + Send;
}

pub(super) fn cache(record: &SubscriptionRecord) -> Result<&SecretValue, SubscriptionError> {
    record
        .cache
        .as_ref()
        .ok_or(SubscriptionError::SignInRequired)
}

async fn record<P: RefreshPort>(port: &P, owner: &SubjectId) -> Result<P::Record, P::Error> {
    port.get(owner)
        .await?
        .filter(|record| record.borrow().owner == *owner)
        .ok_or_else(|| SubscriptionError::SignInRequired.into())
}

pub(super) async fn tokens_within<P: RefreshPort>(
    port: &P,
    owner: &SubjectId,
    force: bool,
    previous_account: Option<&str>,
) -> Result<P::Tokens, P::Error> {
    let mut original_cache: Option<P::Cache> = None;
    loop {
        let current = record(port, owner).await?;
        let current_cache = cache(current.borrow())?;
        let tokens = port.decode_record(&current)?;
        if previous_account.is_some_and(|account| account != tokens.borrow().account_id) {
            return Err(SubscriptionError::SignInRequired.into());
        }
        let refreshed_elsewhere = original_cache
            .as_ref()
            .is_some_and(|original| original.borrow() != current_cache);
        if tokens.borrow().usable_at(SystemTime::now()) && (!force || refreshed_elsewhere) {
            return Ok(tokens);
        }
        if original_cache.is_none() {
            original_cache = Some(port.retain_cache(&current)?);
        }
        if current
            .borrow()
            .refresh_lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at > SystemTime::now())
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        }
        let claimed = port.claim(&current, REFRESH_LEASE)?;
        if !port.replace(&current, claimed.clone()).await? {
            continue;
        }
        let refreshed = port.refresh(&current).await.and_then(|cache| {
            let renewed = port.decode_cache(&cache)?;
            if renewed.borrow().account_id != tokens.borrow().account_id
                || !renewed.borrow().usable_at(SystemTime::now())
            {
                return Err(SubscriptionError::InvalidAccount.into());
            }
            Ok(cache)
        });
        // New sign-in and lease replacement both invalidate a stale refresher.
        for _ in 0..CAS_ATTEMPTS {
            let latest = record(port, owner).await?;
            if latest.borrow().incarnation != claimed.borrow().incarnation
                || latest
                    .borrow()
                    .refresh_lease
                    .as_ref()
                    .zip(claimed.borrow().refresh_lease.as_ref())
                    .is_none_or(|(latest, claimed)| latest.holder != claimed.holder)
            {
                return Err(SubscriptionError::Changed.into());
            }
            let change = match &refreshed {
                Ok(cache) => {
                    port.decode_cache(cache)?;
                    CacheUpdate::Replace(cache)
                }
                Err(error) if P::is_sign_in_required(error) => CacheUpdate::Clear,
                Err(_) => CacheUpdate::Keep,
            };
            let next = port.finish(&latest, change)?;
            if port.replace(&latest, next).await? {
                return port.decode_cache(&refreshed?);
            }
        }
        return Err(SubscriptionError::Changed.into());
    }
}
