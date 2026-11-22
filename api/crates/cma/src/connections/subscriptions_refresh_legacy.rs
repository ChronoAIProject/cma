use super::records::RecordChange;
use super::*;

impl RefreshPort for Legacy<'_> {
    type Record = SubscriptionRecord;
    type Cache = SecretValue;
    type Tokens = SubscriptionTokens;
    type Error = SubscriptionError;

    fn is_sign_in_required(error: &Self::Error) -> bool {
        *error == SubscriptionError::SignInRequired
    }

    async fn get(&self, owner: &SubjectId) -> Result<Option<Self::Record>, Self::Error> {
        self.0.store.get(owner).await
    }

    fn decode_record(&self, record: &Self::Record) -> Result<Self::Tokens, Self::Error> {
        SubscriptionTokens::from_native_cache(cache(record)?)
    }

    fn retain_cache(&self, record: &Self::Record) -> Result<Self::Cache, Self::Error> {
        Ok(cache(record)?.clone())
    }

    fn decode_cache(&self, cache: &Self::Cache) -> Result<Self::Tokens, Self::Error> {
        SubscriptionTokens::from_native_cache(cache)
    }

    fn claim(
        &self,
        record: &Self::Record,
        lease_lifetime: Duration,
    ) -> Result<Self::Record, Self::Error> {
        let holder = uuid::Uuid::new_v4().to_string();
        Ok(RecordChange::Claim {
            holder: &holder,
            lease_lifetime,
        }
        .build(record))
    }

    fn finish(
        &self,
        record: &Self::Record,
        cache: CacheUpdate<'_, Self::Cache>,
    ) -> Result<Self::Record, Self::Error> {
        Ok(RecordChange::Finish(cache).build(record))
    }

    async fn replace(
        &self,
        expected: &Self::Record,
        next: Self::Record,
    ) -> Result<bool, Self::Error> {
        self.0.store.replace(expected, next).await
    }

    async fn refresh(&self, record: &Self::Record) -> Result<Self::Cache, Self::Error> {
        self.0.native.refresh(cache(record)?).await
    }
}
