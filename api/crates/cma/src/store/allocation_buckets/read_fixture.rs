//! Reader fixtures use the production projector/encoding; worker fencing has
//! separate real-Mongo tests. Seeding is not a background-maintenance receipt.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
impl ReportingStore {
    pub(crate) fn allocation_bucket_fixture_document(
        state: &SandboxUsage,
        events: &[UsageEvent],
        range: Range,
    ) -> Result<Document> {
        materialize::build(state, events, range)?.document()
    }
}
