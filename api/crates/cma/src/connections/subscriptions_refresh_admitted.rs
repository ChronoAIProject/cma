use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, SharedOwned,
};

use super::{
    CacheUpdate, CodexSubscriptions, Duration, RefreshPort, SecretValue, SubjectId,
    SubscriptionError, SubscriptionRecord, SubscriptionTokens,
};
use crate::connections::{
    subscription_store::SubscriptionOperation, subscription_tokens::CacheDecodeError,
};

#[path = "subscriptions_refresh_custody.rs"]
mod custody;
use custody::{check, invalid, source, Retained};

#[derive(Debug)]
enum Error {
    Domain(SubscriptionError),
    Source(AdmittedError),
}

impl From<SubscriptionError> for Error {
    fn from(error: SubscriptionError) -> Self {
        Self::Domain(error)
    }
}
impl From<AdmittedError> for Error {
    fn from(error: AdmittedError) -> Self {
        Self::Source(error)
    }
}
impl From<AdmissionRefusal> for Error {
    fn from(error: AdmissionRefusal) -> Self {
        Self::Source(error.into())
    }
}
impl From<CacheDecodeError> for Error {
    fn from(error: CacheDecodeError) -> Self {
        match error {
            CacheDecodeError::Account(error) => Self::Domain(error),
            CacheDecodeError::Refused(error) => error.into(),
        }
    }
}

struct Admitted<'a> {
    manager: &'a CodexSubscriptions,
    owner: &'a SharedOwned<SubjectId>,
    policy: IngressOwnershipPolicy,
}

pub(in crate::connections::subscriptions) fn tokens<'a>(
    manager: &'a CodexSubscriptions,
    owner: &'a SharedOwned<SubjectId>,
    force: bool,
    previous_account: Option<&'a SharedOwned<String>>,
    policy: &IngressOwnershipPolicy,
) -> Result<SubscriptionOperation<'a, SharedOwned<SubscriptionTokens>>, AdmittedError> {
    check(owner, policy)?;
    if let Some(previous) = previous_account {
        check(previous, policy)?;
    }
    let port = Admitted {
        manager,
        owner,
        policy: policy.clone(),
    };
    // Payload copies admit independently. The compiler-sized outer future and
    // scope retain inline engine state; timer/executor storage is separate.
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Data,
        AllocationPlan::checked("subscription refresh orchestration", &[0])?,
        move |scope| async move {
            let result = tokio::time::timeout(
                super::super::REFRESH_BUDGET,
                super::tokens_within(&port, owner, force, previous_account.map(|p| p.as_str())),
            )
            .await
            .unwrap_or(Err(Error::Domain(SubscriptionError::Unavailable)));
            let result = match result {
                Ok(tokens) => {
                    check(&tokens.0, &port.policy)?;
                    Ok(tokens.0)
                }
                Err(Error::Domain(error)) => Err(error),
                Err(Error::Source(error)) => return Err(custody::error(error, &port.policy)),
            };
            Ok(scope.initialize_owned(|| result))
        },
    )
}

impl RefreshPort for Admitted<'_> {
    type Record = Retained<SubscriptionRecord>;
    type Cache = Retained<SecretValue>;
    type Tokens = Retained<SubscriptionTokens>;
    type Error = Error;

    fn is_sign_in_required(error: &Error) -> bool {
        matches!(error, Error::Domain(SubscriptionError::SignInRequired))
    }

    async fn get(&self, owner: &SubjectId) -> Result<Option<Self::Record>, Error> {
        check(self.owner, &self.policy)?;
        if owner != &**self.owner {
            return Err(invalid().into());
        }
        let result = source(
            self.manager.store.get_admitted(self.owner, &self.policy),
            &self.policy,
        )
        .await?;
        match &*result {
            Ok(record) => record
                .as_ref()
                .map(|record| {
                    check(record, &self.policy)?;
                    Ok(Retained(record.clone()))
                })
                .transpose(),
            Err(error) => Err((*error).into()),
        }
    }

    fn decode_record(&self, record: &Self::Record) -> Result<Self::Tokens, Error> {
        self.decode_cache(&self.retain_cache(record)?)
    }

    fn retain_cache(&self, record: &Self::Record) -> Result<Self::Cache, Error> {
        SubscriptionRecord::cache_admitted(&record.0, &self.policy)?
            .map(Retained)
            .ok_or_else(|| SubscriptionError::SignInRequired.into())
    }

    fn decode_cache(&self, cache: &Self::Cache) -> Result<Self::Tokens, Error> {
        let tokens = SubscriptionTokens::from_native_cache_admitted(&cache.0, &self.policy)?;
        check(&tokens, &self.policy)?;
        Ok(Retained(tokens))
    }

    fn claim(
        &self,
        record: &Self::Record,
        lease_lifetime: Duration,
    ) -> Result<Self::Record, Error> {
        check(&record.0, &self.policy)?;
        let holder = self
            .policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "subscription refresh holder",
                    &[sizing::shared_owner_bytes::<String>(), 36],
                )?,
            )?
            .initialize(|| uuid::Uuid::new_v4().to_string());
        Ok(Retained(SubscriptionRecord::claim_admitted(
            &record.0,
            &holder,
            lease_lifetime,
            &self.policy,
        )?))
    }

    fn finish(
        &self,
        record: &Self::Record,
        cache: CacheUpdate<'_, Self::Cache>,
    ) -> Result<Self::Record, Error> {
        let cache = match cache {
            CacheUpdate::Replace(value) => CacheUpdate::Replace(&value.0),
            CacheUpdate::Clear => CacheUpdate::Clear,
            CacheUpdate::Keep => CacheUpdate::Keep,
        };
        Ok(Retained(SubscriptionRecord::finish_admitted(
            &record.0,
            cache,
            &self.policy,
        )?))
    }

    async fn replace(&self, expected: &Self::Record, next: Self::Record) -> Result<bool, Error> {
        check(&expected.0, &self.policy)?;
        check(&next.0, &self.policy)?;
        let result = source(
            self.manager
                .store
                .replace_admitted(&expected.0, &next.0, &self.policy),
            &self.policy,
        )
        .await?;
        (*result).map_err(Into::into)
    }

    async fn refresh(&self, record: &Self::Record) -> Result<Self::Cache, Error> {
        let cache = self.retain_cache(record)?;
        let result = source(
            self.manager.native.refresh_admitted(&cache.0, &self.policy),
            &self.policy,
        )
        .await?;
        match &*result {
            Ok(cache) => {
                check(cache, &self.policy)?;
                Ok(Retained(cache.clone()))
            }
            Err(error) => Err((*error).into()),
        }
    }
}

#[cfg(test)]
#[path = "subscriptions_refresh_admitted_tests.rs"]
mod tests;
