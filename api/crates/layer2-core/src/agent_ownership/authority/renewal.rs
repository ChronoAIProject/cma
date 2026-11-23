use super::Authority;
use crate::agent_ownership::{
    lease_operation::{LeaseWrite, WriteOutcome},
    OwnershipError, OwnershipLease, STORE_DEADLINE_MS,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

impl Authority {
    pub(in crate::agent_ownership) async fn renew_store_owned(
        &self,
    ) -> Result<OwnershipLease, OwnershipError> {
        let operation = {
            let mut retained = self.renewal.lock().unwrap();
            if !self.is_current() {
                return Err(OwnershipError::Stale);
            }
            if retained.as_ref().is_some_and(|operation| matches!(operation.completed(), Some(WriteOutcome::Renewed(ref lease)) if self.validate_renewal(lease).is_ok())) {
                retained.take();
            }
            retained
                .get_or_insert_with(|| {
                    LeaseWrite::renewal(
                        self.store.as_ref().expect("store-backed authority").clone(),
                        self.token.clone(),
                    )
                })
                .clone()
        };
        match operation
            .join_until(Instant::now() + Duration::from_millis(STORE_DEADLINE_MS))
            .await?
        {
            WriteOutcome::Renewed(lease) => {
                self.validate_renewal(&lease)?;
                Ok(lease)
            }
            WriteOutcome::RenewalRefused(error)
            | WriteOutcome::Uncertain(error)
            | WriteOutcome::NotIssued(error) => Err(error),
            WriteOutcome::Acquired { .. } => Err(OwnershipError::Invalid(
                "renewal returned another operation".into(),
            )),
        }
    }

    pub(in crate::agent_ownership) async fn join_renewal_until(
        &self,
        until: Instant,
    ) -> Result<(), OwnershipError> {
        let operation = self.renewal.lock().unwrap().clone();
        let Some(operation) = operation else {
            return Ok(());
        };
        match operation.join_until(until).await? {
            WriteOutcome::Renewed(lease) => self.validate_renewal(&lease),
            // This is termination of the store operation, not authority or
            // receiver settlement. The ordinary caller still receives Stale.
            WriteOutcome::RenewalRefused(OwnershipError::Stale) | WriteOutcome::NotIssued(_) => {
                Ok(())
            }
            WriteOutcome::RenewalRefused(error) | WriteOutcome::Uncertain(error) => Err(error),
            WriteOutcome::Acquired { .. } => Err(OwnershipError::Invalid(
                "renewal returned another operation".into(),
            )),
        }
    }

    pub(in crate::agent_ownership) async fn abort_renewal_until(
        &self,
        until: Instant,
    ) -> Result<(), OwnershipError> {
        let operation: Option<Arc<LeaseWrite>> = self.renewal.lock().unwrap().clone();
        if let Some(operation) = operation {
            operation.abort_until(until).await?;
            match operation.completed() {
                Some(WriteOutcome::Renewed(lease)) => self.validate_renewal(&lease)?,
                Some(WriteOutcome::RenewalRefused(error) | WriteOutcome::Uncertain(error))
                    if !matches!(
                        error,
                        OwnershipError::Unavailable(_) | OwnershipError::Stale
                    ) =>
                {
                    return Err(error)
                }
                _ => (),
            }
        }
        Ok(())
    }

    fn validate_renewal(&self, lease: &OwnershipLease) -> Result<(), OwnershipError> {
        if lease.token != self.token {
            return Err(OwnershipError::Stale);
        }
        if lease.expires_at_ms <= 0 {
            return Err(OwnershipError::Invalid(
                "renewal returned an invalid deadline".into(),
            ));
        }
        Ok(())
    }
}
