//! Uncached channel authority and durable partition assignment storage.
use super::*;

impl ChannelResolver {
    /// Recheck live channel authority without accepting a cached definition.
    pub async fn resolve_current(
        &self,
        raw_id: &str,
    ) -> Result<Arc<ResolvedChannel>, ResolveError> {
        let id = ChannelId::new(raw_id).map_err(|_| ResolveError::Invalid)?;
        let pointer = id
            .open(&self.inner.keys)
            .map_err(|_| ResolveError::Tampered)?
            .payload;
        self.inner
            .records
            .record_url(pointer.record_id.as_str())
            .map_err(|_| ResolveError::Invalid)?;
        if !self
            .inner
            .records
            .names_record(&pointer.record_url, pointer.record_id.as_str())
        {
            return Err(ResolveError::Tampered);
        }
        tokio::time::timeout(self.inner.config.restore_timeout, self.fetch(id, pointer))
            .await
            .unwrap_or(Err(ResolveError::Unavailable))
    }
    pub(crate) fn profile_assignment_store(
        &self,
    ) -> Result<crate::replicas::store::Store, ResolveError> {
        crate::replicas::store::Store::new((*self.inner.records).clone(), self.inner.keys.clone())
            .map_err(|_| ResolveError::Unavailable)
    }
}
