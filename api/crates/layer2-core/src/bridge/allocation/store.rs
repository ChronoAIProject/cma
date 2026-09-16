use super::{
    AllocationError, AllocationIdentity, AllocationMutation, AllocationRecord,
    AllocationReservation,
};

/// Internal persistence port. Public reads require bridge management/read
/// authorization in the calling service. Every mutation fences current bridge,
/// custody, and (for retained targets) exact invoke grant/resource authority in
/// the same transaction as the head and allocation records.
#[async_trait::async_trait]
pub trait AllocationStore: Send + Sync {
    async fn current(
        &self,
        identity: &AllocationIdentity,
    ) -> Result<Option<AllocationRecord>, AllocationError>;
    async fn generation(
        &self,
        identity: &AllocationIdentity,
    ) -> Result<Option<AllocationRecord>, AllocationError>;
    /// Ordinary reconfiguration reuses the current head. Only an explicit reset
    /// may advance its generation, retiring the predecessor atomically.
    async fn reserve(
        &self,
        write: AllocationReservation,
    ) -> Result<AllocationRecord, AllocationError>;
    /// Compare the complete previous record as well as its revision and fence.
    /// Unknown commit may only reconcile an exact majority-durable result.
    async fn mutate(&self, write: AllocationMutation) -> Result<AllocationRecord, AllocationError>;
}
