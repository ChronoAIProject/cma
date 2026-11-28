use super::*;

impl MongoNativeWarmPools {
    pub(super) async fn finish_attempt(
        &self,
        target: &PoolTarget,
        failure: Option<PoolFailure>,
    ) -> Result<(), WarmPoolError> {
        let observed = DateTime::now().timestamp_millis();
        let retry = observed + 15_000;
        let mut fields = doc! {
            "lease": mongodb::bson::Bson::Null,
            "lease_until": DateTime::from_millis(0),
            "reconcile_after": DateTime::from_millis(retry),
            "error": failure.map_or(mongodb::bson::Bson::Null, |failure| failure.message.into()),
            "reconciliation_failure": failure.map_or(mongodb::bson::Bson::Null, |failure| doc! {
                "phase": failure.phase, "reason": failure.reason,
                "observed_at_ms": observed, "retry_after_ms": retry,
            }.into()),
        };
        let mut update = doc! {};
        if failure.is_none() {
            fields.insert("applied_revision", target.revision);
            update.insert(
                "$unset",
                doc! {"preparation.pending_revision": "", "preparation.effect_until": ""},
            );
        }
        update.insert("$set", fields);
        self.check_fenced_write(self.targets.update_one(fence(target), update).await)
            .await
    }
}
